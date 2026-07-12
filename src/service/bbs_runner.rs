use std::time::Duration;

use crate::{
    auth::{Credentials, SecretString},
    bbs::{BbsClient, BbsError, CoinSummary, ForumSignRequest, MissionKind, PostRef},
    config::{AccountConfig, Config},
    http::{HttpClient, RetryPolicy},
    signing::{DsSigner, SystemClock, ThreadRandom},
};

use super::{RunReport, TaskOutcome, TaskRecord, resolve_device_id};

const READ_TARGET: u32 = 3;
const LIKE_TARGET: u32 = 5;

#[derive(Clone, Copy)]
struct Forum {
    gids: &'static str,
    forum_id: &'static str,
    name: &'static str,
}

/// 兼容旧项目的安全默认版块顺序 `[5, 2]`：大别野、原神。
const DEFAULT_FORUMS: [Forum; 2] = [
    Forum {
        gids: "5",
        forum_id: "34",
        name: "大别野",
    },
    Forum {
        gids: "2",
        forum_id: "26",
        name: "原神",
    },
];

pub async fn run_bbs(config: &Config) -> RunReport {
    let mut report = RunReport::default();
    for account in &config.accounts {
        if account.enabled && account.tasks.bbs {
            run_account(&mut report, config, account).await;
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
    let client = BbsClient::new(http, app_cookie, web_cookie, device_id)
        .device(&account.device.name, &account.device.model);
    let mut signer = DsSigner::new(SystemClock, ThreadRandom);

    let summary = match client.missions().await {
        Ok(summary) => summary,
        Err(error) => {
            push_error(report, &account.name, "任务状态", error);
            return;
        }
    };
    let plan = BbsPlan::from_summary(&summary);
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

    let mut high_risk_blocked = false;
    if plan.sign {
        for forum in DEFAULT_FORUMS {
            let request = ForumSignRequest::new(forum.gids);
            let body = serde_json::to_vec(&request).expect("固定社区签到结构应当可序列化");
            let ds = signer.sign_body("", &body).to_string();
            match client.sign_forum_once(&request, &ds, None).await {
                Ok(()) => report.push(record(
                    &account.name,
                    "社区签到",
                    forum.name,
                    TaskOutcome::Success,
                    "签到请求成功",
                )),
                Err(BbsError::CaptchaRequired) => {
                    push_error(report, &account.name, forum.name, BbsError::CaptchaRequired);
                    high_risk_blocked = true;
                    break;
                }
                Err(error) => {
                    let terminal = is_terminal(&error);
                    push_error(report, &account.name, forum.name, error);
                    if terminal {
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
        match client.posts(DEFAULT_FORUMS[0].forum_id, 20, &ds).await {
            Ok(posts) => client.select_posts(&posts, required_posts),
            Err(error) => {
                push_error(report, &account.name, "帖子列表", error);
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
    )
    .await
    {
        FlowSignal::Continue => {}
        FlowSignal::BlockHighRisk => high_risk_blocked = true,
        FlowSignal::Stop => return,
    }

    if high_risk_blocked {
        push_blocked_actions(report, &account.name, &plan);
        return;
    }

    for post in selected.iter().take(plan.like as usize) {
        let ds = signer.sign_app().to_string();
        match client.set_like_once(&post.post_id, false, &ds, None).await {
            Ok(()) => report.push(post_record(
                &account.name,
                "点赞",
                post,
                TaskOutcome::Success,
                "点赞成功",
            )),
            Err(BbsError::CaptchaRequired) => {
                push_error(
                    report,
                    &account.name,
                    &post.subject,
                    BbsError::CaptchaRequired,
                );
                high_risk_blocked = true;
                break;
            }
            Err(error) => {
                let terminal = is_terminal(&error);
                push_error(report, &account.name, &post.subject, error);
                if terminal {
                    return;
                }
                continue;
            }
        }

        let ds = signer.sign_app().to_string();
        match client.set_like_once(&post.post_id, true, &ds, None).await {
            Ok(()) => report.push(post_record(
                &account.name,
                "取消点赞",
                post,
                TaskOutcome::Success,
                "已恢复点赞状态",
            )),
            Err(BbsError::CaptchaRequired) => {
                push_error(
                    report,
                    &account.name,
                    &post.subject,
                    BbsError::CaptchaRequired,
                );
                high_risk_blocked = true;
                break;
            }
            Err(error) => {
                let terminal = is_terminal(&error);
                push_error(report, &account.name, &post.subject, error);
                if terminal {
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
        return;
    }

    if plan.share {
        if let Some(post) = selected.first() {
            let ds = signer.sign_app().to_string();
            match client.share_post(&post.post_id, &ds).await {
                Ok(()) => report.push(post_record(
                    &account.name,
                    "分享",
                    post,
                    TaskOutcome::Success,
                    "分享任务请求成功",
                )),
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
}

async fn run_reads(
    report: &mut RunReport,
    account: &str,
    client: &BbsClient,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    posts: &[PostRef],
    count: u32,
) -> FlowSignal {
    for post in posts.iter().take(count as usize) {
        let ds = signer.sign_app().to_string();
        match client.read_post(&post.post_id, &ds).await {
            Ok(()) => report.push(post_record(
                account,
                "阅读",
                post,
                TaskOutcome::Success,
                "阅读任务请求成功",
            )),
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
        bbs::MissionProgress,
        signing::{BODY_SALT, sign_ds2_with},
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
}
