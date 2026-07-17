use std::{path::Path, time::Duration};

#[cfg(test)]
use crate::auth::SecretString;
use crate::{
    captcha::CaptchaClient,
    checkin::{
        CaptchaHeaders, CheckinError, CheckinState, ChinaCheckinClient, ChinaGame, Reward,
        RoleState, SignState,
    },
    checkin::{HoyolabCheckinClient, HoyolabCheckinError, HoyolabGame},
    config::{AccountConfig, Config, Game},
    http::{HttpClient, RetryPolicy},
    signing::{DsSigner, SystemClock, ThreadRandom},
};

use super::{
    CredentialPersistence, RunReport, TaskOutcome, TaskRecord,
    credential_refresh::{has_authentication_failure, refresh_account_cookie},
    resolve_device_id,
};

pub async fn run_china_checkin(config: &Config) -> RunReport {
    let mut runtime_config = config.clone();
    run_china_checkin_inner(&mut runtime_config, CredentialPersistence::ReadOnly).await
}

pub async fn run_china_checkin_with_refresh(config: &mut Config, path: &Path) -> RunReport {
    run_china_checkin_with_persistence(config, CredentialPersistence::CurrentConfig(path)).await
}

pub async fn run_china_checkin_with_persistence(
    config: &mut Config,
    persistence: CredentialPersistence<'_>,
) -> RunReport {
    run_china_checkin_inner(config, persistence).await
}

async fn run_china_checkin_inner(
    config: &mut Config,
    persistence: CredentialPersistence<'_>,
) -> RunReport {
    let mut report = RunReport::default();
    for account_index in 0..config.accounts.len() {
        let account = config.accounts[account_index].clone();
        if !account.enabled || !account.tasks.china_game_checkin {
            continue;
        }
        let first = run_china_account(config, &account).await;
        if !has_authentication_failure(&first) {
            report.extend(first);
            continue;
        }

        let refresh_http = match build_account_http(config, &account) {
            Ok(http) => http,
            Err(message) => {
                report.extend(first);
                report.push(record(
                    &account.name,
                    "凭据刷新",
                    "cookie_token",
                    TaskOutcome::NetworkFailed,
                    &message,
                ));
                continue;
            }
        };
        match refresh_account_cookie(config, account_index, refresh_http, persistence.path()).await
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
                report.extend(run_china_account(config, &refreshed).await);
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
    report
}

async fn run_china_account(config: &Config, account: &AccountConfig) -> RunReport {
    let mut report = RunReport::default();
    let http = match build_account_http(config, account) {
        Ok(http) => http,
        Err(message) => {
            report.push(record(
                &account.name,
                "网络初始化",
                "HTTP 客户端",
                TaskOutcome::NetworkFailed,
                &message,
            ));
            return report;
        }
    };
    let cookie = account.credentials.cookie.clone();
    let device_id = resolve_device_id(&account.device.id, cookie.expose_secret());
    let captcha = config
        .captcha
        .endpoint
        .clone()
        .map(|endpoint| CaptchaClient::new(http.clone(), endpoint));
    let client = ChinaCheckinClient::new(http, cookie, device_id)
        .user_agent(account.china_checkin.user_agent.clone());
    let mut signer = DsSigner::new(SystemClock, ThreadRandom);

    for configured_game in &account.games {
        let Some(game) = config_game_to_china(configured_game) else {
            continue;
        };
        let context = ChinaGameContext {
            account: &account.name,
            client: &client,
            captcha: captcha.as_ref(),
            role_blacklist: account
                .china_checkin
                .role_blacklist
                .for_game(*configured_game),
            max_attempts: config.runtime.task_max_attempts,
        };
        run_game(&mut report, &mut signer, game, context).await;
    }
    report
}

fn build_account_http(config: &Config, account: &AccountConfig) -> Result<HttpClient, String> {
    let builder = HttpClient::builder()
        .timeout(Duration::from_secs(config.runtime.request_timeout_seconds))
        .retry(RetryPolicy {
            attempts: usize::try_from(config.runtime.retry_count).unwrap_or(usize::MAX),
            base_delay: Duration::from_millis(500),
        })
        .proxy(account.proxy.url.as_ref().map(|url| url.expose_secret()))
        .map_err(|error| error.to_string())?;
    builder.build().map_err(|error| error.to_string())
}

pub async fn run_hoyolab_checkin(config: &Config) -> RunReport {
    let mut report = RunReport::default();
    for account in &config.accounts {
        if !account.enabled || !account.tasks.hoyolab_checkin {
            continue;
        }
        let builder = HttpClient::builder()
            .timeout(Duration::from_secs(config.runtime.request_timeout_seconds))
            .retry(RetryPolicy {
                attempts: usize::try_from(config.runtime.retry_count).unwrap_or(usize::MAX),
                base_delay: Duration::from_millis(500),
            });
        let builder = match builder.proxy(account.proxy.url.as_ref().map(|url| url.expose_secret()))
        {
            Ok(builder) => builder,
            Err(error) => {
                report.push(record(
                    &account.name,
                    "HoYoLAB 签到",
                    "代理",
                    TaskOutcome::NetworkFailed,
                    &error.to_string(),
                ));
                continue;
            }
        };
        let http = match builder.build() {
            Ok(http) => http,
            Err(error) => {
                report.push(record(
                    &account.name,
                    "HoYoLAB 签到",
                    "HTTP 客户端",
                    TaskOutcome::NetworkFailed,
                    &error.to_string(),
                ));
                continue;
            }
        };
        let (cookie, language, user_agent, games) = match &account.hoyolab {
            Some(hoyolab) => (
                hoyolab.cookie.clone(),
                hoyolab.language.as_str(),
                hoyolab.user_agent.as_str(),
                hoyolab.games.as_slice(),
            ),
            None => (
                account.credentials.cookie.clone(),
                "en-us",
                "Mozilla/5.0 (Linux; Android 12) AppleWebKit/537.36 Mobile Safari/537.36",
                account.games.as_slice(),
            ),
        };
        let client = HoyolabCheckinClient::new(http, cookie)
            .language(language)
            .user_agent(user_agent);
        for game in games.iter().filter_map(config_game_to_hoyolab) {
            run_hoyolab_game(
                &mut report,
                &account.name,
                &client,
                game,
                config.runtime.task_max_attempts,
            )
            .await;
        }
    }
    report
}

struct ChinaGameContext<'a> {
    account: &'a str,
    client: &'a ChinaCheckinClient,
    captcha: Option<&'a CaptchaClient>,
    role_blacklist: &'a [String],
    max_attempts: u32,
}

async fn run_game(
    report: &mut RunReport,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    game: ChinaGame,
    context: ChinaGameContext<'_>,
) {
    let spec = game.spec();
    let roles = match context
        .client
        .roles(game, &signer.sign_web().to_string())
        .await
    {
        Ok(RoleState::NoRole) => {
            report.push(record(
                context.account,
                "国内游戏签到",
                spec.display_name,
                TaskOutcome::Skipped,
                "没有绑定角色",
            ));
            return;
        }
        Ok(RoleState::Available(roles)) => roles,
        Err(error) => {
            push_error(report, context.account, spec.display_name, error);
            return;
        }
    };
    let rewards = context
        .client
        .home(game, &signer.sign_web().to_string())
        .await
        .ok();

    for role in roles {
        let subject = format!("{} / {}", spec.display_name, mask_uid(&role.uid));
        if role_is_excluded(context.role_blacklist, &role.uid) {
            report.push(record(
                context.account,
                "国内游戏签到",
                &subject,
                TaskOutcome::Skipped,
                "角色 UID 在配置黑名单中",
            ));
            continue;
        }
        run_china_role(
            report,
            context.account,
            &subject,
            context.client,
            context.captcha,
            signer,
            game,
            &role.region,
            &role.uid,
            rewards.as_deref(),
            context.max_attempts,
        )
        .await;
    }
}

fn role_is_excluded(role_blacklist: &[String], uid: &str) -> bool {
    role_blacklist.iter().any(|excluded| excluded == uid)
}

fn config_game_to_china(game: &Game) -> Option<ChinaGame> {
    match game {
        Game::Genshin => Some(ChinaGame::Genshin),
        Game::Honkai2 => Some(ChinaGame::Honkai2),
        Game::Honkai3rd => Some(ChinaGame::Honkai3rd),
        Game::TearsOfThemis => Some(ChinaGame::TearsOfThemis),
        Game::StarRail => Some(ChinaGame::StarRail),
        Game::ZenlessZoneZero => Some(ChinaGame::ZenlessZoneZero),
    }
}

fn config_game_to_hoyolab(game: &Game) -> Option<HoyolabGame> {
    match game {
        Game::Genshin => Some(HoyolabGame::Genshin),
        Game::Honkai2 => None,
        Game::Honkai3rd => Some(HoyolabGame::Honkai3rd),
        Game::TearsOfThemis => Some(HoyolabGame::TearsOfThemis),
        Game::StarRail => Some(HoyolabGame::StarRail),
        Game::ZenlessZoneZero => Some(HoyolabGame::ZenlessZoneZero),
    }
}

fn push_hoyolab_error(
    report: &mut RunReport,
    account: &str,
    subject: &str,
    error: HoyolabCheckinError,
) {
    let (outcome, message) = match error {
        HoyolabCheckinError::CookieInvalid => (
            TaskOutcome::AuthenticationFailed,
            "Cookie 无效或已过期".to_owned(),
        ),
        HoyolabCheckinError::Http(_) => (TaskOutcome::NetworkFailed, "网络请求失败".to_owned()),
        other => (TaskOutcome::Failed, other.to_string()),
    };
    report.push(record(account, "HoYoLAB 签到", subject, outcome, &message));
}

fn push_error(report: &mut RunReport, account: &str, subject: &str, error: CheckinError) {
    let (outcome, message) = match error {
        CheckinError::CookieInvalid => (
            TaskOutcome::AuthenticationFailed,
            "Cookie 无效或已过期".to_owned(),
        ),
        CheckinError::Http(_) => (TaskOutcome::NetworkFailed, "网络请求失败".to_owned()),
        other => (TaskOutcome::Failed, other.to_string()),
    };
    report.push(record(account, "国内游戏签到", subject, outcome, &message));
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

fn mask_uid(uid: &str) -> String {
    let visible = uid.chars().rev().take(4).collect::<Vec<_>>();
    let suffix = visible.into_iter().rev().collect::<String>();
    format!("***{suffix}")
}

enum ChinaSubmitResult {
    Submitted {
        outcome: TaskOutcome,
        captcha_solved: bool,
    },
    Failed(CheckinError),
    CaptchaBlocked(String),
}

async fn submit_china_sign_once(
    client: &ChinaCheckinClient,
    captcha: Option<&CaptchaClient>,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    game: ChinaGame,
    region: &str,
    uid: &str,
) -> ChinaSubmitResult {
    let state = match client
        .sign_once(game, region, uid, &signer.sign_web().to_string(), None)
        .await
    {
        Ok(state) => state,
        Err(error) => return ChinaSubmitResult::Failed(error),
    };
    match state {
        SignState::Success => ChinaSubmitResult::Submitted {
            outcome: TaskOutcome::Success,
            captcha_solved: false,
        },
        SignState::AlreadySigned => ChinaSubmitResult::Submitted {
            outcome: TaskOutcome::AlreadyCompleted,
            captcha_solved: false,
        },
        SignState::CaptchaRequired { gt, challenge } => {
            let Some(captcha) = captcha else {
                return ChinaSubmitResult::CaptchaBlocked(
                    "触发验证码，但未配置 captcha.endpoint".to_owned(),
                );
            };
            let solution = match captcha.solve(&gt, &challenge).await {
                Ok(solution) => solution,
                Err(error) => {
                    return ChinaSubmitResult::CaptchaBlocked(format!(
                        "验证码平台求解失败：{error}"
                    ));
                }
            };
            let headers = CaptchaHeaders {
                challenge: &solution.challenge,
                validate: &solution.validate,
            };
            match client
                .sign_once(
                    game,
                    region,
                    uid,
                    &signer.sign_web().to_string(),
                    Some(&headers),
                )
                .await
            {
                Ok(SignState::Success) => ChinaSubmitResult::Submitted {
                    outcome: TaskOutcome::Success,
                    captcha_solved: true,
                },
                Ok(SignState::AlreadySigned) => ChinaSubmitResult::Submitted {
                    outcome: TaskOutcome::AlreadyCompleted,
                    captcha_solved: true,
                },
                Ok(SignState::CaptchaRequired { .. }) => ChinaSubmitResult::CaptchaBlocked(
                    "验证码校验后仍被要求验证，已停止本次签到".to_owned(),
                ),
                Err(error) => ChinaSubmitResult::Failed(error),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_china_role(
    report: &mut RunReport,
    account: &str,
    subject: &str,
    client: &ChinaCheckinClient,
    captcha: Option<&CaptchaClient>,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    game: ChinaGame,
    region: &str,
    uid: &str,
    rewards: Option<&[Reward]>,
    max_attempts: u32,
) {
    let max_attempts = max_attempts.max(1);
    let mut attempts = 0;
    let mut last_submission = None;

    loop {
        match client
            .status(game, region, uid, &signer.sign_web().to_string())
            .await
        {
            Ok(CheckinState::FirstBind) if attempts == 0 => {
                report.push(record(
                    account,
                    "国内游戏签到",
                    subject,
                    TaskOutcome::Skipped,
                    "首次绑定，请先手动签到一次",
                ));
                return;
            }
            Ok(CheckinState::FirstBind) => {
                report.push(record(
                    account,
                    "国内游戏签到",
                    subject,
                    TaskOutcome::NetworkFailed,
                    "签到提交后复查显示需要首次手动签到，已停止重试",
                ));
                return;
            }
            Ok(CheckinState::AlreadySigned { total_sign_day }) if attempts == 0 => {
                report.push(record(
                    account,
                    "国内游戏签到",
                    subject,
                    TaskOutcome::AlreadyCompleted,
                    &with_reward(
                        &format!("今日已签到，累计 {total_sign_day} 天"),
                        rewards,
                        total_sign_day,
                    ),
                ));
                return;
            }
            Ok(CheckinState::AlreadySigned { total_sign_day }) => {
                let (outcome, confirmation) = match last_submission.as_ref() {
                    Some(ChinaSubmitResult::Submitted {
                        outcome,
                        captcha_solved,
                    }) => (
                        *outcome,
                        format!(
                            "{}签到成功，第 {attempts} 次尝试后复查确认今日已签到，累计 {total_sign_day} 天",
                            if *captcha_solved {
                                "验证码通过后"
                            } else {
                                ""
                            }
                        ),
                    ),
                    Some(ChinaSubmitResult::Failed(_)) => (
                        TaskOutcome::Success,
                        format!(
                            "签到请求返回错误，但第 {attempts} 次尝试后复查确认今日已签到，累计 {total_sign_day} 天"
                        ),
                    ),
                    Some(ChinaSubmitResult::CaptchaBlocked(_)) | None => {
                        unreachable!("只有实际提交过签到请求后才会进入复查成功分支")
                    }
                };
                report.push(record(
                    account,
                    "国内游戏签到",
                    subject,
                    outcome,
                    &with_reward(&confirmation, rewards, total_sign_day),
                ));
                return;
            }
            Ok(CheckinState::Pending { .. }) if attempts >= max_attempts => {
                push_china_attempts_exhausted(report, account, subject, attempts, last_submission);
                return;
            }
            Ok(CheckinState::Pending { .. }) => {}
            Err(error) if attempts == 0 => {
                push_error(report, account, subject, error);
                return;
            }
            Err(error) => {
                report.push(record(
                    account,
                    "国内游戏签到",
                    subject,
                    TaskOutcome::NetworkFailed,
                    &format!(
                        "第 {attempts} 次签到提交后复查失败，未继续提交以避免重复签到：{error}"
                    ),
                ));
                return;
            }
        }

        attempts += 1;
        match submit_china_sign_once(client, captcha, signer, game, region, uid).await {
            result @ (ChinaSubmitResult::Submitted { .. } | ChinaSubmitResult::Failed(_)) => {
                last_submission = Some(result);
            }
            ChinaSubmitResult::CaptchaBlocked(message) => {
                report.push(record(
                    account,
                    "国内游戏签到",
                    subject,
                    TaskOutcome::CaptchaRequired,
                    &message,
                ));
                return;
            }
        }
    }
}

fn push_china_attempts_exhausted(
    report: &mut RunReport,
    account: &str,
    subject: &str,
    attempts: u32,
    last_submission: Option<ChinaSubmitResult>,
) {
    let (outcome, detail) = match last_submission {
        Some(ChinaSubmitResult::Failed(CheckinError::CookieInvalid)) => (
            TaskOutcome::AuthenticationFailed,
            "最后一次签到请求显示 Cookie 无效或已过期".to_owned(),
        ),
        Some(ChinaSubmitResult::Failed(CheckinError::Http(_))) => (
            TaskOutcome::NetworkFailed,
            "最后一次签到请求网络失败".to_owned(),
        ),
        Some(ChinaSubmitResult::Failed(error)) => (
            TaskOutcome::Failed,
            format!("最后一次签到请求失败：{error}"),
        ),
        Some(ChinaSubmitResult::Submitted { .. }) | None => (
            TaskOutcome::NetworkFailed,
            "每次提交后复查都仍显示今日未签到".to_owned(),
        ),
        Some(ChinaSubmitResult::CaptchaBlocked(_)) => {
            unreachable!("验证码阻断会立即生成报告")
        }
    };
    report.push(record(
        account,
        "国内游戏签到",
        subject,
        outcome,
        &format!("已达到配置的 {attempts} 次签到尝试；{detail}"),
    ));
}

enum HoyolabSubmitResult {
    Submitted(TaskOutcome),
    Failed(HoyolabCheckinError),
}

async fn run_hoyolab_game(
    report: &mut RunReport,
    account: &str,
    client: &HoyolabCheckinClient,
    game: HoyolabGame,
    max_attempts: u32,
) {
    let subject = game.spec().display_name;
    let max_attempts = max_attempts.max(1);
    let mut attempts = 0;
    let mut last_submission = None;

    loop {
        match client.info(game).await {
            Ok(CheckinState::FirstBind) if attempts == 0 => {
                report.push(record(
                    account,
                    "HoYoLAB 签到",
                    subject,
                    TaskOutcome::Skipped,
                    "首次绑定，请先手动签到一次",
                ));
                return;
            }
            Ok(CheckinState::FirstBind) => {
                report.push(record(
                    account,
                    "HoYoLAB 签到",
                    subject,
                    TaskOutcome::NetworkFailed,
                    "签到提交后复查显示需要首次手动签到，已停止重试",
                ));
                return;
            }
            Ok(CheckinState::AlreadySigned { total_sign_day }) if attempts == 0 => {
                let rewards = client.home(game).await.ok();
                report.push(record(
                    account,
                    "HoYoLAB 签到",
                    subject,
                    TaskOutcome::AlreadyCompleted,
                    &with_reward(
                        &format!("今日已签到，累计 {total_sign_day} 天"),
                        rewards.as_deref(),
                        total_sign_day,
                    ),
                ));
                return;
            }
            Ok(CheckinState::AlreadySigned { total_sign_day }) => {
                let rewards = client.home(game).await.ok();
                let (outcome, confirmation) = match last_submission.as_ref() {
                    Some(HoyolabSubmitResult::Submitted(outcome)) => (
                        *outcome,
                        format!(
                            "签到成功，第 {attempts} 次尝试后复查确认今日已签到，累计 {total_sign_day} 天"
                        ),
                    ),
                    Some(HoyolabSubmitResult::Failed(_)) => (
                        TaskOutcome::Success,
                        format!(
                            "签到请求返回错误，但第 {attempts} 次尝试后复查确认今日已签到，累计 {total_sign_day} 天"
                        ),
                    ),
                    None => unreachable!("只有实际提交过签到请求后才会进入复查成功分支"),
                };
                report.push(record(
                    account,
                    "HoYoLAB 签到",
                    subject,
                    outcome,
                    &with_reward(&confirmation, rewards.as_deref(), total_sign_day),
                ));
                return;
            }
            Ok(CheckinState::Pending { .. }) if attempts >= max_attempts => {
                push_hoyolab_attempts_exhausted(
                    report,
                    account,
                    subject,
                    attempts,
                    last_submission,
                );
                return;
            }
            Ok(CheckinState::Pending { .. }) => {}
            Err(error) if attempts == 0 => {
                push_hoyolab_error(report, account, subject, error);
                return;
            }
            Err(error) => {
                report.push(record(
                    account,
                    "HoYoLAB 签到",
                    subject,
                    TaskOutcome::NetworkFailed,
                    &format!(
                        "第 {attempts} 次签到提交后复查失败，未继续提交以避免重复签到：{error}"
                    ),
                ));
                return;
            }
        }

        attempts += 1;
        match client.sign_once(game).await {
            Ok(SignState::Success) => {
                last_submission = Some(HoyolabSubmitResult::Submitted(TaskOutcome::Success));
            }
            Ok(SignState::AlreadySigned) => {
                last_submission = Some(HoyolabSubmitResult::Submitted(
                    TaskOutcome::AlreadyCompleted,
                ));
            }
            Ok(SignState::CaptchaRequired { .. }) => {
                report.push(record(
                    account,
                    "HoYoLAB 签到",
                    subject,
                    TaskOutcome::CaptchaRequired,
                    "触发验证码，已停止本次签到",
                ));
                return;
            }
            Err(error) => {
                last_submission = Some(HoyolabSubmitResult::Failed(error));
            }
        }
    }
}

fn push_hoyolab_attempts_exhausted(
    report: &mut RunReport,
    account: &str,
    subject: &str,
    attempts: u32,
    last_submission: Option<HoyolabSubmitResult>,
) {
    let (outcome, detail) = match last_submission {
        Some(HoyolabSubmitResult::Failed(HoyolabCheckinError::CookieInvalid)) => (
            TaskOutcome::AuthenticationFailed,
            "最后一次签到请求显示 Cookie 无效或已过期".to_owned(),
        ),
        Some(HoyolabSubmitResult::Failed(HoyolabCheckinError::Http(_))) => (
            TaskOutcome::NetworkFailed,
            "最后一次签到请求网络失败".to_owned(),
        ),
        Some(HoyolabSubmitResult::Failed(error)) => (
            TaskOutcome::Failed,
            format!("最后一次签到请求失败：{error}"),
        ),
        Some(HoyolabSubmitResult::Submitted(_)) | None => (
            TaskOutcome::NetworkFailed,
            "每次提交后复查都仍显示今日未签到".to_owned(),
        ),
    };
    report.push(record(
        account,
        "HoYoLAB 签到",
        subject,
        outcome,
        &format!("已达到配置的 {attempts} 次签到尝试；{detail}"),
    ));
}

fn with_reward(base: &str, rewards: Option<&[Reward]>, total_sign_day: u32) -> String {
    let Some(rewards) = rewards else {
        return format!("{base}；奖励详情获取失败");
    };
    let Some(index) = total_sign_day
        .checked_sub(1)
        .and_then(|day| usize::try_from(day).ok())
    else {
        return format!("{base}；未找到第 {total_sign_day} 天奖励");
    };
    match rewards.get(index) {
        Some(reward) => format!("{base}；今日奖励：{} ×{}", reward.name, reward.cnt),
        None => format!("{base}；未找到第 {total_sign_day} 天奖励"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use reqwest::Url;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, Request, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    #[test]
    fn uid_mask_only_keeps_last_four_characters() {
        assert_eq!(mask_uid("123456789"), "***6789");
        assert_eq!(mask_uid("12"), "***12");
    }

    #[tokio::test]
    async fn missing_captcha_endpoint_stops_before_captcha_submission() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/event/luna/sign"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {"success": 1, "gt": "gt", "challenge": "challenge"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let http = HttpClient::builder().build().unwrap();
        let client =
            ChinaCheckinClient::new(http, SecretString::new("cookie_token=secret"), "device-id")
                .endpoint_override(Url::parse(&server.uri()).unwrap());
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);
        let result = submit_china_sign_once(
            &client,
            None,
            &mut signer,
            ChinaGame::Genshin,
            "cn_gf01",
            "10001",
        )
        .await;

        assert!(matches!(
            result,
            ChinaSubmitResult::CaptchaBlocked(message) if message.contains("captcha.endpoint")
        ));
    }

    #[tokio::test]
    async fn retries_only_while_status_still_reports_pending() {
        let server = MockServer::start().await;
        let status_count = Arc::new(AtomicUsize::new(0));
        let responder_count = Arc::clone(&status_count);
        Mock::given(method("GET"))
            .and(path("/event/luna/info"))
            .respond_with(move |_request: &Request| {
                let request = responder_count.fetch_add(1, Ordering::SeqCst) + 1;
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": {
                        "total_sign_day": if request >= 3 { 15 } else { 14 },
                        "is_sign": request >= 3,
                        "first_bind": false
                    }
                }))
            })
            .expect(3)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/event/luna/sign"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {"success": 0, "gt": "", "challenge": ""}
            })))
            .expect(2)
            .mount(&server)
            .await;

        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        let client =
            ChinaCheckinClient::new(http, SecretString::new("cookie_token=secret"), "device-id")
                .endpoint_override(Url::parse(&server.uri()).unwrap());
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);
        let mut report = RunReport::default();

        run_china_role(
            &mut report,
            "account",
            "原神 / ***0001",
            &client,
            None,
            &mut signer,
            ChinaGame::Genshin,
            "cn_gf01",
            "10001",
            None,
            3,
        )
        .await;

        assert_eq!(status_count.load(Ordering::SeqCst), 3);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::Success);
        assert!(report.records[0].message.contains("第 2 次尝试后复查确认"));
    }

    #[tokio::test]
    async fn stops_after_configured_attempts_when_status_remains_pending() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/event/luna/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {
                    "total_sign_day": 14,
                    "is_sign": false,
                    "first_bind": false
                }
            })))
            .expect(4)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/event/luna/sign"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {"success": 0, "gt": "", "challenge": ""}
            })))
            .expect(3)
            .mount(&server)
            .await;

        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        let client =
            ChinaCheckinClient::new(http, SecretString::new("cookie_token=secret"), "device-id")
                .endpoint_override(Url::parse(&server.uri()).unwrap());
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);
        let mut report = RunReport::default();

        run_china_role(
            &mut report,
            "account",
            "原神 / ***0001",
            &client,
            None,
            &mut signer,
            ChinaGame::Genshin,
            "cn_gf01",
            "10001",
            None,
            3,
        )
        .await;

        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::NetworkFailed);
        assert!(report.records[0].message.contains("3 次签到尝试"));
    }

    #[tokio::test]
    async fn hoyolab_retries_only_after_pending_confirmation() {
        let server = MockServer::start().await;
        let status_count = Arc::new(AtomicUsize::new(0));
        let responder_count = Arc::clone(&status_count);
        Mock::given(method("GET"))
            .and(path("/info"))
            .respond_with(move |_request: &Request| {
                let request = responder_count.fetch_add(1, Ordering::SeqCst) + 1;
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": {
                        "total_sign_day": if request >= 3 { 15 } else { 14 },
                        "is_sign": request >= 3,
                        "first_bind": false
                    }
                }))
            })
            .expect(3)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sign"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": null
            })))
            .expect(2)
            .mount(&server)
            .await;

        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        let client = HoyolabCheckinClient::new(http, SecretString::new("ltoken=secret"))
            .endpoint_override(Url::parse(&server.uri()).unwrap());
        let mut report = RunReport::default();

        run_hoyolab_game(&mut report, "account", &client, HoyolabGame::Genshin, 3).await;

        assert_eq!(status_count.load(Ordering::SeqCst), 3);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::Success);
        assert!(report.records[0].message.contains("第 2 次尝试后复查确认"));
    }

    #[test]
    fn reward_detail_uses_confirmed_sign_day_without_changing_outcome() {
        let rewards = vec![
            Reward {
                icon: String::new(),
                name: "摩拉".to_owned(),
                cnt: 5_000,
            },
            Reward {
                icon: String::new(),
                name: "原石".to_owned(),
                cnt: 20,
            },
        ];
        assert_eq!(
            with_reward("今日已签到，累计 2 天", Some(&rewards), 2),
            "今日已签到，累计 2 天；今日奖励：原石 ×20"
        );
        assert_eq!(
            with_reward("签到成功", None, 2),
            "签到成功；奖励详情获取失败"
        );
    }

    #[test]
    fn role_blacklist_matches_complete_uid_only() {
        let blacklist = vec!["123456789".to_owned()];
        assert!(role_is_excluded(&blacklist, "123456789"));
        assert!(!role_is_excluded(&blacklist, "6789"));
    }
}
