use std::{collections::HashSet, path::Path, time::Duration};

use crate::{
    auth::{Credentials, SecretString},
    bbs::{
        BbsClient, BbsError, CoinSummary, ForumSignRequest, ForumSignState, ForumSpec, MissionKind,
        PostRef, forum_by_id,
    },
    captcha::CaptchaClient,
    config::{AccountConfig, BbsTaskConfig, Config},
    http::{HttpClient, RetryPolicy},
    signing::{DsSigner, SystemClock, ThreadRandom},
};

use super::{
    CredentialPersistence, RunReport, TaskOutcome, TaskRecord,
    credential_refresh::{has_authentication_failure, refresh_account_cookie},
    resolve_device_id,
};

const READ_TARGET: u32 = 3;
const LIKE_TARGET: u32 = 5;
#[cfg(not(test))]
const BBS_CONFIRM_DELAY: Duration = Duration::from_secs(3);
#[cfg(test)]
const BBS_CONFIRM_DELAY: Duration = Duration::ZERO;
#[cfg(not(test))]
const CAPTCHA_RETRY_DELAY: Duration = Duration::from_secs(3);
#[cfg(test)]
const CAPTCHA_RETRY_DELAY: Duration = Duration::ZERO;

pub async fn run_bbs(config: &Config) -> RunReport {
    let mut runtime_config = config.clone();
    run_bbs_inner(&mut runtime_config, CredentialPersistence::ReadOnly).await
}

pub async fn run_bbs_with_refresh(config: &mut Config, path: &Path) -> RunReport {
    run_bbs_with_persistence(config, CredentialPersistence::CurrentConfig(path)).await
}

pub async fn run_bbs_with_persistence(
    config: &mut Config,
    persistence: CredentialPersistence<'_>,
) -> RunReport {
    run_bbs_inner(config, persistence).await
}

async fn run_bbs_inner(config: &mut Config, persistence: CredentialPersistence<'_>) -> RunReport {
    let mut report = RunReport::default();
    for account_index in 0..config.accounts.len() {
        let account = config.accounts[account_index].clone();
        if account.enabled && account.tasks.bbs.is_enabled() {
            let mut first = RunReport::default();
            run_account(&mut first, config, &account).await;
            if !has_authentication_failure(&first) {
                report.extend(first);
                continue;
            }

            let refresh_http = match build_http(config, &account) {
                Ok(http) => http,
                Err(message) => {
                    report.extend(first);
                    report.push(record(
                        &account.name,
                        "凭据刷新",
                        "cookie_token",
                        TaskOutcome::NetworkFailed,
                        message,
                    ));
                    continue;
                }
            };
            match refresh_account_cookie(config, account_index, refresh_http, persistence.path())
                .await
            {
                Ok(persisted) => {
                    report.push(record(
                        &account.name,
                        "凭据刷新",
                        "cookie_token",
                        TaskOutcome::Success,
                        match (persistence, persisted) {
                            (CredentialPersistence::CurrentConfig(_), true) => {
                                "检测到认证失效，已通过 SToken 刷新、写回配置并重试"
                            }
                            (CredentialPersistence::CurrentConfig(_), false) => {
                                "检测到认证失效，已通过 SToken 刷新并重试；Cookie 来自环境变量，未写入配置文件"
                            }
                            (CredentialPersistence::ReadOnly, _) => {
                                "检测到认证失效，已通过 SToken 刷新并重试；当前配置源为只读，未写入配置文件"
                            }
                        },
                    ));
                    let refreshed = config.accounts[account_index].clone();
                    run_account(&mut report, config, &refreshed).await;
                }
                Err(message) => {
                    report.extend(first);
                    report.push(record(
                        &account.name,
                        "凭据刷新",
                        "cookie_token",
                        TaskOutcome::AuthenticationFailed,
                        &format!("自动刷新失败：{message}"),
                    ));
                }
            }
        }
    }
    report
}

async fn run_account(report: &mut RunReport, config: &Config, account: &AccountConfig) {
    let http = match build_http(config, account) {
        Ok(http) => http,
        Err(message) => {
            report.push(record(
                &account.name,
                "米游社任务",
                "HTTP 客户端",
                TaskOutcome::NetworkFailed,
                message,
            ));
            return;
        }
    };

    let mut credentials = Credentials::new(
        account.credentials.cookie.expose_secret(),
        account.credentials.stoken.expose_secret(),
    );
    if credentials.hydrate_from_cookie().is_err() {
        report.push(record(
            &account.name,
            "米游社任务",
            "认证",
            TaskOutcome::AuthenticationFailed,
            "Cookie、SToken、UID 或 MID 不完整",
        ));
        return;
    }
    let app_cookie = match credentials.stoken_cookie() {
        Ok(cookie) => cookie,
        Err(_) => {
            report.push(record(
                &account.name,
                "米游社任务",
                "认证",
                TaskOutcome::AuthenticationFailed,
                "无法构造 SToken Cookie",
            ));
            return;
        }
    };

    let web_cookie = SecretString::new(account.credentials.cookie.expose_secret());
    let device_id = resolve_device_id(
        &account.device.id,
        account.credentials.cookie.expose_secret(),
    );
    let captcha = config.captcha.endpoint.clone().map(|endpoint| {
        CaptchaClient::new(
            http.clone().with_retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            }),
            endpoint,
        )
    });
    let client = BbsClient::new(http, app_cookie, web_cookie, device_id).device(
        &account.device.name,
        &account.device.model,
        &account.device.fp,
    );
    let mut signer = DsSigner::new(SystemClock, ThreadRandom);

    run_account_tasks(
        report,
        &account.name,
        &account.tasks.bbs,
        &client,
        captcha.as_ref(),
        &mut signer,
        config.runtime.task_max_attempts.max(1),
    )
    .await;
}

struct BbsTaskContext<'a> {
    report: &'a mut RunReport,
    account: &'a str,
    client: &'a BbsClient,
    captcha: Option<&'a CaptchaClient>,
    signer: &'a mut DsSigner<SystemClock, ThreadRandom>,
    max_attempts: u32,
    logged_unknown_missions: &'a mut HashSet<i64>,
    latest_summary: &'a mut Option<CoinSummary>,
    latest_is_post_action: bool,
    coin_delta_allowed: bool,
    coin_delta_tainted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskRunResult {
    Confirmed,
    TimedOut,
    StopTask,
    StopAccount,
}

async fn run_account_tasks(
    report: &mut RunReport,
    account: &str,
    tasks: &BbsTaskConfig,
    client: &BbsClient,
    captcha: Option<&CaptchaClient>,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    max_attempts: u32,
) {
    let forums = tasks
        .forums
        .iter()
        .filter_map(|id| forum_by_id(*id))
        .collect::<Vec<_>>();
    if forums.is_empty() {
        report.push(record(
            account,
            "米游社任务",
            "社区板块",
            TaskOutcome::Failed,
            "没有可用的社区板块，请检查 tasks.bbs.forums",
        ));
        return;
    }

    let plan = BbsPlan::from_config(tasks);
    let mut logged_unknown_missions = HashSet::new();
    let mut latest_summary = match client.missions().await {
        Ok(summary) => {
            log_unknown_missions(account, &summary, &mut logged_unknown_missions);
            tracing::info!(
                account,
                already_received_points = summary.already_received_points,
                can_get_points = summary.can_get_points,
                total_points = summary.total_points,
                "已记录社区任务执行前的实时米游币基线"
            );
            Some(summary)
        }
        Err(error) => {
            tracing::warn!(
                account,
                error = %error,
                "米游币初始状态查询失败，账号配置仍授权执行首轮任务"
            );
            None
        }
    };

    let has_initial_summary = latest_summary.is_some();
    let mut context = BbsTaskContext {
        report,
        account,
        client,
        captcha,
        signer,
        max_attempts: max_attempts.max(1),
        logged_unknown_missions: &mut logged_unknown_missions,
        latest_summary: &mut latest_summary,
        latest_is_post_action: false,
        coin_delta_allowed: has_initial_summary,
        coin_delta_tainted: false,
    };
    let mut all_confirmed = true;

    if plan.sign {
        let result = run_sign_task(&mut context, &forums).await;
        if apply_task_result(result, &mut context, &mut all_confirmed) {
            push_latest_coin_summary(&mut context, all_confirmed);
            return;
        }
    }
    if plan.read > 0 {
        prepare_task_baseline(&mut context, "阅读").await;
        let result = run_read_task(&mut context, forums[0], plan.read).await;
        if apply_task_result(result, &mut context, &mut all_confirmed) {
            push_latest_coin_summary(&mut context, all_confirmed);
            return;
        }
    }
    if plan.like > 0 {
        prepare_task_baseline(&mut context, "点赞").await;
        let result = run_like_task(&mut context, forums[0], plan.like, tasks.cancel_like).await;
        if apply_task_result(result, &mut context, &mut all_confirmed) {
            push_latest_coin_summary(&mut context, all_confirmed);
            return;
        }
    }
    if plan.share {
        prepare_task_baseline(&mut context, "分享").await;
        let result = run_share_task(&mut context, forums[0]).await;
        if apply_task_result(result, &mut context, &mut all_confirmed) {
            push_latest_coin_summary(&mut context, all_confirmed);
            return;
        }
    }
    push_latest_coin_summary(&mut context, all_confirmed);
}

fn apply_task_result(
    result: TaskRunResult,
    context: &mut BbsTaskContext<'_>,
    all_confirmed: &mut bool,
) -> bool {
    match result {
        TaskRunResult::Confirmed => false,
        TaskRunResult::TimedOut | TaskRunResult::StopTask => {
            *all_confirmed = false;
            context.coin_delta_allowed = false;
            context.coin_delta_tainted = true;
            false
        }
        TaskRunResult::StopAccount => {
            *all_confirmed = false;
            context.coin_delta_allowed = false;
            context.coin_delta_tainted = true;
            true
        }
    }
}

fn push_latest_coin_summary(context: &mut BbsTaskContext<'_>, all_confirmed: bool) {
    if !context.latest_is_post_action {
        return;
    }
    let Some(summary) = context.latest_summary.as_ref().cloned() else {
        return;
    };
    let subject = if all_confirmed {
        "完成确认"
    } else {
        "实时汇总"
    };
    context
        .report
        .push(coin_summary_record(context.account, subject, &summary));
}

async fn prepare_task_baseline(context: &mut BbsTaskContext<'_>, task: &str) {
    if context.coin_delta_allowed {
        return;
    }
    match context.client.missions().await {
        Ok(summary) => {
            log_unknown_missions(context.account, &summary, context.logged_unknown_missions);
            tracing::info!(
                account = context.account,
                task,
                already_received_points = summary.already_received_points,
                can_get_points = summary.can_get_points,
                total_points = summary.total_points,
                "已在执行下一项社区任务前刷新米游币归因基线"
            );
            *context.latest_summary = Some(summary);
            context.latest_is_post_action = true;
            context.coin_delta_allowed = !context.coin_delta_tainted;
        }
        Err(error) => tracing::warn!(
            account = context.account,
            task,
            error_kind = bbs_error_kind(&error),
            "下一项社区任务执行前无法刷新基线，将禁用米游币差值确认"
        ),
    }
}

fn coin_summary_record(account: &str, subject: &str, summary: &CoinSummary) -> TaskRecord {
    record(
        account,
        "米游币",
        subject,
        TaskOutcome::Success,
        &format!(
            "已领取 {}，还可领取 {}，当前共 {} 米游币",
            summary.already_received_points, summary.can_get_points, summary.total_points
        ),
    )
}

async fn run_sign_task(context: &mut BbsTaskContext<'_>, forums: &[&ForumSpec]) -> TaskRunResult {
    let mut completed = Vec::new();
    for attempt in 1..=context.max_attempts {
        let before = current_task_baseline(context);
        let completed_before = completed.len();
        let mut all_idempotent = true;
        let mut blocking_error = false;

        for forum in forums {
            let request = ForumSignRequest::new(forum.gids);
            match sign_forum_with_captcha(
                context.client,
                context.captcha,
                context.signer,
                &request,
                context.max_attempts,
            )
            .await
            {
                Ok(result) => {
                    all_idempotent &= result.already_signed;
                    completed.push((
                        forum.name.to_owned(),
                        result.captcha_solved,
                        result.already_signed,
                    ));
                }
                Err(ActionError::Captcha(message)) => {
                    all_idempotent = false;
                    push_captcha_error(context.report, context.account, forum.name, &message);
                    blocking_error = true;
                    break;
                }
                Err(ActionError::Bbs(error)) => {
                    all_idempotent = false;
                    let terminal = is_terminal(&error);
                    push_error(context.report, context.account, forum.name, error);
                    if terminal {
                        blocking_error = true;
                        break;
                    }
                }
            }
        }

        let completed_this_round = completed.len() - completed_before;
        if completed_this_round == 0 {
            return if blocking_error {
                TaskRunResult::StopAccount
            } else {
                TaskRunResult::StopTask
            };
        }
        let after = match recheck_task(context, "社区签到", attempt).await {
            Ok(after) => after,
            Err(result) => return result,
        };
        log_task_recheck(
            context.account,
            "社区签到",
            attempt,
            before.as_ref(),
            &after,
        );
        let idempotent = completed_this_round == forums.len() && all_idempotent;
        let confirmed = task_confirmed(before.as_ref(), &after, MissionKind::Sign, idempotent);
        let credited_points = credited_points(before.as_ref(), &after);
        store_rechecked_summary(context, after);
        if confirmed {
            push_confirmed_signs(
                context.report,
                context.account,
                &mut completed,
                attempt,
                credited_points,
            );
            return if blocking_error {
                TaskRunResult::StopAccount
            } else {
                TaskRunResult::Confirmed
            };
        }
        if blocking_error {
            return TaskRunResult::StopAccount;
        }
        if attempt == context.max_attempts {
            push_state_sync_timeout(context.report, context.account, "社区签到", attempt);
            return TaskRunResult::TimedOut;
        }
        log_task_retry(context.account, "社区签到", attempt, context.max_attempts);
    }
    TaskRunResult::TimedOut
}

async fn run_read_task(
    context: &mut BbsTaskContext<'_>,
    forum: &ForumSpec,
    first_count: u32,
) -> TaskRunResult {
    let mut completed = Vec::new();
    let mut used = HashSet::new();
    for attempt in 1..=context.max_attempts {
        let count = if attempt == 1 {
            first_count
        } else {
            retry_action_count(
                context.latest_summary.as_ref(),
                MissionKind::Read,
                READ_TARGET,
            )
        };
        let selected = match select_task_posts(context, forum, count, &mut used).await {
            Ok(selected) => selected,
            Err(result) => return result,
        };
        let before = current_task_baseline(context);
        let completed_before = completed.len();
        let signal = run_reads(
            context.report,
            context.account,
            context.client,
            context.signer,
            &selected,
            count,
            &mut completed,
        )
        .await;
        if completed.len() == completed_before {
            return flow_stop_result(signal);
        }
        let after = match recheck_task(context, "阅读", attempt).await {
            Ok(after) => after,
            Err(result) => return result,
        };
        log_task_recheck(context.account, "阅读", attempt, before.as_ref(), &after);
        let confirmed = task_confirmed(before.as_ref(), &after, MissionKind::Read, false);
        let credited_points = credited_points(before.as_ref(), &after);
        store_rechecked_summary(context, after);
        if confirmed {
            push_confirmed_posts(
                context.report,
                context.account,
                "阅读",
                &mut completed,
                attempt,
                credited_points,
            );
            return if signal == FlowSignal::Continue {
                TaskRunResult::Confirmed
            } else {
                TaskRunResult::StopAccount
            };
        }
        if signal != FlowSignal::Continue {
            return TaskRunResult::StopAccount;
        }
        if attempt == context.max_attempts {
            push_state_sync_timeout(context.report, context.account, "阅读", attempt);
            return TaskRunResult::TimedOut;
        }
        log_task_retry(context.account, "阅读", attempt, context.max_attempts);
    }
    TaskRunResult::TimedOut
}

async fn run_like_task(
    context: &mut BbsTaskContext<'_>,
    forum: &ForumSpec,
    first_count: u32,
    cancel_like: bool,
) -> TaskRunResult {
    let mut completed = Vec::new();
    let mut used = HashSet::new();
    for attempt in 1..=context.max_attempts {
        let count = if attempt == 1 {
            first_count
        } else {
            retry_action_count(
                context.latest_summary.as_ref(),
                MissionKind::Like,
                LIKE_TARGET,
            )
        };
        let selected = match select_task_posts(context, forum, count, &mut used).await {
            Ok(selected) => selected,
            Err(result) => return result,
        };
        let before = current_task_baseline(context);
        let completed_before = completed.len();
        let mut signal = FlowSignal::Continue;

        for post in &selected {
            match set_like_with_captcha(
                context.client,
                context.captcha,
                context.signer,
                &post.post_id,
                false,
                context.max_attempts,
            )
            .await
            {
                Ok(solved) => completed.push((post.clone(), solved)),
                Err(ActionError::Captcha(message)) => {
                    push_captcha_error(context.report, context.account, &post.subject, &message);
                    signal = FlowSignal::BlockHighRisk;
                    break;
                }
                Err(ActionError::Bbs(error)) => {
                    let terminal = is_terminal(&error);
                    push_error(context.report, context.account, &post.subject, error);
                    if terminal {
                        signal = FlowSignal::Stop;
                        break;
                    }
                    continue;
                }
            }

            if !cancel_like {
                continue;
            }
            match set_like_with_captcha(
                context.client,
                context.captcha,
                context.signer,
                &post.post_id,
                true,
                context.max_attempts,
            )
            .await
            {
                Ok(solved) => context.report.push(post_record(
                    context.account,
                    "取消点赞",
                    post,
                    TaskOutcome::Success,
                    if solved {
                        "验证码通过后已恢复点赞状态"
                    } else {
                        "已恢复点赞状态"
                    },
                )),
                Err(ActionError::Captcha(message)) => {
                    push_captcha_error(context.report, context.account, &post.subject, &message);
                    signal = FlowSignal::BlockHighRisk;
                    break;
                }
                Err(ActionError::Bbs(error)) => {
                    let terminal = is_terminal(&error);
                    push_error(context.report, context.account, &post.subject, error);
                    if terminal {
                        signal = FlowSignal::Stop;
                        break;
                    }
                }
            }
        }

        if completed.len() == completed_before {
            return flow_stop_result(signal);
        }
        let after = match recheck_task(context, "点赞", attempt).await {
            Ok(after) => after,
            Err(result) => return result,
        };
        log_task_recheck(context.account, "点赞", attempt, before.as_ref(), &after);
        let confirmed = task_confirmed(before.as_ref(), &after, MissionKind::Like, false);
        let credited_points = credited_points(before.as_ref(), &after);
        store_rechecked_summary(context, after);
        if confirmed {
            push_confirmed_likes(
                context.report,
                context.account,
                &mut completed,
                attempt,
                credited_points,
            );
            return if signal == FlowSignal::Continue {
                TaskRunResult::Confirmed
            } else {
                TaskRunResult::StopAccount
            };
        }
        if signal != FlowSignal::Continue {
            return TaskRunResult::StopAccount;
        }
        if attempt == context.max_attempts {
            push_state_sync_timeout(context.report, context.account, "点赞", attempt);
            return TaskRunResult::TimedOut;
        }
        log_task_retry(context.account, "点赞", attempt, context.max_attempts);
    }
    TaskRunResult::TimedOut
}

async fn run_share_task(context: &mut BbsTaskContext<'_>, forum: &ForumSpec) -> TaskRunResult {
    let mut completed = Vec::new();
    let mut used = HashSet::new();
    for attempt in 1..=context.max_attempts {
        let selected = match select_task_posts(context, forum, 1, &mut used).await {
            Ok(selected) => selected,
            Err(result) => return result,
        };
        let Some(post) = selected.first() else {
            return TaskRunResult::StopTask;
        };
        let before = current_task_baseline(context);
        let ds = context.signer.sign_app().to_string();
        match context.client.share_post(&post.post_id, &ds).await {
            Ok(()) => completed.push(post.clone()),
            Err(error) => {
                let blocking = is_account_blocking(&error);
                push_error(context.report, context.account, &post.subject, error);
                return if blocking {
                    TaskRunResult::StopAccount
                } else {
                    TaskRunResult::StopTask
                };
            }
        }
        let after = match recheck_task(context, "分享", attempt).await {
            Ok(after) => after,
            Err(result) => return result,
        };
        log_task_recheck(context.account, "分享", attempt, before.as_ref(), &after);
        let confirmed = task_confirmed(before.as_ref(), &after, MissionKind::Share, false);
        let credited_points = credited_points(before.as_ref(), &after);
        store_rechecked_summary(context, after);
        if confirmed {
            push_confirmed_posts(
                context.report,
                context.account,
                "分享",
                &mut completed,
                attempt,
                credited_points,
            );
            return TaskRunResult::Confirmed;
        }
        if attempt == context.max_attempts {
            push_state_sync_timeout(context.report, context.account, "分享", attempt);
            return TaskRunResult::TimedOut;
        }
        log_task_retry(context.account, "分享", attempt, context.max_attempts);
    }
    TaskRunResult::TimedOut
}

async fn select_task_posts(
    context: &mut BbsTaskContext<'_>,
    forum: &ForumSpec,
    count: u32,
    used: &mut HashSet<String>,
) -> Result<Vec<PostRef>, TaskRunResult> {
    let ds = context.signer.sign_app().to_string();
    let posts = match context.client.posts(forum.forum_id, 20, &ds).await {
        Ok(posts) => posts,
        Err(error) => {
            let blocking = is_account_blocking(&error);
            push_error(context.report, context.account, "帖子列表", error);
            return Err(if blocking {
                TaskRunResult::StopAccount
            } else {
                TaskRunResult::StopTask
            });
        }
    };
    let selected = select_unseen_posts(context.client, &posts, count as usize, used);
    if selected.is_empty() {
        context.report.push(record(
            context.account,
            "米游社任务",
            "帖子列表",
            TaskOutcome::Failed,
            "没有可用于当前任务的新帖子，已停止该任务",
        ));
        return Err(TaskRunResult::StopTask);
    }
    Ok(selected)
}

async fn recheck_task(
    context: &mut BbsTaskContext<'_>,
    task: &str,
    attempt: u32,
) -> Result<CoinSummary, TaskRunResult> {
    tracing::info!(
        account = context.account,
        task,
        attempt,
        max_attempts = context.max_attempts,
        delay_seconds = BBS_CONFIRM_DELAY.as_secs(),
        "社区任务提交完成，等待后复查对应米游币任务"
    );
    tokio::time::sleep(BBS_CONFIRM_DELAY).await;
    match context.client.missions().await {
        Ok(summary) => {
            log_unknown_missions(context.account, &summary, context.logged_unknown_missions);
            Ok(summary)
        }
        Err(error) => {
            let blocking = is_terminal(&error);
            push_error(
                context.report,
                context.account,
                &format!("{task}完成复查"),
                error,
            );
            Err(if blocking {
                TaskRunResult::StopAccount
            } else {
                TaskRunResult::StopTask
            })
        }
    }
}

fn current_task_baseline(context: &BbsTaskContext<'_>) -> Option<CoinSummary> {
    if context.coin_delta_allowed {
        context.latest_summary.as_ref().cloned()
    } else {
        None
    }
}

fn store_rechecked_summary(context: &mut BbsTaskContext<'_>, summary: CoinSummary) {
    *context.latest_summary = Some(summary);
    context.latest_is_post_action = true;
    context.coin_delta_allowed = !context.coin_delta_tainted;
}

const fn flow_stop_result(signal: FlowSignal) -> TaskRunResult {
    match signal {
        FlowSignal::Continue => TaskRunResult::StopTask,
        FlowSignal::BlockHighRisk | FlowSignal::Stop => TaskRunResult::StopAccount,
    }
}

fn task_confirmed(
    before: Option<&CoinSummary>,
    after: &CoinSummary,
    kind: MissionKind,
    idempotent: bool,
) -> bool {
    idempotent
        || after
            .mission(kind)
            .is_some_and(|mission| mission.award_received)
        || before.is_some_and(|before| {
            after.already_received_points > before.already_received_points
                || after.can_get_points < before.can_get_points
        })
}

fn credited_points(before: Option<&CoinSummary>, after: &CoinSummary) -> u32 {
    before.map_or(0, |before| {
        after
            .already_received_points
            .saturating_sub(before.already_received_points)
            .max(before.can_get_points.saturating_sub(after.can_get_points))
    })
}

fn retry_action_count(summary: Option<&CoinSummary>, kind: MissionKind, target: u32) -> u32 {
    summary
        .and_then(|summary| summary.mission(kind))
        .map(|mission| {
            if mission.award_received {
                0
            } else {
                target.saturating_sub(mission.happened_times).max(1)
            }
        })
        .unwrap_or(target)
}

fn log_task_recheck(
    account: &str,
    task: &str,
    attempt: u32,
    before: Option<&CoinSummary>,
    after: &CoinSummary,
) {
    let (received_delta, available_delta) = before.map_or((0, 0), |before| {
        (
            after
                .already_received_points
                .saturating_sub(before.already_received_points),
            before.can_get_points.saturating_sub(after.can_get_points),
        )
    });
    tracing::info!(
        account,
        task,
        attempt,
        baseline_available = before.is_some(),
        received_delta,
        available_delta,
        already_received_points = after.already_received_points,
        can_get_points = after.can_get_points,
        total_points = after.total_points,
        "社区任务完成复查返回实时米游币状态"
    );
}

fn log_task_retry(account: &str, task: &str, attempt: u32, max_attempts: u32) {
    tracing::warn!(
        account,
        task,
        attempt,
        max_attempts,
        "对应米游币任务尚未确认完成，将重新执行该任务"
    );
}

async fn run_reads(
    report: &mut RunReport,
    account: &str,
    client: &BbsClient,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    posts: &[PostRef],
    count: u32,
    completed: &mut Vec<PostRef>,
) -> FlowSignal {
    for post in posts.iter().take(count as usize) {
        let ds = signer.sign_app().to_string();
        match client.read_post(&post.post_id, &ds).await {
            Ok(()) => completed.push(post.clone()),
            Err(BbsError::CaptchaRequired) => {
                push_error(report, account, &post.subject, BbsError::CaptchaRequired);
                return FlowSignal::BlockHighRisk;
            }
            Err(error) => {
                let terminal = is_terminal(&error);
                push_error(report, account, &post.subject, error);
                if terminal {
                    return FlowSignal::Stop;
                }
            }
        }
    }
    FlowSignal::Continue
}

fn select_unseen_posts(
    client: &BbsClient,
    posts: &[PostRef],
    count: usize,
    used: &mut HashSet<String>,
) -> Vec<PostRef> {
    let available = posts
        .iter()
        .filter(|post| !used.contains(&post.post_id))
        .cloned()
        .collect::<Vec<_>>();
    let selected = client.select_posts(&available, count);
    used.extend(selected.iter().map(|post| post.post_id.clone()));
    selected
}

fn push_confirmed_signs(
    report: &mut RunReport,
    account: &str,
    completed: &mut Vec<(String, bool, bool)>,
    attempts: u32,
    mut credited_points: u32,
) {
    let mut seen = HashSet::new();
    let signs = std::mem::take(completed);
    if signs.is_empty() {
        report.push(record(
            account,
            "社区签到",
            "米游币任务",
            TaskOutcome::Success,
            &format!("第 {attempts} 轮复查确认社区签到任务完成"),
        ));
        return;
    }
    let mut latest_signs = Vec::new();
    for sign in signs.into_iter().rev() {
        if seen.insert(sign.0.clone()) {
            latest_signs.push(sign);
        }
    }
    latest_signs.reverse();
    for (forum, captcha_solved, already_signed) in latest_signs {
        let confirmation = if already_signed {
            format!("签到接口确认今日已签到，第 {attempts} 轮复查完成")
        } else {
            format!("第 {attempts} 轮复查确认社区签到米游币领取")
        };
        let captcha_prefix = if captcha_solved {
            "验证码通过后提交签到，"
        } else {
            ""
        };
        let coin_change = credited_points_message(credited_points);
        credited_points = 0;
        report.push(record(
            account,
            "社区签到",
            &forum,
            TaskOutcome::Success,
            &format!("{captcha_prefix}{confirmation}{coin_change}"),
        ));
    }
}

fn push_confirmed_posts(
    report: &mut RunReport,
    account: &str,
    task: &str,
    completed: &mut Vec<PostRef>,
    attempts: u32,
    mut credited_points: u32,
) {
    let mut seen = HashSet::new();
    let posts = std::mem::take(completed);
    if posts.is_empty() {
        report.push(record(
            account,
            task,
            "米游币任务",
            TaskOutcome::Success,
            &format!("第 {attempts} 轮复查确认{task}任务完成"),
        ));
        return;
    }
    for post in posts {
        if !seen.insert(post.post_id.clone()) {
            continue;
        }
        let coin_change = credited_points_message(credited_points);
        credited_points = 0;
        report.push(post_record(
            account,
            task,
            &post,
            TaskOutcome::Success,
            &format!("第 {attempts} 轮复查确认{task}米游币领取{coin_change}"),
        ));
    }
}

fn push_confirmed_likes(
    report: &mut RunReport,
    account: &str,
    completed: &mut Vec<(PostRef, bool)>,
    attempts: u32,
    mut credited_points: u32,
) {
    let mut seen = HashSet::new();
    let likes = std::mem::take(completed);
    if likes.is_empty() {
        report.push(record(
            account,
            "点赞",
            "米游币任务",
            TaskOutcome::Success,
            &format!("第 {attempts} 轮复查确认点赞任务完成"),
        ));
        return;
    }
    for (post, captcha_solved) in likes {
        if !seen.insert(post.post_id.clone()) {
            continue;
        }
        let coin_change = credited_points_message(credited_points);
        credited_points = 0;
        report.push(post_record(
            account,
            "点赞",
            &post,
            TaskOutcome::Success,
            &format!(
                "{}第 {attempts} 轮复查确认点赞米游币领取{coin_change}",
                if captcha_solved {
                    "验证码通过后提交点赞，"
                } else {
                    ""
                }
            ),
        ));
    }
}

fn push_state_sync_timeout(report: &mut RunReport, account: &str, task: &str, attempts: u32) {
    report.push(record(
        account,
        task,
        "米游币任务",
        TaskOutcome::StateSyncTimeout,
        &format!(
            "状态同步超时：已执行 {attempts} 轮并在每轮后等待 3 秒复查，服务端仍未确认{task}任务完成"
        ),
    ));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnmappedMissionClass {
    AuxiliaryCompleted,
    AuxiliaryPending,
    Unknown,
}

fn classify_unmapped_mission(mission_id: i64, award_received: bool) -> UnmappedMissionClass {
    match (mission_id, award_received) {
        (62 | 64, true) => UnmappedMissionClass::AuxiliaryCompleted,
        (62 | 64, false) => UnmappedMissionClass::AuxiliaryPending,
        _ => UnmappedMissionClass::Unknown,
    }
}

fn credited_points_message(points: u32) -> String {
    if points == 0 {
        String::new()
    } else {
        format!("；本轮米游币 +{points}")
    }
}

fn log_unknown_missions(account: &str, summary: &CoinSummary, logged: &mut HashSet<i64>) {
    for mission in &summary.missions {
        let MissionKind::Other(mission_id) = mission.kind else {
            continue;
        };
        if !logged.insert(mission_id) {
            continue;
        }
        match classify_unmapped_mission(mission_id, mission.award_received) {
            UnmappedMissionClass::AuxiliaryCompleted => tracing::debug!(
                account,
                mission_id,
                happened_times = mission.happened_times,
                "固定辅助任务已完成；任务属于绑定角色或修改个性签名，不会自动执行"
            ),
            UnmappedMissionClass::AuxiliaryPending => tracing::warn!(
                account,
                mission_id,
                happened_times = mission.happened_times,
                "固定辅助任务尚未完成；具体对应绑定角色或修改个性签名待确认，禁止自动执行"
            ),
            UnmappedMissionClass::Unknown => tracing::warn!(
                account,
                mission_id,
                award_received = mission.award_received,
                happened_times = mission.happened_times,
                "发现未映射的固定米游币任务，已保留状态但不会自动执行"
            ),
        }
    }
}

fn build_http(config: &Config, account: &AccountConfig) -> Result<HttpClient, &'static str> {
    let builder = HttpClient::builder()
        .timeout(Duration::from_secs(config.runtime.request_timeout_seconds))
        .retry(RetryPolicy {
            attempts: usize::try_from(config.runtime.retry_count).unwrap_or(usize::MAX),
            base_delay: Duration::from_millis(500),
        });
    let builder = builder
        .proxy(account.proxy.url.as_ref().map(|url| url.expose_secret()))
        .map_err(|_| "代理配置无效")?;
    builder.build().map_err(|_| "HTTP 客户端初始化失败")
}

#[derive(Debug)]
enum ActionError {
    Bbs(BbsError),
    Captcha(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ForumSignResult {
    captcha_solved: bool,
    already_signed: bool,
}

async fn sign_forum_with_captcha(
    client: &BbsClient,
    captcha: Option<&CaptchaClient>,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    request: &ForumSignRequest<'_>,
    captcha_max_attempts: u32,
) -> Result<ForumSignResult, ActionError> {
    let body = serde_json::to_vec(request).expect("固定社区签到结构应当可序列化");
    let ds = signer.sign_body("", &body).to_string();
    match client.sign_forum_once(request, &ds, None).await {
        Ok(state) => Ok(ForumSignResult {
            captcha_solved: false,
            already_signed: state == ForumSignState::AlreadySigned,
        }),
        Err(BbsError::CaptchaRequired) => {
            let challenge =
                solve_bbs_captcha(client, captcha, signer, captcha_max_attempts).await?;
            let ds = signer.sign_body("", &body).to_string();
            match client.sign_forum_once(request, &ds, Some(&challenge)).await {
                Ok(state) => Ok(ForumSignResult {
                    captcha_solved: true,
                    already_signed: state == ForumSignState::AlreadySigned,
                }),
                Err(BbsError::CaptchaRequired) => Err(ActionError::Captcha(
                    "验证码校验通过，但原签到动作重试后仍要求验证码".to_owned(),
                )),
                Err(error) => Err(ActionError::Bbs(error)),
            }
        }
        Err(error) => Err(ActionError::Bbs(error)),
    }
}

async fn set_like_with_captcha(
    client: &BbsClient,
    captcha: Option<&CaptchaClient>,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    post_id: &str,
    cancel: bool,
    captcha_max_attempts: u32,
) -> Result<bool, ActionError> {
    let ds = signer.sign_app().to_string();
    match client.set_like_once(post_id, cancel, &ds, None).await {
        Ok(()) => Ok(false),
        Err(BbsError::CaptchaRequired) => {
            let challenge =
                solve_bbs_captcha(client, captcha, signer, captcha_max_attempts).await?;
            let ds = signer.sign_app().to_string();
            match client
                .set_like_once(post_id, cancel, &ds, Some(&challenge))
                .await
            {
                Ok(()) => Ok(true),
                Err(BbsError::CaptchaRequired) => {
                    let action = if cancel { "取消点赞" } else { "点赞" };
                    Err(ActionError::Captcha(format!(
                        "验证码校验通过，但原{action}动作重试后仍要求验证码"
                    )))
                }
                Err(error) => Err(ActionError::Bbs(error)),
            }
        }
        Err(error) => Err(ActionError::Bbs(error)),
    }
}

async fn solve_bbs_captcha(
    client: &BbsClient,
    captcha: Option<&CaptchaClient>,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    max_attempts: u32,
) -> Result<String, ActionError> {
    let captcha = captcha
        .ok_or_else(|| ActionError::Captcha("触发验证码，但未配置 captcha.endpoint".to_owned()))?;
    let max_attempts = max_attempts.max(1);
    let mut last_error = None;
    for attempt in 1..=max_attempts {
        let ds = signer.sign_app().to_string();
        let verification = client
            .create_verification(&ds)
            .await
            .map_err(|error| ActionError::Captcha(format!("创建米游社验证码失败：{error}")))?;
        match captcha
            .solve(&verification.gt, &verification.challenge)
            .await
        {
            Ok(solution) => {
                let ds = signer.sign_app().to_string();
                return client
                    .verify_verification(&solution.challenge, &solution.validate, &ds)
                    .await
                    .map_err(|error| {
                        ActionError::Captcha(format!("米游社验证码校验失败：{error}"))
                    });
            }
            Err(error) => {
                let reason = error.safe_reason();
                last_error = Some(reason);
                if attempt < max_attempts {
                    tracing::warn!(
                        attempt,
                        max_attempts,
                        delay_seconds = CAPTCHA_RETRY_DELAY.as_secs(),
                        reason,
                        "验证码平台返回异常，等待后重新生成验证码"
                    );
                    tokio::time::sleep(CAPTCHA_RETRY_DELAY).await;
                }
            }
        }
    }
    Err(ActionError::Captcha(format!(
        "验证码平台连续 {max_attempts} 次求解失败：{}",
        last_error.unwrap_or("未知错误")
    )))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BbsPlan {
    sign: bool,
    read: u32,
    like: u32,
    share: bool,
}

impl BbsPlan {
    fn from_config(tasks: &BbsTaskConfig) -> Self {
        if !tasks.enabled {
            return Self::default();
        }
        Self {
            sign: tasks.sign,
            read: if tasks.read { READ_TARGET } else { 0 },
            like: if tasks.like { LIKE_TARGET } else { 0 },
            share: tasks.share,
        }
    }
}

fn is_terminal(error: &BbsError) -> bool {
    matches!(error, BbsError::AuthExpired)
}

fn is_account_blocking(error: &BbsError) -> bool {
    matches!(error, BbsError::AuthExpired | BbsError::CaptchaRequired)
}

const fn bbs_error_kind(error: &BbsError) -> &'static str {
    match error {
        BbsError::Http(_) => "http",
        BbsError::InvalidHeader(_) => "invalid_header",
        BbsError::InvalidResponse(_) => "invalid_response",
        BbsError::AuthExpired => "auth_expired",
        BbsError::CaptchaRequired => "captcha_required",
        BbsError::Api { .. } => "api",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlowSignal {
    Continue,
    BlockHighRisk,
    Stop,
}

fn push_error(report: &mut RunReport, account: &str, subject: &str, error: BbsError) {
    let (outcome, message) = match error {
        BbsError::AuthExpired => (
            TaskOutcome::AuthenticationFailed,
            "Cookie 或 SToken 无效".to_owned(),
        ),
        BbsError::CaptchaRequired => (
            TaskOutcome::CaptchaRequired,
            "触发验证码，已停止后续高风险动作".to_owned(),
        ),
        BbsError::Http(_) => (TaskOutcome::NetworkFailed, "网络请求失败".to_owned()),
        other => (TaskOutcome::Failed, other.to_string()),
    };
    report.push(record(account, "米游社任务", subject, outcome, &message));
}

fn push_captcha_error(report: &mut RunReport, account: &str, subject: &str, message: &str) {
    report.push(record(
        account,
        "米游社任务",
        subject,
        TaskOutcome::CaptchaRequired,
        message,
    ));
}

fn post_record(
    account: &str,
    task: &str,
    post: &PostRef,
    outcome: TaskOutcome,
    message: &str,
) -> TaskRecord {
    record(account, task, &post.subject, outcome, message)
}

fn record(
    account: &str,
    task: &str,
    subject: &str,
    outcome: TaskOutcome,
    message: &str,
) -> TaskRecord {
    TaskRecord {
        account: account.to_owned(),
        task: task.to_owned(),
        subject: subject.to_owned(),
        outcome,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::{
        bbs::{BbsEndpoints, MissionProgress},
        captcha::CaptchaError,
        http::HttpError,
        signing::{BODY_SALT, sign_ds2_with},
    };
    use reqwest::Url;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, Request, ResponseTemplate,
        matchers::{body_json, method, path, query_param},
    };

    fn summary(can_get: u32, missions: Vec<MissionProgress>) -> CoinSummary {
        summary_with_points(can_get, 0, 100, missions)
    }

    fn summary_with_points(
        can_get: u32,
        already_received: u32,
        total: u32,
        missions: Vec<MissionProgress>,
    ) -> CoinSummary {
        CoinSummary {
            can_get_points: can_get,
            already_received_points: already_received,
            total_points: total,
            missions,
        }
    }

    fn mission(kind: MissionKind, done: bool, happened: u32) -> MissionProgress {
        MissionProgress {
            kind,
            award_received: done,
            happened_times: happened,
        }
    }

    fn bbs_tasks(sign: bool, read: bool, like: bool, share: bool) -> BbsTaskConfig {
        BbsTaskConfig {
            enabled: true,
            sign,
            forums: vec![2],
            read,
            like,
            cancel_like: false,
            share,
        }
    }

    #[test]
    fn first_plan_is_created_only_from_each_accounts_configuration() {
        assert_eq!(
            BbsPlan::from_config(&bbs_tasks(true, false, true, false)),
            BbsPlan {
                sign: true,
                read: 0,
                like: LIKE_TARGET,
                share: false,
            }
        );
        assert_eq!(
            BbsPlan::from_config(&bbs_tasks(false, true, false, true)),
            BbsPlan {
                sign: false,
                read: READ_TARGET,
                like: 0,
                share: true,
            }
        );
        let mut disabled = bbs_tasks(true, true, true, true);
        disabled.enabled = false;
        assert_eq!(BbsPlan::from_config(&disabled), BbsPlan::default());
    }

    #[test]
    fn fixed_auxiliary_missions_are_classified_without_entering_any_plan() {
        assert_eq!(
            classify_unmapped_mission(62, true),
            UnmappedMissionClass::AuxiliaryCompleted
        );
        assert_eq!(
            classify_unmapped_mission(64, false),
            UnmappedMissionClass::AuxiliaryPending
        );
        assert_eq!(
            classify_unmapped_mission(999, true),
            UnmappedMissionClass::Unknown
        );

        let summary = summary(
            30,
            vec![
                mission(MissionKind::Other(62), true, 1),
                mission(MissionKind::Other(64), true, 1),
            ],
        );
        let mut logged = HashSet::new();
        log_unknown_missions("测试账号", &summary, &mut logged);
        log_unknown_missions("测试账号", &summary, &mut logged);
        assert_eq!(logged, HashSet::from([62, 64]));
    }

    #[test]
    fn confirmation_uses_only_the_current_task_and_positive_coin_delta() {
        let before =
            summary_with_points(30, 0, 4219, vec![mission(MissionKind::Other(62), true, 1)]);
        let unchanged =
            summary_with_points(30, 0, 4219, vec![mission(MissionKind::Other(64), true, 1)]);
        assert!(!task_confirmed(
            Some(&before),
            &unchanged,
            MissionKind::Sign,
            false
        ));

        let received = summary_with_points(0, 30, 4249, Vec::new());
        assert!(task_confirmed(
            Some(&before),
            &received,
            MissionKind::Sign,
            false
        ));
        assert!(task_confirmed(
            None,
            &summary(30, vec![mission(MissionKind::Sign, true, 1)]),
            MissionKind::Sign,
            false
        ));
        assert!(task_confirmed(
            None,
            &summary(30, Vec::new()),
            MissionKind::Sign,
            true
        ));
    }

    #[test]
    fn later_rounds_use_rechecked_progress_without_becoming_zero_action_rounds() {
        assert_eq!(
            retry_action_count(
                Some(&summary(30, vec![mission(MissionKind::Read, false, 2)])),
                MissionKind::Read,
                READ_TARGET
            ),
            1
        );
        assert_eq!(
            retry_action_count(
                Some(&summary(
                    30,
                    vec![mission(MissionKind::Like, false, LIKE_TARGET)]
                )),
                MissionKind::Like,
                LIKE_TARGET
            ),
            1
        );
        assert_eq!(
            retry_action_count(None, MissionKind::Like, LIKE_TARGET),
            LIKE_TARGET
        );
    }

    fn test_bbs_client(server: &MockServer) -> BbsClient {
        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        let base = Url::parse(&format!("{}/", server.uri())).unwrap();
        BbsClient::new(
            http,
            SecretString::new("stuid=123;stoken=secret"),
            SecretString::new("cookie_token=secret"),
            "device-id",
        )
        .endpoints(BbsEndpoints::from_base_url(&base).unwrap())
    }

    #[tokio::test]
    async fn missing_sign_mission_and_completed_auxiliary_tasks_still_run_first_sign() {
        let server = MockServer::start().await;
        let mission_calls = Arc::new(AtomicUsize::new(0));
        let responder_calls = Arc::clone(&mission_calls);
        Mock::given(method("GET"))
            .and(path("/apihub/wapi/getUserMissionsState"))
            .respond_with(move |_request: &Request| {
                let call = responder_calls.fetch_add(1, Ordering::SeqCst);
                let (can_get, received, total, states) = if call == 0 {
                    (
                        30,
                        0,
                        4219,
                        json!([
                            {"mission_id": 62, "is_get_award": true, "happened_times": 1},
                            {"mission_id": 64, "is_get_award": true, "happened_times": 1}
                        ]),
                    )
                } else {
                    (
                        0,
                        30,
                        4249,
                        json!([
                            {"mission_id": 58, "is_get_award": true, "happened_times": 1},
                            {"mission_id": 62, "is_get_award": true, "happened_times": 1},
                            {"mission_id": 64, "is_get_award": true, "happened_times": 1}
                        ]),
                    )
                };
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": {
                        "can_get_points": can_get,
                        "already_received_points": received,
                        "total_points": total,
                        "states": states
                    }
                }))
            })
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/apihub/app/api/signIn"))
            .and(body_json(json!({"gids": "2"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0, "message": "OK", "data": {}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_bbs_client(&server);
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);
        let mut report = RunReport::default();
        run_account_tasks(
            &mut report,
            "测试账号",
            &bbs_tasks(true, false, false, false),
            &client,
            None,
            &mut signer,
            3,
        )
        .await;

        assert_eq!(mission_calls.load(Ordering::SeqCst), 2);
        let final_summary = report
            .records
            .iter()
            .find(|record| record.task == "米游币" && record.subject == "完成确认")
            .unwrap();
        assert_eq!(
            final_summary.message,
            "已领取 30，还可领取 0，当前共 4249 米游币"
        );
        assert_eq!(report.exit_code(), 0);
    }

    #[tokio::test]
    async fn unchanged_missing_sign_mission_retries_only_after_successful_recheck() {
        let server = MockServer::start().await;
        let mission_calls = Arc::new(AtomicUsize::new(0));
        let responder_calls = Arc::clone(&mission_calls);
        Mock::given(method("GET"))
            .and(path("/apihub/wapi/getUserMissionsState"))
            .respond_with(move |_request: &Request| {
                let call = responder_calls.fetch_add(1, Ordering::SeqCst);
                let states = if call < 2 {
                    json!([{"mission_id": 62, "is_get_award": true, "happened_times": 1}])
                } else {
                    json!([{"mission_id": 58, "is_get_award": true, "happened_times": 1}])
                };
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": {
                        "can_get_points": 30,
                        "already_received_points": 0,
                        "total_points": 4219,
                        "states": states
                    }
                }))
            })
            .expect(3)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/apihub/app/api/signIn"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0, "message": "OK", "data": {}
            })))
            .expect(2)
            .mount(&server)
            .await;

        let client = test_bbs_client(&server);
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);
        let mut report = RunReport::default();
        run_account_tasks(
            &mut report,
            "测试账号",
            &bbs_tasks(true, false, false, false),
            &client,
            None,
            &mut signer,
            3,
        )
        .await;

        assert_eq!(mission_calls.load(Ordering::SeqCst), 3);
        assert!(
            report
                .records
                .iter()
                .any(|record| record.task == "社区签到" && record.message.contains("第 2 轮"))
        );
        assert_eq!(report.exit_code(), 0);
    }

    #[tokio::test]
    async fn unchanged_sign_state_reaches_limit_and_keeps_latest_realtime_summary() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apihub/wapi/getUserMissionsState"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {
                    "can_get_points": 30,
                    "already_received_points": 0,
                    "total_points": 4219,
                    "states": [
                        {"mission_id": 62, "is_get_award": true, "happened_times": 1}
                    ]
                }
            })))
            .expect(3)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/apihub/app/api/signIn"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0, "message": "OK", "data": {}
            })))
            .expect(2)
            .mount(&server)
            .await;

        let client = test_bbs_client(&server);
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);
        let mut report = RunReport::default();
        run_account_tasks(
            &mut report,
            "测试账号",
            &bbs_tasks(true, false, false, false),
            &client,
            None,
            &mut signer,
            2,
        )
        .await;

        assert!(report.records.iter().any(|record| {
            record.task == "社区签到" && record.outcome == TaskOutcome::StateSyncTimeout
        }));
        let realtime = report
            .records
            .iter()
            .find(|record| record.task == "米游币" && record.subject == "实时汇总")
            .unwrap();
        assert_eq!(
            realtime.message,
            "已领取 0，还可领取 30，当前共 4219 米游币"
        );
    }

    #[tokio::test]
    async fn failed_confirmation_query_stops_sign_without_blind_retry() {
        let server = MockServer::start().await;
        let mission_calls = Arc::new(AtomicUsize::new(0));
        let responder_calls = Arc::clone(&mission_calls);
        Mock::given(method("GET"))
            .and(path("/apihub/wapi/getUserMissionsState"))
            .respond_with(move |_request: &Request| {
                if responder_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(200).set_body_json(json!({
                        "retcode": 0,
                        "message": "OK",
                        "data": {
                            "can_get_points": 30,
                            "already_received_points": 0,
                            "total_points": 4219,
                            "states": []
                        }
                    }))
                } else {
                    ResponseTemplate::new(500)
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/apihub/app/api/signIn"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0, "message": "OK", "data": {}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_bbs_client(&server);
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);
        let mut report = RunReport::default();
        run_account_tasks(
            &mut report,
            "测试账号",
            &bbs_tasks(true, false, false, false),
            &client,
            None,
            &mut signer,
            3,
        )
        .await;

        assert_eq!(mission_calls.load(Ordering::SeqCst), 2);
        assert!(
            report
                .records
                .iter()
                .any(|record| record.outcome == TaskOutcome::NetworkFailed)
        );
        assert!(
            report
                .records
                .iter()
                .all(|record| record.outcome != TaskOutcome::StateSyncTimeout)
        );
    }

    #[test]
    fn forum_sign_ds2_uses_the_exact_serialized_request_body() {
        let request = ForumSignRequest::new("2");
        let body = serde_json::to_vec(&request).unwrap();
        assert_eq!(body, br#"{"gids":"2"}"#);

        let header = sign_ds2_with(BODY_SALT, 1_700_000_000, 123_456, "", &body);
        let independently_serialized = serde_json::to_vec(&request).unwrap();
        assert_eq!(body, independently_serialized);
        assert_eq!(header.checksum, "b4ec3312d474ed70fcfcb5e6f25b04a4");
    }

    #[tokio::test]
    async fn bbs_captcha_flow_creates_solves_and_verifies_challenge() {
        let bbs_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/misc/api/createVerification"))
            .and(query_param("is_high", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {"gt": "gt-value", "challenge": "original-challenge"}
            })))
            .expect(1)
            .mount(&bbs_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/misc/api/verifyVerification"))
            .and(body_json(json!({
                "geetest_challenge": "solver-challenge",
                "geetest_seccode": "validate-value|jordan",
                "geetest_validate": "validate-value"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {"challenge": "passed-challenge"}
            })))
            .expect(1)
            .mount(&bbs_server)
            .await;

        let solver_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/pass_nine"))
            .and(query_param("gt", "gt-value"))
            .and(query_param("challenge", "original-challenge"))
            .and(query_param("use_v3_model", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {
                    "result": "success",
                    "validate": "validate-value",
                    "challenge": "solver-challenge"
                }
            })))
            .expect(1)
            .mount(&solver_server)
            .await;

        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        let bbs_base = Url::parse(&format!("{}/", bbs_server.uri())).unwrap();
        let client = BbsClient::new(
            http.clone(),
            SecretString::new("stuid=123;stoken=secret"),
            SecretString::new("cookie_token=secret"),
            "device-id",
        )
        .endpoints(BbsEndpoints::from_base_url(&bbs_base).unwrap());
        let captcha = CaptchaClient::new(
            http,
            Url::parse(&format!("{}/pass_nine", solver_server.uri())).unwrap(),
        );
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);

        assert_eq!(
            solve_bbs_captcha(&client, Some(&captcha), &mut signer, 1)
                .await
                .unwrap(),
            "passed-challenge"
        );
    }

    #[tokio::test]
    async fn bbs_captcha_failure_recreates_challenge_before_retrying() {
        let bbs_server = MockServer::start().await;
        let create_count = Arc::new(AtomicUsize::new(0));
        let create_responder_count = Arc::clone(&create_count);
        Mock::given(method("GET"))
            .and(path("/misc/api/createVerification"))
            .respond_with(move |_request: &Request| {
                let attempt = create_responder_count.fetch_add(1, Ordering::SeqCst) + 1;
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": {
                        "gt": format!("gt-{attempt}"),
                        "challenge": format!("challenge-{attempt}")
                    }
                }))
            })
            .expect(2)
            .mount(&bbs_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/misc/api/verifyVerification"))
            .and(body_json(json!({
                "geetest_challenge": "solver-challenge-2",
                "geetest_seccode": "validate-2|jordan",
                "geetest_validate": "validate-2"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {"challenge": "passed-after-retry"}
            })))
            .expect(1)
            .mount(&bbs_server)
            .await;

        let solver_server = MockServer::start().await;
        let solve_count = Arc::new(AtomicUsize::new(0));
        let solve_responder_count = Arc::clone(&solve_count);
        Mock::given(method("GET"))
            .and(path("/pass_nine"))
            .respond_with(move |_request: &Request| {
                let attempt = solve_responder_count.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt == 1 {
                    ResponseTemplate::new(200).set_body_json(json!({"data": {"result": "fail"}}))
                } else {
                    ResponseTemplate::new(200).set_body_json(json!({
                        "data": {
                            "result": "success",
                            "validate": "validate-2",
                            "challenge": "solver-challenge-2"
                        }
                    }))
                }
            })
            .expect(2)
            .mount(&solver_server)
            .await;

        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        let base = Url::parse(&format!("{}/", bbs_server.uri())).unwrap();
        let client = BbsClient::new(
            http.clone(),
            SecretString::new("stuid=123;stoken=secret"),
            SecretString::new("cookie_token=secret"),
            "device-id",
        )
        .endpoints(BbsEndpoints::from_base_url(&base).unwrap());
        let captcha = CaptchaClient::new(
            http,
            Url::parse(&format!("{}/pass_nine", solver_server.uri())).unwrap(),
        );
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);

        assert_eq!(
            solve_bbs_captcha(&client, Some(&captcha), &mut signer, 2)
                .await
                .unwrap(),
            "passed-after-retry"
        );
        assert_eq!(create_count.load(Ordering::SeqCst), 2);
        assert_eq!(solve_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn missing_captcha_endpoint_is_reported_without_network_attempt() {
        let server = MockServer::start().await;
        let http = HttpClient::builder().build().unwrap();
        let base = Url::parse(&format!("{}/", server.uri())).unwrap();
        let client = BbsClient::new(
            http,
            SecretString::new("stuid=123;stoken=secret"),
            SecretString::new("cookie_token=secret"),
            "device-id",
        )
        .endpoints(BbsEndpoints::from_base_url(&base).unwrap());
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);

        assert!(matches!(
            solve_bbs_captcha(&client, None, &mut signer, 3).await,
            Err(ActionError::Captcha(message)) if message.contains("未配置 captcha.endpoint")
        ));
    }

    #[test]
    fn captcha_error_reason_never_contains_solver_query_details() {
        let error = CaptchaError::Http(HttpError::Connect(
            "https://solver.example/pass?gt=secret&challenge=secret".to_owned(),
        ));

        assert_eq!(error.safe_reason(), "无法连接验证码平台");
        assert!(!error.safe_reason().contains("secret"));
    }
}
