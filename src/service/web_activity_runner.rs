use crate::config::{Config, WebActivity};

use super::{RunReport, TaskOutcome, TaskRecord};

pub fn run_web_activities(config: &Config) -> RunReport {
    let mut report = RunReport::default();
    for account in &config.accounts {
        if !account.enabled || !account.tasks.web_activity.enabled {
            continue;
        }
        if account.tasks.web_activity.activities.is_empty() {
            report.push(record(
                &account.name,
                "配置",
                "已启用 Web 活动，但 activities 为空",
            ));
            continue;
        }
        for activity in &account.tasks.web_activity.activities {
            match activity {
                WebActivity::GenshinMizone => report.push(record(
                    &account.name,
                    "原神脉动联动",
                    "活动已于 2025-10-31 结束，已跳过且不会请求失效接口",
                )),
            }
        }
    }
    report
}

fn record(account: &str, subject: &str, message: &str) -> TaskRecord {
    TaskRecord {
        account: account.to_owned(),
        task: "Web 活动".to_owned(),
        subject: subject.to_owned(),
        outcome: TaskOutcome::Skipped,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        auth::SecretString,
        config::{
            AccountConfig, CURRENT_CONFIG_VERSION, CaptchaConfig, ChinaCheckinConfig,
            CloudGamesConfig, CredentialConfig, DeviceConfig, NotificationsConfig, ProxyConfig,
            RuntimeConfig, TaskConfig, WebActivityTaskConfig,
        },
    };

    use super::*;

    fn config(activities: Vec<WebActivity>) -> Config {
        Config {
            version: CURRENT_CONFIG_VERSION,
            runtime: RuntimeConfig::default(),
            captcha: CaptchaConfig::default(),
            accounts: vec![AccountConfig {
                name: "测试账号".to_owned(),
                remark: None,
                enabled: true,
                credentials: CredentialConfig {
                    cookie: SecretString::new("cookie"),
                    stoken: SecretString::new("stoken"),
                },
                device: DeviceConfig::default(),
                proxy: ProxyConfig::default(),
                china_checkin: ChinaCheckinConfig::default(),
                hoyolab: None,
                cloud_games: CloudGamesConfig::default(),
                tasks: TaskConfig {
                    web_activity: WebActivityTaskConfig {
                        enabled: true,
                        activities,
                    },
                    ..TaskConfig::default()
                },
                games: Vec::new(),
            }],
            notifications: NotificationsConfig::default(),
        }
    }

    #[test]
    fn expired_activity_is_reported_instead_of_silently_ignored() {
        let report = run_web_activities(&config(vec![WebActivity::GenshinMizone]));
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].outcome, TaskOutcome::Skipped);
        assert!(report.records[0].message.contains("2025-10-31"));
    }

    #[test]
    fn enabled_empty_list_is_explicitly_reported() {
        let report = run_web_activities(&config(Vec::new()));
        assert_eq!(report.records.len(), 1);
        assert!(report.records[0].message.contains("activities 为空"));
    }
}
