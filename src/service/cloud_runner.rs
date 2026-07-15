use std::time::Duration;

use rand::Rng;

use crate::{
    auth::SecretString,
    cloud_game::{CloudGame, CloudGameClient, CloudGameError, CloudWallet},
    config::{AccountConfig, Config},
    http::{HttpClient, RetryPolicy},
};

use super::{RunReport, TaskOutcome, TaskRecord};

pub async fn run_cloud_games(
    config: &Config,
    china_selected: bool,
    overseas_selected: bool,
) -> RunReport {
    let mut report = RunReport::default();
    for account in &config.accounts {
        let run_china = china_selected && account.tasks.china_cloud_game;
        let run_overseas = overseas_selected && account.tasks.overseas_cloud_game;
        if !account.enabled || (!run_china && !run_overseas) {
            continue;
        }
        let http = match build_http(config, account) {
            Ok(http) => http,
            Err(message) => {
                report.push(record(
                    account,
                    "云游戏",
                    "HTTP 客户端",
                    TaskOutcome::NetworkFailed,
                    message,
                ));
                continue;
            }
        };
        let client = CloudGameClient::new(http);

        if run_china {
            let china = &account.cloud_games.china;
            if !china.genshin.enabled && !china.zenless_zone_zero.enabled {
                report.push(record(
                    account,
                    "国内云游戏",
                    "配置",
                    TaskOutcome::Skipped,
                    "未启用任何国内云游戏",
                ));
            }
            if china.genshin.enabled {
                run_one(
                    &mut report,
                    account,
                    &client,
                    "国内云游戏",
                    CloudGame::ChinaGenshin,
                    china.genshin.token.as_ref(),
                    "zh-cn",
                )
                .await;
            }
            if china.zenless_zone_zero.enabled {
                run_one(
                    &mut report,
                    account,
                    &client,
                    "国内云游戏",
                    CloudGame::ChinaZenlessZoneZero,
                    china.zenless_zone_zero.token.as_ref(),
                    "zh-cn",
                )
                .await;
            }
        }

        if run_overseas {
            let overseas = &account.cloud_games.overseas;
            if overseas.genshin.enabled {
                run_one(
                    &mut report,
                    account,
                    &client,
                    "国际服云游戏",
                    CloudGame::OverseasGenshin,
                    overseas.genshin.token.as_ref(),
                    &overseas.language,
                )
                .await;
            } else {
                report.push(record(
                    account,
                    "国际服云游戏",
                    "配置",
                    TaskOutcome::Skipped,
                    "未启用国际服云原神",
                ));
            }
        }
    }
    report
}

async fn run_one(
    report: &mut RunReport,
    account: &AccountConfig,
    client: &CloudGameClient,
    task: &str,
    game: CloudGame,
    token: Option<&SecretString>,
    language: &str,
) {
    let Some(token) = token else {
        report.push(record(
            account,
            task,
            game.display_name(),
            TaskOutcome::Failed,
            "已启用但未配置 Token",
        ));
        return;
    };
    match wallet_with_confirmation(client, game, token, language).await {
        Ok(wallet) => report.push(wallet_record(account, task, game, wallet)),
        Err(error) => {
            let outcome = match &error {
                CloudGameError::TokenInvalid => TaskOutcome::AuthenticationFailed,
                CloudGameError::Http(_) => TaskOutcome::NetworkFailed,
                CloudGameError::InvalidHeader(_)
                | CloudGameError::Api { .. }
                | CloudGameError::InvalidResponse(_) => TaskOutcome::Failed,
            };
            report.push(record(
                account,
                task,
                game.display_name(),
                outcome,
                error.to_string(),
            ));
        }
    }
}

async fn wallet_with_confirmation(
    client: &CloudGameClient,
    game: CloudGame,
    token: &SecretString,
    language: &str,
) -> Result<CloudWallet, CloudGameError> {
    let first = client.wallet(game, token, language).await?;
    if game == CloudGame::OverseasGenshin || first.sent_minutes > 0 || first.total_minutes >= 600 {
        return Ok(first);
    }
    let delay = rand::rng().random_range(3..=6);
    tokio::time::sleep(Duration::from_secs(delay)).await;
    let second = client.wallet(game, token, language).await?;
    Ok(confirm_wallet(first, second))
}

fn confirm_wallet(first: CloudWallet, mut second: CloudWallet) -> CloudWallet {
    if second.sent_minutes == 0 && second.total_minutes > first.total_minutes {
        second.sent_minutes = second.total_minutes - first.total_minutes;
    }
    second
}

fn wallet_record(
    account: &AccountConfig,
    task: &str,
    game: CloudGame,
    wallet: CloudWallet,
) -> TaskRecord {
    let outcome = if wallet.sent_minutes > 0 {
        TaskOutcome::Success
    } else {
        TaskOutcome::AlreadyCompleted
    };
    let action = if wallet.sent_minutes > 0 {
        format!("签到成功，获得 {} 分钟免费时长", wallet.sent_minutes)
    } else {
        "本次未增加免费时长，今日可能已签到或已达到免费时长上限".to_owned()
    };
    record(
        account,
        task,
        game.display_name(),
        outcome,
        format!(
            "{action}；当前免费时长 {}；畅玩卡状态：{}；{}：{}",
            format_minutes(wallet.total_minutes),
            wallet.play_card,
            game.coin_name(),
            wallet.coins
        ),
    )
}

fn build_http(config: &Config, account: &AccountConfig) -> Result<HttpClient, String> {
    HttpClient::builder()
        .timeout(Duration::from_secs(config.runtime.request_timeout_seconds))
        .retry(RetryPolicy {
            attempts: usize::try_from(config.runtime.retry_count).unwrap_or(usize::MAX),
            base_delay: Duration::from_millis(500),
        })
        .proxy(account.proxy.url.as_ref().map(SecretString::expose_secret))
        .map_err(|error| error.to_string())?
        .build()
        .map_err(|error| error.to_string())
}

fn format_minutes(minutes: u64) -> String {
    let days = minutes / (24 * 60);
    let hours = minutes % (24 * 60) / 60;
    let minutes = minutes % 60;
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}天"));
    }
    if hours > 0 {
        parts.push(format!("{hours}小时"));
    }
    if minutes > 0 || parts.is_empty() {
        parts.push(format!("{minutes}分钟"));
    }
    parts.join("")
}

fn record(
    account: &AccountConfig,
    task: &str,
    subject: &str,
    outcome: TaskOutcome,
    message: impl Into<String>,
) -> TaskRecord {
    TaskRecord {
        account: account.name.clone(),
        task: task.to_owned(),
        subject: subject.to_owned(),
        outcome,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CURRENT_CONFIG_VERSION, CaptchaConfig, CredentialConfig, DeviceConfig, NotificationsConfig,
        ProxyConfig, RuntimeConfig, TaskConfig,
    };

    fn account() -> AccountConfig {
        AccountConfig {
            name: "测试账号".to_owned(),
            remark: None,
            enabled: true,
            credentials: CredentialConfig {
                cookie: SecretString::new("cookie"),
                stoken: SecretString::new("stoken"),
            },
            device: DeviceConfig::default(),
            proxy: ProxyConfig::default(),
            cloud_games: Default::default(),
            tasks: TaskConfig::default(),
            games: Vec::new(),
        }
    }

    #[test]
    fn successful_wallet_includes_all_details() {
        let record = wallet_record(
            &account(),
            "国内云游戏",
            CloudGame::ChinaGenshin,
            CloudWallet {
                sent_minutes: 15,
                total_minutes: 1_565,
                play_card: "未开通".to_owned(),
                coins: 20,
            },
        );
        assert_eq!(record.outcome, TaskOutcome::Success);
        assert!(record.message.contains("1天2小时5分钟"));
        assert!(record.message.contains("米云币：20"));
    }

    #[test]
    fn zero_minutes_is_already_completed() {
        let record = wallet_record(
            &account(),
            "国际服云游戏",
            CloudGame::OverseasGenshin,
            CloudWallet {
                sent_minutes: 0,
                total_minutes: 0,
                play_card: "未知".to_owned(),
                coins: 0,
            },
        );
        assert_eq!(record.outcome, TaskOutcome::AlreadyCompleted);
        assert!(record.message.contains("0分钟"));
    }

    #[test]
    fn confirmation_uses_total_time_increase_when_send_field_lags() {
        let confirmed = confirm_wallet(
            CloudWallet {
                sent_minutes: 0,
                total_minutes: 100,
                play_card: "未开通".to_owned(),
                coins: 1,
            },
            CloudWallet {
                sent_minutes: 0,
                total_minutes: 115,
                play_card: "未开通".to_owned(),
                coins: 1,
            },
        );
        assert_eq!(confirmed.sent_minutes, 15);
        assert_eq!(confirmed.total_minutes, 115);
    }

    #[tokio::test]
    async fn region_filter_does_not_initialize_unselected_account() {
        let mut account = account();
        account.tasks.overseas_cloud_game = true;
        account.proxy.url = Some(SecretString::new("ftp://invalid-proxy"));
        let config = Config {
            version: CURRENT_CONFIG_VERSION,
            runtime: RuntimeConfig::default(),
            captcha: CaptchaConfig::default(),
            accounts: vec![account],
            notifications: NotificationsConfig::default(),
        };
        let report = run_cloud_games(&config, true, false).await;
        assert!(report.records.is_empty());
    }
}
