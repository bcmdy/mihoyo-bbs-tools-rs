use std::collections::HashSet;

use serde_yaml_ng::{Mapping, Value};
use url::Url;

use super::{
    AccountConfig, CURRENT_CONFIG_VERSION, CaptchaConfig, Config, ConfigError, CredentialConfig,
    DeviceConfig, Game, LoadedConfig, NotificationsConfig, ProxyConfig, RuntimeConfig, TaskConfig,
};
use crate::auth::SecretString;

pub(super) fn migrate_value(
    value: &Value,
    account_name: &str,
) -> Result<LoadedConfig, ConfigError> {
    let root = value.as_mapping().ok_or_else(|| {
        ConfigError::Validation(vec!["旧版配置的顶层必须是 YAML mapping".to_owned()])
    })?;
    let version = unsigned(root, "version").unwrap_or_default();
    if !(11..=15).contains(&version) {
        return Err(ConfigError::UnsupportedVersion(version));
    }

    let mut warnings = vec![format!(
        "已将 Python version {version} 直接迁移到统一 version {CURRENT_CONFIG_VERSION} 模型"
    )];
    if version < 15 {
        warnings.push(format!(
            "旧版 version {version} 使用直接迁移策略，并按 Python v15 的安全默认值补齐缺失字段"
        ));
    }

    let account = mapping(root, "account");
    let cookie = account
        .and_then(|map| scalar_string(map, "cookie"))
        .unwrap_or_default();
    let stoken = account
        .and_then(|map| scalar_string(map, "stoken"))
        .unwrap_or_default();
    let captcha_endpoint = account
        .and_then(|map| scalar_string(map, "CAPTCHA_ENDPOINT"))
        .filter(|value| !value.trim().is_empty())
        .and_then(|raw| match Url::parse(&raw) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => Some(url),
            _ => {
                warnings.push("account.CAPTCHA_ENDPOINT 无效，未迁移".to_owned());
                None
            }
        });
    let legacy_device = mapping(root, "device");
    let device_defaults = DeviceConfig::default();
    let device = DeviceConfig {
        id: legacy_device
            .and_then(|map| scalar_string(map, "id"))
            .unwrap_or_default(),
        name: legacy_device
            .and_then(|map| scalar_string(map, "name"))
            .unwrap_or(device_defaults.name),
        model: legacy_device
            .and_then(|map| scalar_string(map, "model"))
            .unwrap_or(device_defaults.model),
        fp: legacy_device
            .and_then(|map| scalar_string(map, "fp"))
            .unwrap_or_default(),
    };

    let games = mapping(root, "games");
    let cn = games.and_then(|map| mapping(map, "cn"));
    let overseas = games.and_then(|map| mapping(map, "os"));
    let cn_enabled = cn.and_then(|map| boolean(map, "enable")).unwrap_or(true);
    let overseas_enabled = overseas
        .and_then(|map| boolean(map, "enable"))
        .unwrap_or(false);
    let retry_count = cn
        .and_then(|map| unsigned(map, "retries"))
        .unwrap_or(3)
        .try_into()
        .unwrap_or_else(|_| {
            warnings.push("games.cn.retries 超出范围，已使用默认值 3".to_owned());
            3
        });

    let mut selected_games = Vec::new();
    let mut seen = HashSet::new();
    for (legacy_name, game) in [
        ("genshin", Game::Genshin),
        ("honkai2", Game::Honkai2),
        ("honkai3rd", Game::Honkai3rd),
        ("tears_of_themis", Game::TearsOfThemis),
        ("honkai_sr", Game::StarRail),
        ("zzz", Game::ZenlessZoneZero),
    ] {
        let enabled = (cn_enabled && game_checkin(cn, legacy_name))
            || (overseas_enabled && game_checkin(overseas, legacy_name));
        if enabled && seen.insert(game) {
            selected_games.push(game);
        }
    }

    let bbs = mapping(root, "mihoyobbs");
    let bbs_enabled = bbs.and_then(|map| boolean(map, "enable")).unwrap_or(true);
    let cloud_games = mapping(root, "cloud_games");
    let cloud_cn_enabled = cloud_games
        .and_then(|map| mapping(map, "cn"))
        .and_then(|map| boolean(map, "enable"))
        .or_else(|| {
            cloud_games
                .and_then(|map| mapping(map, "genshin"))
                .and_then(|map| boolean(map, "enable"))
        })
        .unwrap_or(false);
    let cloud_os_enabled = cloud_games
        .and_then(|map| mapping(map, "os"))
        .and_then(|map| boolean(map, "enable"))
        .unwrap_or(false);
    let web_activity_enabled = mapping(root, "web_activity")
        .and_then(|map| boolean(map, "enable"))
        .unwrap_or(false);

    warn_about_lossy_fields(root, &mut warnings);

    let config = Config {
        version: CURRENT_CONFIG_VERSION,
        runtime: RuntimeConfig {
            retry_count,
            ..RuntimeConfig::default()
        },
        captcha: CaptchaConfig {
            endpoint: captcha_endpoint,
        },
        accounts: vec![AccountConfig {
            name: account_name.trim().to_owned(),
            enabled: boolean(root, "enable").unwrap_or(true),
            credentials: CredentialConfig {
                cookie: SecretString::new(cookie),
                stoken: SecretString::new(stoken),
            },
            device,
            proxy: ProxyConfig::default(),
            tasks: TaskConfig {
                china_game_checkin: cn_enabled,
                hoyolab_checkin: overseas_enabled,
                bbs: bbs_enabled,
                china_cloud_game: cloud_cn_enabled,
                overseas_cloud_game: cloud_os_enabled,
                web_activity: web_activity_enabled,
            },
            games: selected_games,
        }],
        notifications: NotificationsConfig::default(),
    };

    Ok(LoadedConfig { config, warnings })
}

fn warn_about_lossy_fields(root: &Mapping, warnings: &mut Vec<String>) {
    if mapping(root, "competition").is_some() {
        warnings.push("旧版 competition 功能已移除，未迁移".to_owned());
    }
    if scalar_string(root, "push").is_some_and(|value| !value.trim().is_empty()) {
        warnings.push("旧版 push.ini 文件引用无法转换为内联通知配置，未迁移".to_owned());
    }
    if let Some(games) = mapping(root, "games") {
        if mapping(games, "os")
            .and_then(|map| scalar_string(map, "cookie"))
            .is_some_and(|value| !value.is_empty())
        {
            warnings.push("旧版国际服独立 Cookie 尚无对应凭据字段，未迁移".to_owned());
        }
        if has_blacklists(games) {
            warnings.push("旧版游戏角色黑名单尚无对应的新模型，未迁移".to_owned());
        }
    }
    if has_cloud_tokens(root) {
        warnings.push("旧版云游戏 Token 尚无对应的新模型，未迁移".to_owned());
    }
}

fn has_blacklists(games: &Mapping) -> bool {
    ["cn", "os"].iter().any(|region| {
        mapping(games, region).is_some_and(|region| {
            region.values().any(|game| {
                game.as_mapping()
                    .and_then(|game| get(game, "black_list"))
                    .and_then(Value::as_sequence)
                    .is_some_and(|items| !items.is_empty())
            })
        })
    })
}

fn has_cloud_tokens(root: &Mapping) -> bool {
    mapping(root, "cloud_games").is_some_and(|cloud| {
        cloud.values().any(|region_or_game| {
            region_or_game.as_mapping().is_some_and(|map| {
                scalar_string(map, "token").is_some_and(|token| !token.is_empty())
                    || map.values().any(|game| {
                        game.as_mapping()
                            .and_then(|game| scalar_string(game, "token"))
                            .is_some_and(|token| !token.is_empty())
                    })
            })
        })
    })
}

fn game_checkin(region: Option<&Mapping>, game: &str) -> bool {
    region
        .and_then(|region| mapping(region, game))
        .and_then(|game| boolean(game, "checkin"))
        .unwrap_or(false)
}

fn mapping<'a>(map: &'a Mapping, key: &str) -> Option<&'a Mapping> {
    get(map, key).and_then(Value::as_mapping)
}

fn boolean(map: &Mapping, key: &str) -> Option<bool> {
    get(map, key).and_then(Value::as_bool)
}

fn unsigned(map: &Mapping, key: &str) -> Option<u64> {
    let value = get(map, key)?;
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn scalar_string(map: &Mapping, key: &str) -> Option<String> {
    let value = get(map, key)?;
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn get<'a>(map: &'a Mapping, key: &str) -> Option<&'a Value> {
    map.get(Value::String(key.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_v15_fixture_without_exposing_secrets() {
        let value: Value =
            serde_yaml_ng::from_str(include_str!("fixtures/legacy_v15.yaml")).unwrap();
        let migrated = migrate_value(&value, "legacy-v15").unwrap();
        let account = &migrated.config.accounts[0];
        assert_eq!(account.name, "legacy-v15");
        assert!(account.tasks.china_game_checkin);
        assert!(account.tasks.bbs);
        assert_eq!(account.games, vec![Game::Genshin, Game::StarRail]);
        assert_eq!(account.device.id, "fixture-device-id");
        assert_eq!(account.device.name, "Fixture Device");
        assert_eq!(account.device.model, "Fixture Model");
        assert_eq!(account.device.fp, "fixture-device-fp");
        assert_eq!(migrated.config.runtime.retry_count, 4);
        assert!(!format!("{:?}", migrated.config).contains("fixture-cookie-token"));
        assert!(!migrated
            .warnings
            .iter()
            .any(|warning| warning.contains("device")));
    }

    #[test]
    fn all_supported_legacy_versions_use_direct_migration() {
        for version in 11..=15 {
            let source = include_str!("fixtures/legacy_v11.yaml")
                .replace("version: 11", &format!("version: {version}"));
            let value: Value = serde_yaml_ng::from_str(&source).unwrap();
            let migrated = migrate_value(&value, &format!("v{version}")).unwrap();
            assert_eq!(migrated.config.version, CURRENT_CONFIG_VERSION);
            assert_eq!(migrated.config.accounts[0].name, format!("v{version}"));
            if version < 15 {
                assert!(
                    migrated
                        .warnings
                        .iter()
                        .any(|warning| warning.contains("安全默认值"))
                );
            }
        }
    }

    #[test]
    fn v11_cloud_game_shape_is_recognized() {
        let value: Value =
            serde_yaml_ng::from_str(include_str!("fixtures/legacy_v11.yaml")).unwrap();
        let migrated = migrate_value(&value, "v11").unwrap();
        assert!(migrated.config.accounts[0].tasks.china_cloud_game);
        assert!(
            migrated
                .warnings
                .iter()
                .any(|warning| warning.contains("云游戏 Token"))
        );
    }
}
