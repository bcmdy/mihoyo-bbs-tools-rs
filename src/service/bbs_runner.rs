use std::{collections::HashSet, path::Path, time::Duration};

use crate::{
    auth::{Credentials, SecretString},
    bbs::{BbsClient, BbsError, CoinSummary, ForumSignRequest, MissionKind, PostRef, forum_by_id},
    captcha::CaptchaClient,
    config::{AccountConfig, Config},
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

#[derive(Default)]
struct CompletedRequests {
    forum_signs: Vec<(String, bool)>,
    reads: Vec<PostRef>,
    likes: Vec<(PostRef, bool)>,
    shares: Vec<PostRef>,
}

#[derive(Clone, Copy, Debug, Default)]
struct TaskAttempts {
    sign: u32,
    read: u32,
    like: u32,
    share: u32,
}

impl TaskAttempts {
    fn record(&mut self, plan: BbsPlan) {
        self.sign += if plan.sign { 1 } else { 0 };
        self.read += if plan.read > 0 { 1 } else { 0 };
        self.like += if plan.like > 0 { 1 } else { 0 };
        self.share += if plan.share { 1 } else { 0 };
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct StoppedTasks {
    sign: bool,
    read: bool,
    like: bool,
    share: bool,
}

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

    let mut summary = match client.missions().await {
        Ok(summary) => summary,
        Err(error) => {
            push_error(report, &account.name, "任务状态", error);
            return;
        }
    };
    let plan = BbsPlan::from_summary(&summary).filtered(&account.tasks.bbs);
    report.push(record(
        &account.name,
        "米游币",
        "任务状态",
        if plan.is_complete() {
            TaskOutcome::AlreadyCompleted
        } else {
            TaskOutcome::Success
        },
        &format!(
            "已领取 {}，还可领取 {}，当前共 {} 米游币",
            summary.already_received_points, summary.can_get_points, summary.total_points
        ),
    ));
    if plan.is_complete() {
        return;
    }

    let forums = account
        .tasks
        .bbs
        .forums
        .iter()
        .filter_map(|id| forum_by_id(*id))
        .collect::<Vec<_>>();
    if forums.is_empty() {
        report.push(record(
            &account.name,
            "米游社任务",
            "社区板块",
            TaskOutcome::Failed,
            "没有可用的社区板块，请检查 tasks.bbs.forums",
        ));
        return;
    }

    let max_attempts = config.runtime.task_max_attempts.max(1);
    let mut completed_requests = CompletedRequests::default();
    let mut attempts = TaskAttempts::default();
    let mut stopped = StoppedTasks::default();
    let mut used_reads = HashSet::new();
    let mut used_likes = HashSet::new();
    let mut used_shares = HashSet::new();
    let mut timed_out = false;
    let mut high_risk_blocked = false;

    loop {
        let plan = BbsPlan::from_summary(&summary)
            .filtered(&account.tasks.bbs)
            .without_stopped(stopped);
        if plan.is_complete() {
            break;
        }
        let mut stop_after_confirmation = false;
        let mut attempted = BbsPlan::default();

        if plan.sign {
            attempted.sign = true;
            for forum in &forums {
                let request = ForumSignRequest::new(forum.gids);
                match sign_forum_with_captcha(
                    &client,
                    captcha.as_ref(),
                    &mut signer,
                    &request,
                    max_attempts,
                )
                .await
                {
                    Ok(solved) => completed_requests
                        .forum_signs
                        .push((forum.name.to_owned(), solved)),
                    Err(ActionError::Captcha(message)) => {
                        push_captcha_error(report, &account.name, forum.name, &message);
                        high_risk_blocked = true;
                        break;
                    }
                    Err(ActionError::Bbs(error)) => {
                        let terminal = is_terminal(&error);
                        push_error(report, &account.name, forum.name, error);
                        if terminal {
                            stop_after_confirmation = true;
                            break;
                        }
                    }
                }
            }
        }

        let post_pool =
            if !high_risk_blocked && !stop_after_confirmation && plan.required_posts() > 0 {
                let ds = signer.sign_app().to_string();
                match client.posts(forums[0].forum_id, 20, &ds).await {
                    Ok(posts) => posts,
                    Err(error) => {
                        let terminal = is_terminal(&error);
                        push_error(report, &account.name, "帖子列表", error);
                        if terminal {
                            stop_after_confirmation = true;
                        }
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };

        if !high_risk_blocked && !stop_after_confirmation && plan.read > 0 {
            attempted.read = plan.read;
            let selected =
                select_unseen_posts(&client, &post_pool, plan.read as usize, &mut used_reads);
            match run_reads(
                report,
                &account.name,
                &client,
                &mut signer,
                &selected,
                plan.read,
                &mut completed_requests.reads,
            )
            .await
            {
                FlowSignal::Continue => {}
                FlowSignal::BlockHighRisk => high_risk_blocked = true,
                FlowSignal::Stop => stop_after_confirmation = true,
            }
        }

        if !high_risk_blocked && !stop_after_confirmation && plan.like > 0 {
            attempted.like = plan.like;
            let selected =
                select_unseen_posts(&client, &post_pool, plan.like as usize, &mut used_likes);
            for post in &selected {
                match set_like_with_captcha(
                    &client,
                    captcha.as_ref(),
                    &mut signer,
                    &post.post_id,
                    false,
                    max_attempts,
                )
                .await
                {
                    Ok(solved) => completed_requests.likes.push((post.clone(), solved)),
                    Err(ActionError::Captcha(message)) => {
                        push_captcha_error(report, &account.name, &post.subject, &message);
                        high_risk_blocked = true;
                        break;
                    }
                    Err(ActionError::Bbs(error)) => {
                        let terminal = is_terminal(&error);
                        push_error(report, &account.name, &post.subject, error);
                        if terminal {
                            stop_after_confirmation = true;
                            break;
                        }
                        continue;
                    }
                }

                if !account.tasks.bbs.cancel_like {
                    continue;
                }
                match set_like_with_captcha(
                    &client,
                    captcha.as_ref(),
                    &mut signer,
                    &post.post_id,
                    true,
                    max_attempts,
                )
                .await
                {
                    Ok(solved) => report.push(post_record(
                        &account.name,
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
                        push_captcha_error(report, &account.name, &post.subject, &message);
                        high_risk_blocked = true;
                        break;
                    }
                    Err(ActionError::Bbs(error)) => {
                        let terminal = is_terminal(&error);
                        push_error(report, &account.name, &post.subject, error);
                        if terminal {
                            stop_after_confirmation = true;
                            break;
                        }
                    }
                }
            }
        }

        if !high_risk_blocked && !stop_after_confirmation && plan.share {
            attempted.share = true;
            let selected = select_unseen_posts(&client, &post_pool, 1, &mut used_shares);
            if let Some(post) = selected.first() {
                let ds = signer.sign_app().to_string();
                match client.share_post(&post.post_id, &ds).await {
                    Ok(()) => completed_requests.shares.push(post.clone()),
                    Err(error) => {
                        let terminal = is_terminal(&error);
                        push_error(report, &account.name, &post.subject, error);
                        if terminal {
                            stop_after_confirmation = true;
                        }
                    }
                }
            }
        }

        attempts.record(attempted);
        tokio::time::sleep(BBS_CONFIRM_DELAY).await;
        summary = match client.missions().await {
            Ok(summary) => summary,
            Err(error) => {
                push_error(report, &account.name, "任务完成复查", error);
                return;
            }
        };
        timed_out |= reconcile_task_results(
            report,
            &account.name,
            attempted,
            &mut completed_requests,
            &summary,
            attempts,
            max_attempts,
            &mut stopped,
        );

        if high_risk_blocked {
            let pending = BbsPlan::from_summary(&summary).filtered(&account.tasks.bbs);
            push_blocked_actions(report, &account.name, &pending);
            break;
        }
        if stop_after_confirmation {
            break;
        }
    }

    let remaining = BbsPlan::from_summary(&summary).filtered(&account.tasks.bbs);
    let all_confirmed = remaining.is_complete() && !timed_out && !high_risk_blocked;
    if all_confirmed {
        report.push(record(
            &account.name,
            "米游币",
            "完成确认",
            TaskOutcome::Success,
            &format!(
                "复查后已领取 {}，还可领取 {}，当前共 {} 米游币",
                summary.already_received_points, summary.can_get_points, summary.total_points
            ),
        ));
    }
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

#[allow(clippy::too_many_arguments)]
fn reconcile_task_results(
    report: &mut RunReport,
    account: &str,
    requested: BbsPlan,
    completed: &mut CompletedRequests,
    summary: &CoinSummary,
    attempts: TaskAttempts,
    max_attempts: u32,
    stopped: &mut StoppedTasks,
) -> bool {
    let mut timed_out = false;

    if requested.sign {
        match mission_status(summary, MissionKind::Sign) {
            MissionStatus::Completed => {
                push_confirmed_signs(report, account, completed, attempts.sign);
                stopped.sign = true;
            }
            MissionStatus::Pending if attempts.sign >= max_attempts => {
                completed.forum_signs.clear();
                push_state_sync_timeout(report, account, "社区签到", attempts.sign);
                stopped.sign = true;
                timed_out = true;
            }
            MissionStatus::Missing => {
                completed.forum_signs.clear();
                push_missing_mission(report, account, "社区签到");
                stopped.sign = true;
            }
            MissionStatus::Pending => {}
        }
    }

    if requested.read > 0 {
        match mission_status(summary, MissionKind::Read) {
            MissionStatus::Completed => {
                push_confirmed_posts(report, account, "阅读", &mut completed.reads, attempts.read);
                stopped.read = true;
            }
            MissionStatus::Pending if attempts.read >= max_attempts => {
                completed.reads.clear();
                push_state_sync_timeout(report, account, "阅读", attempts.read);
                stopped.read = true;
                timed_out = true;
            }
            MissionStatus::Missing => {
                completed.reads.clear();
                push_missing_mission(report, account, "阅读");
                stopped.read = true;
            }
            MissionStatus::Pending => {}
        }
    }

    if requested.like > 0 {
        match mission_status(summary, MissionKind::Like) {
            MissionStatus::Completed => {
                push_confirmed_likes(report, account, completed, attempts.like);
                stopped.like = true;
            }
            MissionStatus::Pending if attempts.like >= max_attempts => {
                completed.likes.clear();
                push_state_sync_timeout(report, account, "点赞", attempts.like);
                stopped.like = true;
                timed_out = true;
            }
            MissionStatus::Missing => {
                completed.likes.clear();
                push_missing_mission(report, account, "点赞");
                stopped.like = true;
            }
            MissionStatus::Pending => {}
        }
    }

    if requested.share {
        match mission_status(summary, MissionKind::Share) {
            MissionStatus::Completed => {
                push_confirmed_posts(
                    report,
                    account,
                    "分享",
                    &mut completed.shares,
                    attempts.share,
                );
                stopped.share = true;
            }
            MissionStatus::Pending if attempts.share >= max_attempts => {
                completed.shares.clear();
                push_state_sync_timeout(report, account, "分享", attempts.share);
                stopped.share = true;
                timed_out = true;
            }
            MissionStatus::Missing => {
                completed.shares.clear();
                push_missing_mission(report, account, "分享");
                stopped.share = true;
            }
            MissionStatus::Pending => {}
        }
    }

    timed_out
}

fn push_confirmed_signs(
    report: &mut RunReport,
    account: &str,
    completed: &mut CompletedRequests,
    attempts: u32,
) {
    let mut seen = HashSet::new();
    let signs = std::mem::take(&mut completed.forum_signs);
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
    for (forum, captcha_solved) in signs {
        if !seen.insert(forum.clone()) {
            continue;
        }
        report.push(record(
            account,
            "社区签到",
            &forum,
            TaskOutcome::Success,
            &format!(
                "{}第 {attempts} 轮复查确认社区签到米游币领取",
                if captcha_solved {
                    "验证码通过后提交签到，"
                } else {
                    ""
                }
            ),
        ));
    }
}

fn push_confirmed_posts(
    report: &mut RunReport,
    account: &str,
    task: &str,
    completed: &mut Vec<PostRef>,
    attempts: u32,
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
        report.push(post_record(
            account,
            task,
            &post,
            TaskOutcome::Success,
            &format!("第 {attempts} 轮复查确认{task}米游币领取"),
        ));
    }
}

fn push_confirmed_likes(
    report: &mut RunReport,
    account: &str,
    completed: &mut CompletedRequests,
    attempts: u32,
) {
    let mut seen = HashSet::new();
    let likes = std::mem::take(&mut completed.likes);
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
        report.push(post_record(
            account,
            "点赞",
            &post,
            TaskOutcome::Success,
            &format!(
                "{}第 {attempts} 轮复查确认点赞米游币领取",
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

fn push_missing_mission(report: &mut RunReport, account: &str, task: &str) {
    report.push(record(
        account,
        task,
        "米游币任务",
        TaskOutcome::Failed,
        "任务状态响应缺少对应任务，状态未知，已停止重试",
    ));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MissionStatus {
    Completed,
    Pending,
    Missing,
}

fn mission_status(summary: &CoinSummary, kind: MissionKind) -> MissionStatus {
    if summary.can_get_points == 0 {
        return MissionStatus::Completed;
    }
    match summary.mission(kind) {
        Some(mission) if mission.award_received => MissionStatus::Completed,
        Some(_) => MissionStatus::Pending,
        None => MissionStatus::Missing,
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

async fn sign_forum_with_captcha(
    client: &BbsClient,
    captcha: Option<&CaptchaClient>,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    request: &ForumSignRequest<'_>,
    captcha_max_attempts: u32,
) -> Result<bool, ActionError> {
    let body = serde_json::to_vec(request).expect("固定社区签到结构应当可序列化");
    let ds = signer.sign_body("", &body).to_string();
    match client.sign_forum_once(request, &ds, None).await {
        Ok(()) => Ok(false),
        Err(BbsError::CaptchaRequired) => {
            let challenge =
                solve_bbs_captcha(client, captcha, signer, captcha_max_attempts).await?;
            let ds = signer.sign_body("", &body).to_string();
            match client.sign_forum_once(request, &ds, Some(&challenge)).await {
                Ok(()) => Ok(true),
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
                last_error = Some(error.to_string());
                if attempt < max_attempts {
                    tracing::warn!(
                        attempt,
                        max_attempts,
                        delay_seconds = CAPTCHA_RETRY_DELAY.as_secs(),
                        error = %error,
                        "验证码平台返回异常，等待后重新生成验证码"
                    );
                    tokio::time::sleep(CAPTCHA_RETRY_DELAY).await;
                }
            }
        }
    }
    Err(ActionError::Captcha(format!(
        "验证码平台连续 {max_attempts} 次求解失败：{}",
        last_error.unwrap_or_else(|| "未知错误".to_owned())
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
    fn from_summary(summary: &CoinSummary) -> Self {
        if summary.can_get_points == 0 {
            return Self::default();
        }
        Self {
            sign: pending_once(summary, MissionKind::Sign),
            read: remaining(summary, MissionKind::Read, READ_TARGET),
            like: remaining(summary, MissionKind::Like, LIKE_TARGET),
            share: pending_once(summary, MissionKind::Share),
        }
    }

    fn filtered(mut self, switches: &crate::config::BbsTaskConfig) -> Self {
        if !switches.sign {
            self.sign = false;
        }
        if !switches.read {
            self.read = 0;
        }
        if !switches.like {
            self.like = 0;
        }
        if !switches.share {
            self.share = false;
        }
        self
    }

    fn without_stopped(mut self, stopped: StoppedTasks) -> Self {
        if stopped.sign {
            self.sign = false;
        }
        if stopped.read {
            self.read = 0;
        }
        if stopped.like {
            self.like = 0;
        }
        if stopped.share {
            self.share = false;
        }
        self
    }

    fn is_complete(self) -> bool {
        !self.sign && self.read == 0 && self.like == 0 && !self.share
    }

    fn required_posts(self) -> usize {
        (self.read.max(self.like).max(u32::from(self.share))) as usize
    }
}

fn pending_once(summary: &CoinSummary, kind: MissionKind) -> bool {
    summary
        .mission(kind)
        .map(|mission| !mission.award_received)
        .unwrap_or(false)
}

fn remaining(summary: &CoinSummary, kind: MissionKind, target: u32) -> u32 {
    summary
        .mission(kind)
        .map(|mission| {
            if mission.award_received {
                0
            } else {
                target.saturating_sub(mission.happened_times)
            }
        })
        .unwrap_or(0)
}

fn is_terminal(error: &BbsError) -> bool {
    matches!(error, BbsError::AuthExpired)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlowSignal {
    Continue,
    BlockHighRisk,
    Stop,
}

fn push_blocked_actions(report: &mut RunReport, account: &str, plan: &BbsPlan) {
    if plan.like > 0 {
        report.push(record(
            account,
            "点赞",
            "后续动作",
            TaskOutcome::Skipped,
            "此前触发验证码，已停止高风险动作",
        ));
    }
    if plan.share {
        report.push(record(
            account,
            "分享",
            "后续动作",
            TaskOutcome::Skipped,
            "此前触发验证码，已停止高风险动作",
        ));
    }
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
        signing::{BODY_SALT, sign_ds2_with},
    };
    use reqwest::Url;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, Request, ResponseTemplate,
        matchers::{body_json, method, path, query_param},
    };

    fn summary(can_get: u32, missions: Vec<MissionProgress>) -> CoinSummary {
        CoinSummary {
            can_get_points: can_get,
            already_received_points: 0,
            total_points: 100,
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

    #[test]
    fn completed_coin_day_produces_no_actions() {
        assert!(BbsPlan::from_summary(&summary(0, Vec::new())).is_complete());
    }

    #[test]
    fn plan_uses_only_mapped_missions_returned_by_server() {
        let plan = BbsPlan::from_summary(&summary(
            20,
            vec![
                mission(MissionKind::Sign, true, 1),
                mission(MissionKind::Read, false, 2),
                mission(MissionKind::Like, false, 99),
            ],
        ));
        assert_eq!(
            plan,
            BbsPlan {
                sign: false,
                read: 1,
                like: 0,
                share: false,
            }
        );
        assert_eq!(plan.required_posts(), 1);

        assert!(BbsPlan::from_summary(&summary(100, Vec::new())).is_complete());
    }

    #[test]
    fn attempt_counter_ignores_tasks_not_entered_after_an_earlier_block() {
        let mut attempts = TaskAttempts::default();
        attempts.record(BbsPlan {
            sign: true,
            ..BbsPlan::default()
        });

        assert_eq!(attempts.sign, 1);
        assert_eq!(attempts.read, 0);
        assert_eq!(attempts.like, 0);
        assert_eq!(attempts.share, 0);
    }

    #[test]
    fn rechecked_task_is_confirmed_or_times_out_independently() {
        let mut requests = CompletedRequests {
            reads: vec![PostRef {
                post_id: "42".to_owned(),
                subject: "测试帖子".to_owned(),
            }],
            ..CompletedRequests::default()
        };
        let mut confirmed_report = RunReport::default();
        let mut confirmed_stopped = StoppedTasks::default();
        assert!(!reconcile_task_results(
            &mut confirmed_report,
            "测试账号",
            BbsPlan {
                read: 1,
                ..BbsPlan::default()
            },
            &mut requests,
            &summary(20, vec![mission(MissionKind::Read, true, 3)]),
            TaskAttempts {
                read: 2,
                ..TaskAttempts::default()
            },
            3,
            &mut confirmed_stopped,
        ));
        assert_eq!(confirmed_report.records.len(), 1);
        assert!(
            confirmed_report
                .records
                .iter()
                .all(|record| record.outcome == TaskOutcome::Success)
        );
        assert!(
            confirmed_report
                .records
                .iter()
                .any(|record| record.message.contains("复查"))
        );
        assert!(confirmed_stopped.read);
        assert!(requests.reads.is_empty());

        let mut stale_requests = CompletedRequests::default();
        let mut stale_report = RunReport::default();
        let mut stale_stopped = StoppedTasks::default();
        assert!(reconcile_task_results(
            &mut stale_report,
            "测试账号",
            BbsPlan {
                read: 1,
                ..BbsPlan::default()
            },
            &mut stale_requests,
            &summary(30, vec![mission(MissionKind::Read, false, 3)]),
            TaskAttempts {
                read: 3,
                ..TaskAttempts::default()
            },
            3,
            &mut stale_stopped,
        ));
        assert_eq!(stale_report.exit_code(), 1);
        assert_eq!(stale_report.records.len(), 1);
        assert_eq!(
            stale_report.records[0].outcome,
            TaskOutcome::StateSyncTimeout
        );
        assert!(
            stale_report
                .records
                .iter()
                .any(|record| record.message.contains("状态同步超时"))
        );
        assert!(stale_stopped.read);
    }

    #[test]
    fn no_remaining_points_confirms_an_omitted_mission() {
        assert_eq!(
            mission_status(&summary(0, Vec::new()), MissionKind::Share),
            MissionStatus::Completed
        );
    }

    #[test]
    fn captcha_report_has_priority_and_marks_high_risk_actions_skipped() {
        let mut report = RunReport::default();
        push_error(&mut report, "account", "原神", BbsError::CaptchaRequired);
        push_blocked_actions(
            &mut report,
            "account",
            &BbsPlan {
                sign: true,
                read: 3,
                like: 5,
                share: true,
            },
        );

        assert_eq!(report.exit_code(), 4);
        assert_eq!(report.records[0].outcome, TaskOutcome::CaptchaRequired);
        assert!(
            report
                .records
                .iter()
                .skip(1)
                .all(|record| record.outcome == TaskOutcome::Skipped)
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
}
