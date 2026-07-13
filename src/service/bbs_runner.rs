use std::{path::Path, time::Duration};

use crate::{
    auth::{Credentials, SecretString},
    bbs::{BbsClient, BbsError, CoinSummary, ForumSignRequest, MissionKind, PostRef, forum_by_id},
    captcha::CaptchaClient,
    config::{AccountConfig, Config},
    http::{HttpClient, RetryPolicy},
    signing::{DsSigner, SystemClock, ThreadRandom},
};

use super::{
    RunReport, TaskOutcome, TaskRecord,
    credential_refresh::{has_authentication_failure, refresh_account_cookie},
    resolve_device_id,
};

const READ_TARGET: u32 = 3;
const LIKE_TARGET: u32 = 5;

#[derive(Default)]
struct CompletedRequests {
    forum_signs: Vec<(String, bool)>,
    reads: Vec<PostRef>,
    likes: Vec<(PostRef, bool)>,
    share: Option<PostRef>,
}

impl CompletedRequests {
    fn is_empty(&self) -> bool {
        self.forum_signs.is_empty()
            && self.reads.is_empty()
            && self.likes.is_empty()
            && self.share.is_none()
    }
}

pub async fn run_bbs(config: &Config) -> RunReport {
    let mut runtime_config = config.clone();
    run_bbs_inner(&mut runtime_config, None).await
}

pub async fn run_bbs_with_refresh(config: &mut Config, path: &Path) -> RunReport {
    run_bbs_inner(config, Some(path)).await
}

async fn run_bbs_inner(config: &mut Config, path: Option<&Path>) -> RunReport {
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
            match refresh_account_cookie(config, account_index, refresh_http, path).await {
                Ok(persisted) => {
                    report.push(record(
                        &account.name,
                        "凭据刷新",
                        "cookie_token",
                        TaskOutcome::Success,
                        if persisted {
                            "检测到认证失效，已通过 SToken 刷新、写回配置并重试"
                        } else {
                            "检测到认证失效，已通过 SToken 刷新并重试；Cookie 来自环境变量，未写入配置文件"
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
    let captcha = config
        .captcha
        .endpoint
        .clone()
        .map(|endpoint| CaptchaClient::new(http.clone(), endpoint));
    let client = BbsClient::new(http, app_cookie, web_cookie, device_id).device(
        &account.device.name,
        &account.device.model,
        &account.device.fp,
    );
    let mut signer = DsSigner::new(SystemClock, ThreadRandom);

    let summary = match client.missions().await {
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

    let mut completed_requests = CompletedRequests::default();
    let mut high_risk_blocked = false;
    if plan.sign {
        for forum in &forums {
            let request = ForumSignRequest::new(forum.gids);
            match sign_forum_with_captcha(&client, captcha.as_ref(), &mut signer, &request).await {
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
                        confirm_completed_requests(
                            report,
                            &account.name,
                            &client,
                            &completed_requests,
                        )
                        .await;
                        return;
                    }
                }
            }
        }
    }

    let required_posts = plan.required_posts();
    let selected = if required_posts == 0 {
        Vec::new()
    } else {
        let ds = signer.sign_app().to_string();
        match client.posts(forums[0].forum_id, 20, &ds).await {
            Ok(posts) => client.select_posts(&posts, required_posts),
            Err(error) => {
                push_error(report, &account.name, "帖子列表", error);
                confirm_completed_requests(report, &account.name, &client, &completed_requests)
                    .await;
                return;
            }
        }
    };

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
        FlowSignal::Stop => {
            confirm_completed_requests(report, &account.name, &client, &completed_requests).await;
            return;
        }
    }

    if high_risk_blocked {
        push_blocked_actions(report, &account.name, &plan);
        confirm_completed_requests(report, &account.name, &client, &completed_requests).await;
        return;
    }

    for post in selected.iter().take(plan.like as usize) {
        match set_like_with_captcha(&client, captcha.as_ref(), &mut signer, &post.post_id, false)
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
                    confirm_completed_requests(report, &account.name, &client, &completed_requests)
                        .await;
                    return;
                }
                continue;
            }
        }

        if !account.tasks.bbs.cancel_like {
            continue;
        }
        match set_like_with_captcha(&client, captcha.as_ref(), &mut signer, &post.post_id, true)
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
                    confirm_completed_requests(report, &account.name, &client, &completed_requests)
                        .await;
                    return;
                }
            }
        }
    }

    if high_risk_blocked {
        if plan.share {
            report.push(record(
                &account.name,
                "分享",
                "后续动作",
                TaskOutcome::Skipped,
                "此前触发验证码，已停止高风险动作",
            ));
        }
        confirm_completed_requests(report, &account.name, &client, &completed_requests).await;
        return;
    }

    if plan.share {
        if let Some(post) = selected.first() {
            let ds = signer.sign_app().to_string();
            match client.share_post(&post.post_id, &ds).await {
                Ok(()) => completed_requests.share = Some(post.clone()),
                Err(error) => push_error(report, &account.name, &post.subject, error),
            }
        } else {
            report.push(record(
                &account.name,
                "分享",
                "帖子",
                TaskOutcome::Skipped,
                "没有可用帖子",
            ));
        }
    }

    confirm_completed_requests(report, &account.name, &client, &completed_requests).await;
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

async fn confirm_completed_requests(
    report: &mut RunReport,
    account: &str,
    client: &BbsClient,
    completed: &CompletedRequests,
) {
    if completed.is_empty() {
        return;
    }

    match client.missions().await {
        Ok(summary) => push_completion_results(report, account, completed, &summary),
        Err(error) => push_error(report, account, "任务完成复查", error),
    }
}

fn push_completion_results(
    report: &mut RunReport,
    account: &str,
    completed: &CompletedRequests,
    summary: &CoinSummary,
) {
    let sign_confirmed = mission_confirmed(summary, MissionKind::Sign);
    let read_confirmed = mission_confirmed(summary, MissionKind::Read);
    let like_confirmed = mission_confirmed(summary, MissionKind::Like);
    let share_confirmed = mission_confirmed(summary, MissionKind::Share);

    for (forum, captcha_solved) in &completed.forum_signs {
        report.push(record(
            account,
            "社区签到",
            forum,
            completion_outcome(sign_confirmed),
            if sign_confirmed {
                if *captcha_solved {
                    "验证码通过后提交签到，任务状态复查已确认米游币领取"
                } else {
                    "签到请求已提交，任务状态复查已确认米游币领取"
                }
            } else {
                "签到请求返回成功，但任务状态复查未确认米游币领取"
            },
        ));
    }
    for post in &completed.reads {
        report.push(post_record(
            account,
            "阅读",
            post,
            completion_outcome(read_confirmed),
            if read_confirmed {
                "任务状态复查已确认阅读米游币领取"
            } else {
                "阅读请求返回成功，但任务状态复查未确认米游币领取"
            },
        ));
    }
    for (post, captcha_solved) in &completed.likes {
        report.push(post_record(
            account,
            "点赞",
            post,
            completion_outcome(like_confirmed),
            if like_confirmed {
                if *captcha_solved {
                    "验证码通过后提交点赞，任务状态复查已确认米游币领取"
                } else {
                    "任务状态复查已确认点赞米游币领取"
                }
            } else {
                "点赞请求返回成功，但任务状态复查未确认米游币领取"
            },
        ));
    }
    if let Some(post) = &completed.share {
        report.push(post_record(
            account,
            "分享",
            post,
            completion_outcome(share_confirmed),
            if share_confirmed {
                "任务状态复查已确认分享米游币领取"
            } else {
                "分享请求返回成功，但任务状态复查未确认米游币领取"
            },
        ));
    }

    let all_confirmed = (completed.forum_signs.is_empty() || sign_confirmed)
        && (completed.reads.is_empty() || read_confirmed)
        && (completed.likes.is_empty() || like_confirmed)
        && (completed.share.is_none() || share_confirmed);
    report.push(record(
        account,
        "米游币",
        "完成确认",
        completion_outcome(all_confirmed),
        &format!(
            "复查后已领取 {}，还可领取 {}，当前共 {} 米游币",
            summary.already_received_points, summary.can_get_points, summary.total_points
        ),
    ));
}

fn mission_confirmed(summary: &CoinSummary, kind: MissionKind) -> bool {
    summary.can_get_points == 0
        || summary
            .mission(kind)
            .is_some_and(|mission| mission.award_received)
}

fn completion_outcome(confirmed: bool) -> TaskOutcome {
    if confirmed {
        TaskOutcome::Success
    } else {
        TaskOutcome::Failed
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
) -> Result<bool, ActionError> {
    let body = serde_json::to_vec(request).expect("固定社区签到结构应当可序列化");
    let ds = signer.sign_body("", &body).to_string();
    match client.sign_forum_once(request, &ds, None).await {
        Ok(()) => Ok(false),
        Err(BbsError::CaptchaRequired) => {
            let challenge = solve_bbs_captcha(client, captcha, signer).await?;
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
) -> Result<bool, ActionError> {
    let ds = signer.sign_app().to_string();
    match client.set_like_once(post_id, cancel, &ds, None).await {
        Ok(()) => Ok(false),
        Err(BbsError::CaptchaRequired) => {
            let challenge = solve_bbs_captcha(client, captcha, signer).await?;
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
) -> Result<String, ActionError> {
    let captcha = captcha
        .ok_or_else(|| ActionError::Captcha("触发验证码，但未配置 captcha.endpoint".to_owned()))?;
    let ds = signer.sign_app().to_string();
    let verification = client
        .create_verification(&ds)
        .await
        .map_err(|error| ActionError::Captcha(format!("创建米游社验证码失败：{error}")))?;
    let solution = captcha
        .solve(&verification.gt, &verification.challenge)
        .await
        .map_err(|error| ActionError::Captcha(format!("验证码平台求解失败：{error}")))?;
    let ds = signer.sign_app().to_string();
    client
        .verify_verification(&solution.challenge, &solution.validate, &ds)
        .await
        .map_err(|error| ActionError::Captcha(format!("米游社验证码校验失败：{error}")))
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
        .unwrap_or(true)
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
        .unwrap_or(target)
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
    use super::*;
    use crate::{
        bbs::{BbsEndpoints, MissionProgress},
        signing::{BODY_SALT, sign_ds2_with},
    };
    use reqwest::Url;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
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

    fn test_client(server: &MockServer) -> BbsClient {
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
            SecretString::new("stuid=123;stoken=test-secret"),
            SecretString::new("cookie_token=test-secret"),
            "test-device-id",
        )
        .endpoints(BbsEndpoints::from_base_url(&base).unwrap())
    }

    #[test]
    fn completed_coin_day_produces_no_actions() {
        assert!(BbsPlan::from_summary(&summary(0, Vec::new())).is_complete());
    }

    #[test]
    fn plan_uses_saturating_remaining_counts_and_fresh_day_defaults() {
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
                share: true,
            }
        );
        assert_eq!(plan.required_posts(), 1);

        let fresh = BbsPlan::from_summary(&summary(100, Vec::new()));
        assert_eq!(fresh.read, READ_TARGET);
        assert_eq!(fresh.like, LIKE_TARGET);
        assert!(fresh.sign && fresh.share);
    }

    #[tokio::test]
    async fn completed_request_success_depends_on_rechecked_mission_state() {
        let confirmed_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apihub/wapi/getUserMissionsState"))
            .and(query_param("point_sn", "myb"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {
                    "can_get_points": 20,
                    "already_received_points": 80,
                    "total_points": 180,
                    "states": [
                        {"mission_id": 59, "is_get_award": true, "happened_times": 3}
                    ]
                }
            })))
            .expect(1)
            .mount(&confirmed_server)
            .await;
        let requests = CompletedRequests {
            reads: vec![PostRef {
                post_id: "42".to_owned(),
                subject: "测试帖子".to_owned(),
            }],
            ..CompletedRequests::default()
        };
        let mut confirmed_report = RunReport::default();
        confirm_completed_requests(
            &mut confirmed_report,
            "测试账号",
            &test_client(&confirmed_server),
            &requests,
        )
        .await;
        assert_eq!(confirmed_report.records.len(), 2);
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

        let stale_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apihub/wapi/getUserMissionsState"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {
                    "can_get_points": 30,
                    "already_received_points": 70,
                    "total_points": 170,
                    "states": [
                        {"mission_id": 59, "is_get_award": false, "happened_times": 3}
                    ]
                }
            })))
            .expect(1)
            .mount(&stale_server)
            .await;
        let mut stale_report = RunReport::default();
        confirm_completed_requests(
            &mut stale_report,
            "测试账号",
            &test_client(&stale_server),
            &requests,
        )
        .await;
        assert_eq!(stale_report.exit_code(), 1);
        assert!(
            stale_report
                .records
                .iter()
                .all(|record| record.outcome == TaskOutcome::Failed)
        );
        assert!(
            stale_report
                .records
                .iter()
                .any(|record| record.message.contains("未确认"))
        );
    }

    #[test]
    fn no_remaining_points_confirms_an_omitted_mission() {
        assert!(mission_confirmed(
            &summary(0, Vec::new()),
            MissionKind::Share
        ));
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
            solve_bbs_captcha(&client, Some(&captcha), &mut signer)
                .await
                .unwrap(),
            "passed-challenge"
        );
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
            solve_bbs_captcha(&client, None, &mut signer).await,
            Err(ActionError::Captcha(message)) if message.contains("未配置 captcha.endpoint")
        ));
    }
}
