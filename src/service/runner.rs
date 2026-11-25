use std::path::Path;
use std::time::Duration;

#[cfg(test)]
use crate::auth::SecretString;
#[cfg(test)]
use crate::http::RetryPolicy;
use crate::{
    captcha::CaptchaClient,
    checkin::{
        CaptchaHeaders, CheckinError, CheckinState, ChinaCheckinClient, ChinaGame, Reward,
        RoleState, SignState,
    },
    checkin::{HoyolabCheckinClient, HoyolabCheckinError, HoyolabGame},
    config::{AccountConfig, Config, Game},
    http::HttpClient,
    signing::{DsSigner, SystemClock, ThreadRandom},
};

use super::{
    CredentialPersistence, RecordKind, RunReport, TaskId, TaskOutcome, TaskRecord,
    credential_refresh::{has_authentication_failure, refresh_account_cookie},
    resolve_device_id,
};

/// 提交后的只读确认查询次数上限：提交次数由 `task_max_attempts` 控制，
/// 确认查询不消耗提交次数，但同样有明确上限。
const CHECKIN_CONFIRM_MAX_QUERIES: u32 = 3;
/// 每次确认查询前的等待；`CHECKIN_CONFIRM_MAX_QUERIES` 次等待即整个确认窗口。
#[cfg(not(test))]
const CHECKIN_CONFIRM_DELAY: Duration = Duration::from_secs(1);
#[cfg(test)]
const CHECKIN_CONFIRM_DELAY: Duration = Duration::ZERO;
/// 确认窗口总时长：生产 3 秒（3 次 × 1 秒），测试为 0 秒以免拖慢用例。
#[cfg(not(test))]
const CHECKIN_CONFIRM_WINDOW: Duration = Duration::from_secs(3);
#[cfg(test)]
const CHECKIN_CONFIRM_WINDOW: Duration = Duration::ZERO;

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
    crate::http::factory::for_account(config, account).map_err(|error| {
        // 只暴露类别，不把代理地址等连接细节写进任务报告。
        crate::http::safe_http_reason(&error).to_owned()
    })
}

pub async fn run_hoyolab_checkin(config: &Config) -> RunReport {
    let mut report = RunReport::default();
    for account in &config.accounts {
        if !account.enabled || !account.tasks.hoyolab_checkin {
            continue;
        }
        let http = match crate::http::factory::for_account(config, account) {
            Ok(http) => http,
            Err(error) => {
                report.push(record(
                    &account.name,
                    "HoYoLAB 签到",
                    "HTTP 客户端",
                    TaskOutcome::NetworkFailed,
                    crate::http::safe_http_reason(&error),
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

/// 把国内签到错误映射为报告结果；认证失败类别必须在提交后复查时保留，
/// 否则账号级 SToken 刷新不会被触发。
fn classify_checkin_error(error: &CheckinError) -> TaskOutcome {
    match error {
        CheckinError::CookieInvalid => TaskOutcome::AuthenticationFailed,
        CheckinError::Http(_) => TaskOutcome::NetworkFailed,
        CheckinError::InvalidHeader(_)
        | CheckinError::InvalidResponse(_)
        | CheckinError::Api { .. } => TaskOutcome::Failed,
    }
}

fn checkin_error_message(error: &CheckinError) -> String {
    match error {
        CheckinError::CookieInvalid => "Cookie 无效或已过期".to_owned(),
        CheckinError::Http(_) => "网络请求失败".to_owned(),
        other => other.to_string(),
    }
}

/// HoYoLAB 使用独立凭据逻辑，不做国内 SToken 刷新，但结果类别与国内保持同一口径。
fn classify_hoyolab_checkin_error(error: &HoyolabCheckinError) -> TaskOutcome {
    match error {
        HoyolabCheckinError::CookieInvalid => TaskOutcome::AuthenticationFailed,
        HoyolabCheckinError::Http(_) => TaskOutcome::NetworkFailed,
        HoyolabCheckinError::InvalidHeader(_)
        | HoyolabCheckinError::InvalidResponse(_)
        | HoyolabCheckinError::Api { .. } => TaskOutcome::Failed,
    }
}

fn hoyolab_checkin_error_message(error: &HoyolabCheckinError) -> String {
    match error {
        HoyolabCheckinError::CookieInvalid => "Cookie 无效或已过期".to_owned(),
        HoyolabCheckinError::Http(_) => "网络请求失败".to_owned(),
        other => other.to_string(),
    }
}

fn push_hoyolab_error(
    report: &mut RunReport,
    account: &str,
    subject: &str,
    error: HoyolabCheckinError,
) {
    let outcome = classify_hoyolab_checkin_error(&error);
    let message = hoyolab_checkin_error_message(&error);
    report.push(
        record(account, "HoYoLAB 签到", subject, outcome, &message)
            .with_error_code(hoyolab_error_code(&error)),
    );
}

fn push_error(report: &mut RunReport, account: &str, subject: &str, error: CheckinError) {
    let outcome = classify_checkin_error(&error);
    let message = checkin_error_message(&error);
    report.push(
        record(account, "国内游戏签到", subject, outcome, &message)
            .with_error_code(checkin_error_code(&error)),
    );
}

/// 签到错误的稳定错误码；`Api` 保留 retcode 便于外部聚合。
fn checkin_error_code(error: &CheckinError) -> String {
    match error {
        CheckinError::CookieInvalid => "cookie_invalid".to_owned(),
        CheckinError::Http(_) => "http_error".to_owned(),
        CheckinError::InvalidHeader(_) => "invalid_header".to_owned(),
        CheckinError::InvalidResponse(_) => "invalid_response".to_owned(),
        CheckinError::Api { retcode, .. } => format!("retcode:{retcode}"),
    }
}

fn hoyolab_error_code(error: &HoyolabCheckinError) -> String {
    match error {
        HoyolabCheckinError::CookieInvalid => "cookie_invalid".to_owned(),
        HoyolabCheckinError::Http(_) => "http_error".to_owned(),
        HoyolabCheckinError::InvalidHeader(_) => "invalid_header".to_owned(),
        HoyolabCheckinError::InvalidResponse(_) => "invalid_response".to_owned(),
        HoyolabCheckinError::Api { retcode, .. } => format!("retcode:{retcode}"),
    }
}

fn record(
    account: &str,
    task: &str,
    subject: &str,
    outcome: TaskOutcome,
    message: &str,
) -> TaskRecord {
    TaskRecord::new(account, task, subject, outcome, message)
        .with_task_id(checkin_task_id(task))
        .with_kind(checkin_record_kind(task))
}

/// 按签到任务名映射稳定标识。
fn checkin_task_id(task: &str) -> TaskId {
    match task {
        "国内游戏签到" => TaskId::ChinaCheckin,
        "HoYoLAB 签到" => TaskId::HoyolabCheckin,
        "凭据刷新" => TaskId::CredentialRefresh,
        "网络初始化" => TaskId::Config,
        "配置" => TaskId::Config,
        "通知" => TaskId::Notification,
        _ => TaskId::Config,
    }
}

/// 凭据刷新、网络初始化与配置提示属诊断信息，不计入任务数。
fn checkin_record_kind(task: &str) -> RecordKind {
    match task {
        "凭据刷新" | "网络初始化" | "配置" | "通知" => RecordKind::Diagnostic,
        _ => RecordKind::Task,
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

fn submission_authentication_failed(submission: Option<&ChinaSubmitResult>) -> bool {
    matches!(
        submission,
        Some(ChinaSubmitResult::Failed(error))
            if classify_checkin_error(error) == TaskOutcome::AuthenticationFailed
    )
}

/// 提交后只读确认的结果；`Failed` 携带对应平台的错误类型，供上层沿用统一分类。
enum Confirmation<E> {
    Signed { total_sign_day: u32 },
    FirstBind,
    StillPending,
    Failed(E),
}

/// 在确认窗口内轮询国内签到状态。查询本身失败时立即返回，绝不因确认失败再次提交。
async fn confirm_china_sign(
    client: &ChinaCheckinClient,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    game: ChinaGame,
    region: &str,
    uid: &str,
) -> Confirmation<CheckinError> {
    for _ in 0..CHECKIN_CONFIRM_MAX_QUERIES {
        tokio::time::sleep(CHECKIN_CONFIRM_DELAY).await;
        match client
            .status(game, region, uid, &signer.sign_web().to_string())
            .await
        {
            Ok(CheckinState::AlreadySigned { total_sign_day }) => {
                return Confirmation::Signed { total_sign_day };
            }
            Ok(CheckinState::FirstBind) => return Confirmation::FirstBind,
            Ok(CheckinState::Pending { .. }) => {}
            Err(error) => return Confirmation::Failed(error),
        }
    }
    Confirmation::StillPending
}

fn log_confirmation_window(account: &str, subject: &str, submissions: u32, max_attempts: u32) {
    tracing::info!(
        account,
        subject,
        submission = submissions,
        max_submissions = max_attempts,
        confirm_queries = CHECKIN_CONFIRM_MAX_QUERIES,
        confirm_delay_seconds = CHECKIN_CONFIRM_DELAY.as_secs(),
        window_seconds = CHECKIN_CONFIRM_WINDOW.as_secs(),
        "签到已提交，进入只读状态确认窗口"
    );
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
                        "验证码平台求解失败：{}",
                        error.safe_reason()
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

    // 首次只读查询：已签到、首次绑定与查询错误都在此终止，不产生任何提交。
    match client
        .status(game, region, uid, &signer.sign_web().to_string())
        .await
    {
        Ok(CheckinState::FirstBind) => {
            report.push(record(
                account,
                "国内游戏签到",
                subject,
                TaskOutcome::Skipped,
                "首次绑定，请先手动签到一次",
            ));
            return;
        }
        Ok(CheckinState::AlreadySigned { total_sign_day }) => {
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
        Ok(CheckinState::Pending { .. }) => {}
        Err(error) => {
            push_error(report, account, subject, error);
            return;
        }
    }

    let mut submissions = 0;
    loop {
        submissions += 1;
        // 每轮提交结果只在本次迭代内使用，直接由 match 产生 Option，
        // 避免循环外初始化后又被覆盖的死赋值。
        let last_submission =
            match submit_china_sign_once(client, captcha, signer, game, region, uid).await {
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
                result => Some(result),
            };
        log_confirmation_window(account, subject, submissions, max_attempts);

        // 提交后只做只读确认：确认窗口内的查询不属于“提交”，不消耗提交次数。
        match confirm_china_sign(client, signer, game, region, uid).await {
            Confirmation::Signed { total_sign_day } => {
                let (outcome, confirmation) = match last_submission.as_ref() {
                    Some(ChinaSubmitResult::Submitted {
                        outcome,
                        captcha_solved,
                    }) => (
                        *outcome,
                        format!(
                            "{}签到成功，第 {submissions} 次提交后复查确认今日已签到，累计 {total_sign_day} 天",
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
                            "签到请求返回错误，但第 {submissions} 次提交后复查确认今日已签到，累计 {total_sign_day} 天"
                        ),
                    ),
                    Some(ChinaSubmitResult::CaptchaBlocked(_)) | None => {
                        unreachable!("只有实际提交过签到请求后才会进入确认成功分支")
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
            Confirmation::FirstBind => {
                report.push(record(
                    account,
                    "国内游戏签到",
                    subject,
                    TaskOutcome::NetworkFailed,
                    "签到提交后复查显示需要首次手动签到，已停止重试",
                ));
                return;
            }
            Confirmation::Failed(error) => {
                let outcome = classify_checkin_error(&error);
                report.push(
                    record(
                        account,
                        "国内游戏签到",
                        subject,
                        outcome,
                        &format!(
                            "第 {submissions} 次签到提交后确认查询失败；已提交，结果未知，未再次提交：{}",
                            checkin_error_message(&error)
                        ),
                    )
                    .with_error_code(checkin_error_code(&error)),
                );
                return;
            }
            Confirmation::StillPending => {
                // 认证已失效时不能靠下一次提交“再试”：交给账号级 SToken 刷新，
                // 刷新后的重跑会先做只读状态确认，明确未完成后才允许再次提交。
                if submission_authentication_failed(last_submission.as_ref()) {
                    report.push(record(
                        account,
                        "国内游戏签到",
                        subject,
                        TaskOutcome::AuthenticationFailed,
                        &format!(
                            "第 {submissions} 次签到提交显示认证失效；已提交，结果未知，未再次提交，等待凭据刷新后先做只读状态确认"
                        ),
                    ));
                    return;
                }
                // 确认窗口内明确仍未完成，且还有提交次数时，才允许下一次提交。
                if submissions >= max_attempts {
                    push_china_attempts_exhausted(
                        report,
                        account,
                        subject,
                        submissions,
                        last_submission,
                    );
                    return;
                }
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
        Some(ChinaSubmitResult::Failed(error)) => {
            let outcome = classify_checkin_error(&error);
            let detail = match outcome {
                TaskOutcome::AuthenticationFailed => {
                    "最后一次签到请求显示 Cookie 无效或已过期".to_owned()
                }
                TaskOutcome::NetworkFailed => "最后一次签到请求网络失败".to_owned(),
                _ => format!("最后一次签到请求失败：{error}"),
            };
            (outcome, detail)
        }
        Some(ChinaSubmitResult::Submitted { .. }) => (
            TaskOutcome::StateSyncTimeout,
            "每次提交后的确认窗口都仍显示今日未签到".to_owned(),
        ),
        None => (
            TaskOutcome::NetworkFailed,
            "签到状态查询始终未确认今日已签到".to_owned(),
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
        &format!("已达到配置的 {attempts} 次签到提交；{detail}"),
    ));
}

enum HoyolabSubmitResult {
    Submitted(TaskOutcome),
    Failed(HoyolabCheckinError),
}

/// 在确认窗口内轮询 HoYoLAB 签到状态；查询失败立即返回，不再提交。
async fn confirm_hoyolab_sign(
    client: &HoyolabCheckinClient,
    game: HoyolabGame,
) -> Confirmation<HoyolabCheckinError> {
    for _ in 0..CHECKIN_CONFIRM_MAX_QUERIES {
        tokio::time::sleep(CHECKIN_CONFIRM_DELAY).await;
        match client.info(game).await {
            Ok(CheckinState::AlreadySigned { total_sign_day }) => {
                return Confirmation::Signed { total_sign_day };
            }
            Ok(CheckinState::FirstBind) => return Confirmation::FirstBind,
            Ok(CheckinState::Pending { .. }) => {}
            Err(error) => return Confirmation::Failed(error),
        }
    }
    Confirmation::StillPending
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

    // 首次只读查询：已签到、首次绑定与查询错误都在此终止，不产生任何提交。
    match client.info(game).await {
        Ok(CheckinState::FirstBind) => {
            report.push(record(
                account,
                "HoYoLAB 签到",
                subject,
                TaskOutcome::Skipped,
                "首次绑定，请先手动签到一次",
            ));
            return;
        }
        Ok(CheckinState::AlreadySigned { total_sign_day }) => {
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
        Ok(CheckinState::Pending { .. }) => {}
        Err(error) => {
            push_hoyolab_error(report, account, subject, error);
            return;
        }
    }

    let mut submissions = 0;
    loop {
        submissions += 1;
        // 同国内流程：提交结果只在本次迭代内使用，由 match 直接产生 Option。
        let last_submission = match client.sign_once(game).await {
            Ok(SignState::Success) => Some(HoyolabSubmitResult::Submitted(TaskOutcome::Success)),
            Ok(SignState::AlreadySigned) => Some(HoyolabSubmitResult::Submitted(
                TaskOutcome::AlreadyCompleted,
            )),
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
            Err(error) => Some(HoyolabSubmitResult::Failed(error)),
        };
        log_confirmation_window(account, subject, submissions, max_attempts);

        // 提交后只做只读确认：确认窗口内的查询不消耗提交次数。
        match confirm_hoyolab_sign(client, game).await {
            Confirmation::Signed { total_sign_day } => {
                let rewards = client.home(game).await.ok();
                let (outcome, confirmation) = match last_submission.as_ref() {
                    Some(HoyolabSubmitResult::Submitted(outcome)) => (
                        *outcome,
                        format!(
                            "签到成功，第 {submissions} 次提交后复查确认今日已签到，累计 {total_sign_day} 天"
                        ),
                    ),
                    Some(HoyolabSubmitResult::Failed(_)) => (
                        TaskOutcome::Success,
                        format!(
                            "签到请求返回错误，但第 {submissions} 次提交后复查确认今日已签到，累计 {total_sign_day} 天"
                        ),
                    ),
                    None => unreachable!("只有实际提交过签到请求后才会进入确认成功分支"),
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
            Confirmation::FirstBind => {
                report.push(record(
                    account,
                    "HoYoLAB 签到",
                    subject,
                    TaskOutcome::NetworkFailed,
                    "签到提交后复查显示需要首次手动签到，已停止重试",
                ));
                return;
            }
            Confirmation::Failed(error) => {
                let outcome = classify_hoyolab_checkin_error(&error);
                report.push(
                    record(
                        account,
                        "HoYoLAB 签到",
                        subject,
                        outcome,
                        &format!(
                            "第 {submissions} 次签到提交后确认查询失败；已提交，结果未知，未再次提交：{}",
                            hoyolab_checkin_error_message(&error)
                        ),
                    )
                    .with_error_code(hoyolab_error_code(&error)),
                );
                return;
            }
            Confirmation::StillPending => {
                // 确认窗口内明确仍未完成，且还有提交次数时，才允许下一次提交。
                if submissions >= max_attempts {
                    push_hoyolab_attempts_exhausted(
                        report,
                        account,
                        subject,
                        submissions,
                        last_submission,
                    );
                    return;
                }
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
        Some(HoyolabSubmitResult::Failed(error)) => {
            let outcome = classify_hoyolab_checkin_error(&error);
            let detail = match outcome {
                TaskOutcome::AuthenticationFailed => {
                    "最后一次签到请求显示 Cookie 无效或已过期".to_owned()
                }
                TaskOutcome::NetworkFailed => "最后一次签到请求网络失败".to_owned(),
                _ => format!("最后一次签到请求失败：{error}"),
            };
            (outcome, detail)
        }
        Some(HoyolabSubmitResult::Submitted(_)) => (
            TaskOutcome::StateSyncTimeout,
            "每次提交后的确认窗口都仍显示今日未签到".to_owned(),
        ),
        None => (
            TaskOutcome::NetworkFailed,
            "签到状态查询始终未确认今日已签到".to_owned(),
        ),
    };
    report.push(record(
        account,
        "HoYoLAB 签到",
        subject,
        outcome,
        &format!("已达到配置的 {attempts} 次签到提交；{detail}"),
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

    use crate::http::HttpError;
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

    /// 状态序列：Pending → 提交成功 → Pending → Pending → 已签到。
    /// 断言：确认窗口内完成确认，`sign` 只被调用一次，提交次数不因 Pending 增加。
    #[tokio::test]
    async fn pending_confirmation_window_confirms_without_resubmitting() {
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
                        "total_sign_day": if request >= 4 { 15 } else { 14 },
                        "is_sign": request >= 4,
                        "first_bind": false
                    }
                }))
            })
            .expect(4)
            .mount(&server)
            .await;
        let sign_count = Arc::new(AtomicUsize::new(0));
        let sign_responder = Arc::clone(&sign_count);
        Mock::given(method("POST"))
            .and(path("/event/luna/sign"))
            .respond_with(move |_request: &Request| {
                sign_responder.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": {"success": 0, "gt": "", "challenge": ""}
                }))
            })
            .expect(1)
            .mount(&server)
            .await;

        let client = china_client(&server);
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

        // 1 次首次查询 + 3 次确认查询；提交只发生 1 次。
        assert_eq!(status_count.load(Ordering::SeqCst), 4);
        assert_eq!(sign_count.load(Ordering::SeqCst), 1);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::Success);
        assert!(
            report.records[0]
                .message
                .contains("第 1 次提交后复查确认今日已签到，累计 15 天")
        );
    }

    /// 状态序列：持续 Pending。`task_max_attempts` 是最多提交次数，
    /// 每次都先跑完确认窗口；断言提交次数、确认查询次数与分类均有明确上限。
    #[tokio::test]
    async fn persistent_pending_hits_submission_limit_with_state_sync_timeout() {
        let server = MockServer::start().await;
        let status_count = Arc::new(AtomicUsize::new(0));
        let status_responder = Arc::clone(&status_count);
        Mock::given(method("GET"))
            .and(path("/event/luna/info"))
            .respond_with(move |_request: &Request| {
                status_responder.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(pending_info())
            })
            // 1 次首次查询 + 3 次提交 × 3 次确认查询。
            .expect(10)
            .mount(&server)
            .await;
        let sign_count = Arc::new(AtomicUsize::new(0));
        let sign_responder = Arc::clone(&sign_count);
        Mock::given(method("POST"))
            .and(path("/event/luna/sign"))
            .respond_with(move |_request: &Request| {
                sign_responder.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": {"success": 0, "gt": "", "challenge": ""}
                }))
            })
            .expect(3)
            .mount(&server)
            .await;

        let client = china_client(&server);
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

        assert_eq!(status_count.load(Ordering::SeqCst), 10);
        assert_eq!(sign_count.load(Ordering::SeqCst), 3);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::StateSyncTimeout);
        assert!(report.records[0].message.contains("3 次签到提交"));
        assert!(
            report.records[0]
                .message
                .contains("每次提交后的确认窗口都仍显示今日未签到")
        );
    }

    /// 提交次数上限为 1 时，只允许一次提交，确认窗口后立即给出状态同步超时。
    #[tokio::test]
    async fn single_configured_submission_is_never_exceeded_by_confirmation() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/event/luna/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pending_info()))
            // 1 次首次查询 + 1 次提交 × 3 次确认查询。
            .expect(4)
            .mount(&server)
            .await;
        let sign_count = Arc::new(AtomicUsize::new(0));
        let sign_responder = Arc::clone(&sign_count);
        Mock::given(method("POST"))
            .and(path("/event/luna/sign"))
            .respond_with(move |_request: &Request| {
                sign_responder.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": {"success": 0, "gt": "", "challenge": ""}
                }))
            })
            .expect(1)
            .mount(&server)
            .await;

        let client = china_client(&server);
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
            1,
        )
        .await;

        assert_eq!(sign_count.load(Ordering::SeqCst), 1);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::StateSyncTimeout);
        assert!(report.records[0].message.contains("1 次签到提交"));
    }

    /// 状态序列：Pending → 提交成功 → Pending → Pending → 已签到。
    /// 断言：HoYoLAB 同样只提交一次，由确认窗口完成确认。
    #[tokio::test]
    async fn hoyolab_pending_confirmation_window_confirms_without_resubmitting() {
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
                        "total_sign_day": if request >= 4 { 15 } else { 14 },
                        "is_sign": request >= 4,
                        "first_bind": false
                    }
                }))
            })
            .expect(4)
            .mount(&server)
            .await;
        let sign_count = Arc::new(AtomicUsize::new(0));
        let sign_responder = Arc::clone(&sign_count);
        Mock::given(method("POST"))
            .and(path("/sign"))
            .respond_with(move |_request: &Request| {
                sign_responder.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": null
                }))
            })
            .expect(1)
            .mount(&server)
            .await;

        let client = hoyolab_client(&server);
        let mut report = RunReport::default();

        run_hoyolab_game(&mut report, "account", &client, HoyolabGame::Genshin, 3).await;

        assert_eq!(status_count.load(Ordering::SeqCst), 4);
        assert_eq!(sign_count.load(Ordering::SeqCst), 1);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::Success);
        assert!(
            report.records[0]
                .message
                .contains("第 1 次提交后复查确认今日已签到，累计 15 天")
        );
    }

    /// 状态序列：持续 Pending。断言 HoYoLAB 提交次数上限与分类同样准确。
    #[tokio::test]
    async fn hoyolab_persistent_pending_hits_submission_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pending_info()))
            // 1 次首次查询 + 3 次提交 × 3 次确认查询。
            .expect(10)
            .mount(&server)
            .await;
        let sign_count = Arc::new(AtomicUsize::new(0));
        let sign_responder = Arc::clone(&sign_count);
        Mock::given(method("POST"))
            .and(path("/sign"))
            .respond_with(move |_request: &Request| {
                sign_responder.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": null
                }))
            })
            .expect(3)
            .mount(&server)
            .await;

        let client = hoyolab_client(&server);
        let mut report = RunReport::default();

        run_hoyolab_game(&mut report, "account", &client, HoyolabGame::Genshin, 3).await;

        assert_eq!(sign_count.load(Ordering::SeqCst), 3);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::StateSyncTimeout);
        assert!(report.records[0].message.contains("3 次签到提交"));
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

    #[test]
    fn checkin_error_classification_keeps_authentication_and_transport_categories() {
        assert_eq!(
            classify_checkin_error(&CheckinError::CookieInvalid),
            TaskOutcome::AuthenticationFailed
        );
        assert_eq!(
            classify_checkin_error(&CheckinError::Http(HttpError::Timeout)),
            TaskOutcome::NetworkFailed
        );
        assert_eq!(
            classify_checkin_error(&CheckinError::InvalidResponse("无效数据".to_owned())),
            TaskOutcome::Failed
        );
        assert_eq!(
            classify_checkin_error(&CheckinError::Api {
                retcode: -1,
                message: "activity unavailable".to_owned()
            }),
            TaskOutcome::Failed
        );
        assert_eq!(
            classify_hoyolab_checkin_error(&HoyolabCheckinError::CookieInvalid),
            TaskOutcome::AuthenticationFailed
        );
        assert_eq!(
            classify_hoyolab_checkin_error(&HoyolabCheckinError::Http(HttpError::Connect(
                "连接失败".to_owned()
            ))),
            TaskOutcome::NetworkFailed
        );
        assert_eq!(
            classify_hoyolab_checkin_error(&HoyolabCheckinError::InvalidHeader("Cookie")),
            TaskOutcome::Failed
        );
    }

    fn china_client(server: &MockServer) -> ChinaCheckinClient {
        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        ChinaCheckinClient::new(http, SecretString::new("cookie_token=secret"), "device-id")
            .endpoint_override(Url::parse(&server.uri()).unwrap())
    }

    fn hoyolab_client(server: &MockServer) -> HoyolabCheckinClient {
        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        HoyolabCheckinClient::new(http, SecretString::new("ltoken=secret"))
            .endpoint_override(Url::parse(&server.uri()).unwrap())
    }

    fn pending_info() -> serde_json::Value {
        json!({
            "retcode": 0,
            "message": "OK",
            "data": { "total_sign_day": 14, "is_sign": false, "first_bind": false }
        })
    }

    fn signed_info(total_sign_day: u32) -> serde_json::Value {
        json!({
            "retcode": 0,
            "message": "OK",
            "data": {
                "total_sign_day": total_sign_day,
                "is_sign": true,
                "first_bind": false
            }
        })
    }

    fn cookie_invalid_envelope() -> serde_json::Value {
        json!({
            "retcode": -100,
            "message": "登录失效，请重新登录",
            "data": null
        })
    }

    /// 状态序列：Pending → 提交 → 复查 CookieInvalid。
    /// 断言：结果分类为认证失败、触发刷新判定、提交只发生一次。
    #[tokio::test]
    async fn recheck_authentication_failure_after_submission_keeps_auth_category() {
        let server = MockServer::start().await;
        let status_count = Arc::new(AtomicUsize::new(0));
        let status_responder = Arc::clone(&status_count);
        Mock::given(method("GET"))
            .and(path("/event/luna/info"))
            .respond_with(move |_request: &Request| {
                let call = status_responder.fetch_add(1, Ordering::SeqCst) + 1;
                if call == 1 {
                    ResponseTemplate::new(200).set_body_json(pending_info())
                } else {
                    ResponseTemplate::new(200).set_body_json(cookie_invalid_envelope())
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        let sign_count = Arc::new(AtomicUsize::new(0));
        let sign_responder = Arc::clone(&sign_count);
        Mock::given(method("POST"))
            .and(path("/event/luna/sign"))
            .respond_with(move |_request: &Request| {
                sign_responder.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": { "success": 0, "gt": "", "challenge": "" }
                }))
            })
            .expect(1)
            .mount(&server)
            .await;

        let client = china_client(&server);
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

        assert_eq!(status_count.load(Ordering::SeqCst), 2);
        assert_eq!(sign_count.load(Ordering::SeqCst), 1);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::AuthenticationFailed);
        assert!(
            report.records[0]
                .message
                .contains("已提交，结果未知，未再次提交")
        );
        assert!(report.records[0].message.contains("Cookie 无效或已过期"));
        assert!(has_authentication_failure(&report));
    }

    /// 状态序列：Pending → 提交 → 复查传输失败。
    /// 断言：仍为网络失败、提交只发生一次、保留提交结果未知上下文。
    #[tokio::test]
    async fn recheck_transport_failure_after_submission_stays_network_failed() {
        let server = MockServer::start().await;
        let status_count = Arc::new(AtomicUsize::new(0));
        let status_responder = Arc::clone(&status_count);
        Mock::given(method("GET"))
            .and(path("/event/luna/info"))
            .respond_with(move |_request: &Request| {
                let call = status_responder.fetch_add(1, Ordering::SeqCst) + 1;
                if call == 1 {
                    ResponseTemplate::new(200).set_body_json(pending_info())
                } else {
                    ResponseTemplate::new(500)
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        let sign_count = Arc::new(AtomicUsize::new(0));
        let sign_responder = Arc::clone(&sign_count);
        Mock::given(method("POST"))
            .and(path("/event/luna/sign"))
            .respond_with(move |_request: &Request| {
                sign_responder.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": { "success": 0, "gt": "", "challenge": "" }
                }))
            })
            .expect(1)
            .mount(&server)
            .await;

        let client = china_client(&server);
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

        assert_eq!(status_count.load(Ordering::SeqCst), 2);
        assert_eq!(sign_count.load(Ordering::SeqCst), 1);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::NetworkFailed);
        assert!(
            report.records[0]
                .message
                .contains("已提交，结果未知，未再次提交")
        );
        assert!(report.records[0].message.contains("网络请求失败"));
        assert!(!has_authentication_failure(&report));
    }

    /// 状态序列：Pending → 提交返回 CookieInvalid → 复查仍 Pending；
    /// 随后模拟刷新后重跑，首次只读查询即已签到。
    /// 断言：首次运行报告认证失败且不再提交；重跑不提交；sign 全局只调用一次。
    #[tokio::test]
    async fn authentication_failure_after_submission_waits_for_refresh_confirmation() {
        let server = MockServer::start().await;
        let status_count = Arc::new(AtomicUsize::new(0));
        let status_responder = Arc::clone(&status_count);
        Mock::given(method("GET"))
            .and(path("/event/luna/info"))
            .respond_with(move |_request: &Request| {
                let call = status_responder.fetch_add(1, Ordering::SeqCst) + 1;
                ResponseTemplate::new(200).set_body_json(match call {
                    // 首次查询 + 确认窗口（3 次）均为 Pending；
                    // 之后模拟凭据刷新重跑，首次只读查询即已签到。
                    1..=4 => pending_info(),
                    _ => signed_info(15),
                })
            })
            .expect(5)
            .mount(&server)
            .await;
        let sign_count = Arc::new(AtomicUsize::new(0));
        let sign_responder = Arc::clone(&sign_count);
        Mock::given(method("POST"))
            .and(path("/event/luna/sign"))
            .respond_with(move |_request: &Request| {
                sign_responder.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(cookie_invalid_envelope())
            })
            .expect(1)
            .mount(&server)
            .await;

        let client = china_client(&server);
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);
        let mut first = RunReport::default();

        run_china_role(
            &mut first,
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

        assert_eq!(sign_count.load(Ordering::SeqCst), 1);
        assert_eq!(first.records.len(), 1);
        assert_eq!(first.records[0].outcome, TaskOutcome::AuthenticationFailed);
        assert!(first.records[0].message.contains("认证失效"));
        assert!(has_authentication_failure(&first));

        // 模拟账号级 SToken 刷新后重跑：只读查询已签到即返回，不产生第二次提交。
        let mut second = RunReport::default();
        run_china_role(
            &mut second,
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

        assert_eq!(status_count.load(Ordering::SeqCst), 5);
        assert_eq!(sign_count.load(Ordering::SeqCst), 1);
        assert_eq!(second.records.len(), 1);
        assert_eq!(second.records[0].outcome, TaskOutcome::AlreadyCompleted);
        assert!(second.records[0].message.contains("今日已签到，累计 15 天"));
        assert!(!has_authentication_failure(&second));
    }

    /// 状态序列：Pending → 提交 → 复查 CookieInvalid。
    /// 断言：HoYoLAB 仅修正错误分类，不触发国内 SToken 刷新路径。
    #[tokio::test]
    async fn hoyolab_recheck_authentication_failure_keeps_auth_category() {
        let server = MockServer::start().await;
        let status_count = Arc::new(AtomicUsize::new(0));
        let status_responder = Arc::clone(&status_count);
        Mock::given(method("GET"))
            .and(path("/info"))
            .respond_with(move |_request: &Request| {
                let call = status_responder.fetch_add(1, Ordering::SeqCst) + 1;
                if call == 1 {
                    ResponseTemplate::new(200).set_body_json(pending_info())
                } else {
                    ResponseTemplate::new(200).set_body_json(cookie_invalid_envelope())
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        let sign_count = Arc::new(AtomicUsize::new(0));
        let sign_responder = Arc::clone(&sign_count);
        Mock::given(method("POST"))
            .and(path("/sign"))
            .respond_with(move |_request: &Request| {
                sign_responder.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": 0,
                    "message": "OK",
                    "data": null
                }))
            })
            .expect(1)
            .mount(&server)
            .await;

        let client = hoyolab_client(&server);
        let mut report = RunReport::default();

        run_hoyolab_game(&mut report, "account", &client, HoyolabGame::Genshin, 3).await;

        assert_eq!(status_count.load(Ordering::SeqCst), 2);
        assert_eq!(sign_count.load(Ordering::SeqCst), 1);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::AuthenticationFailed);
        assert!(
            report.records[0]
                .message
                .contains("已提交，结果未知，未再次提交")
        );
        assert!(report.records[0].message.contains("Cookie 无效或已过期"));
        assert!(has_authentication_failure(&report));
    }
}
