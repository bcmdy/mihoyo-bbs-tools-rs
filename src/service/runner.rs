use std::time::Duration;

use crate::{
    auth::SecretString,
    captcha::CaptchaClient,
    checkin::{
        CaptchaHeaders, CheckinError, CheckinState, ChinaCheckinClient, ChinaGame, RoleState,
        SignState,
    },
    checkin::{HoyolabCheckinClient, HoyolabCheckinError, HoyolabGame},
    config::{Config, Game},
    http::{HttpClient, RetryPolicy},
    signing::{DsSigner, SystemClock, ThreadRandom},
};

use super::{RunReport, TaskOutcome, TaskRecord, resolve_device_id};

pub async fn run_china_checkin(config: &Config) -> RunReport {
    let mut report = RunReport::default();
    for account in &config.accounts {
        if !account.enabled || !account.tasks.china_game_checkin {
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
                    "网络初始化",
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
                    "网络初始化",
                    "HTTP 客户端",
                    TaskOutcome::NetworkFailed,
                    &error.to_string(),
                ));
                continue;
            }
        };

        let cookie = account.credentials.cookie.clone();
        let device_id = resolve_device_id(&account.device.id, cookie.expose_secret());
        let captcha = config
            .captcha
            .endpoint
            .clone()
            .map(|endpoint| CaptchaClient::new(http.clone(), endpoint));
        let client =
            ChinaCheckinClient::new(http, SecretString::new(cookie.expose_secret()), device_id);
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);

        for game in account.games.iter().filter_map(config_game_to_china) {
            run_game(
                &mut report,
                &account.name,
                &client,
                captcha.as_ref(),
                &mut signer,
                game,
            )
            .await;
        }
    }
    report
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
        let client = HoyolabCheckinClient::new(http, account.credentials.cookie.clone());
        for game in account.games.iter().filter_map(config_game_to_hoyolab) {
            let subject = game.spec().display_name;
            match client.info(game).await {
                Ok(CheckinState::FirstBind) => report.push(record(
                    &account.name,
                    "HoYoLAB 签到",
                    subject,
                    TaskOutcome::Skipped,
                    "首次绑定，请先手动签到一次",
                )),
                Ok(CheckinState::AlreadySigned { total_sign_day }) => report.push(record(
                    &account.name,
                    "HoYoLAB 签到",
                    subject,
                    TaskOutcome::AlreadyCompleted,
                    &format!("今日已签到，累计 {total_sign_day} 天"),
                )),
                Ok(CheckinState::Pending { .. }) => match client.sign_once(game).await {
                    Ok(SignState::Success) => report.push(record(
                        &account.name,
                        "HoYoLAB 签到",
                        subject,
                        TaskOutcome::Success,
                        "签到成功",
                    )),
                    Ok(SignState::AlreadySigned) => report.push(record(
                        &account.name,
                        "HoYoLAB 签到",
                        subject,
                        TaskOutcome::AlreadyCompleted,
                        "接口返回今日已签到",
                    )),
                    Ok(SignState::CaptchaRequired { .. }) => report.push(record(
                        &account.name,
                        "HoYoLAB 签到",
                        subject,
                        TaskOutcome::CaptchaRequired,
                        "触发验证码，已停止重复请求",
                    )),
                    Err(error) => push_hoyolab_error(&mut report, &account.name, subject, error),
                },
                Err(error) => push_hoyolab_error(&mut report, &account.name, subject, error),
            }
        }
    }
    report
}

async fn run_game(
    report: &mut RunReport,
    account: &str,
    client: &ChinaCheckinClient,
    captcha: Option<&CaptchaClient>,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    game: ChinaGame,
) {
    let spec = game.spec();
    let roles = match client.roles(game, &signer.sign_web().to_string()).await {
        Ok(RoleState::NoRole) => {
            report.push(record(
                account,
                "国内游戏签到",
                spec.display_name,
                TaskOutcome::Skipped,
                "没有绑定角色",
            ));
            return;
        }
        Ok(RoleState::Available(roles)) => roles,
        Err(error) => {
            push_error(report, account, spec.display_name, error);
            return;
        }
    };

    for role in roles {
        let subject = format!("{} / {}", spec.display_name, mask_uid(&role.uid));
        match client
            .status(
                game,
                &role.region,
                &role.uid,
                &signer.sign_web().to_string(),
            )
            .await
        {
            Ok(CheckinState::FirstBind) => report.push(record(
                account,
                "国内游戏签到",
                &subject,
                TaskOutcome::Skipped,
                "首次绑定，请先手动签到一次",
            )),
            Ok(CheckinState::AlreadySigned { total_sign_day }) => report.push(record(
                account,
                "国内游戏签到",
                &subject,
                TaskOutcome::AlreadyCompleted,
                &format!("今日已签到，累计 {total_sign_day} 天"),
            )),
            Ok(CheckinState::Pending { .. }) => match client
                .sign_once(
                    game,
                    &role.region,
                    &role.uid,
                    &signer.sign_web().to_string(),
                    None,
                )
                .await
            {
                Ok(SignState::Success) => report.push(record(
                    account,
                    "国内游戏签到",
                    &subject,
                    TaskOutcome::Success,
                    "签到成功",
                )),
                Ok(SignState::AlreadySigned) => report.push(record(
                    account,
                    "国内游戏签到",
                    &subject,
                    TaskOutcome::AlreadyCompleted,
                    "接口返回今日已签到",
                )),
                Ok(SignState::CaptchaRequired { gt, challenge }) => {
                    solve_captcha_and_retry(
                        report,
                        account,
                        &subject,
                        client,
                        captcha,
                        signer,
                        game,
                        &role.region,
                        &role.uid,
                        &gt,
                        &challenge,
                    )
                    .await;
                }
                Err(error) => push_error(report, account, &subject, error),
            },
            Err(error) => push_error(report, account, &subject, error),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_mask_only_keeps_last_four_characters() {
        assert_eq!(mask_uid("123456789"), "***6789");
        assert_eq!(mask_uid("12"), "***12");
    }

    #[tokio::test]
    async fn missing_captcha_endpoint_is_reported_without_retrying() {
        let http = HttpClient::builder().build().unwrap();
        let client =
            ChinaCheckinClient::new(http, SecretString::new("cookie_token=secret"), "device-id");
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);
        let mut report = RunReport::default();

        solve_captcha_and_retry(
            &mut report,
            "account",
            "原神 / ***0001",
            &client,
            None,
            &mut signer,
            ChinaGame::Genshin,
            "cn_gf01",
            "10001",
            "gt",
            "challenge",
        )
        .await;

        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::CaptchaRequired);
        assert!(report.records[0].message.contains("captcha.endpoint"));
    }
}

#[allow(clippy::too_many_arguments)]
async fn solve_captcha_and_retry(
    report: &mut RunReport,
    account: &str,
    subject: &str,
    client: &ChinaCheckinClient,
    captcha: Option<&CaptchaClient>,
    signer: &mut DsSigner<SystemClock, ThreadRandom>,
    game: ChinaGame,
    region: &str,
    uid: &str,
    gt: &str,
    challenge: &str,
) {
    let Some(captcha) = captcha else {
        report.push(record(
            account,
            "国内游戏签到",
            subject,
            TaskOutcome::CaptchaRequired,
            "触发验证码，但未配置 captcha.endpoint",
        ));
        return;
    };

    let solution = match captcha.solve(gt, challenge).await {
        Ok(solution) => solution,
        Err(error) => {
            report.push(record(
                account,
                "国内游戏签到",
                subject,
                TaskOutcome::CaptchaRequired,
                &format!("验证码平台求解失败：{error}"),
            ));
            return;
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
        Ok(SignState::Success) => report.push(record(
            account,
            "国内游戏签到",
            subject,
            TaskOutcome::Success,
            "验证码通过，签到成功",
        )),
        Ok(SignState::AlreadySigned) => report.push(record(
            account,
            "国内游戏签到",
            subject,
            TaskOutcome::AlreadyCompleted,
            "验证码通过，接口返回今日已签到",
        )),
        Ok(SignState::CaptchaRequired { .. }) => report.push(record(
            account,
            "国内游戏签到",
            subject,
            TaskOutcome::CaptchaRequired,
            "验证码校验后仍被要求验证，已停止重试",
        )),
        Err(error) => push_error(report, account, subject, error),
    }
}
