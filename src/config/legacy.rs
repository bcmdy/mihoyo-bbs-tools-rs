use std::collections::HashSet;

use serde_yaml_ng::{Mapping, Value};
use url::Url;

use super::{
    AccountConfig, CURRENT_CONFIG_VERSION, CaptchaConfig, ChinaCheckinConfig,
    ChinaCloudGamesConfig, CloudGameEntryConfig, CloudGamesConfig, Config, ConfigError,
    ConfigSource, CredentialConfig, DeviceConfig, Game, HoyolabConfig, LoadedConfig,
    NotificationsConfig, OverseasCloudGamesConfig, ProxyConfig, RoleBlacklistConfig, RuntimeConfig,
    TaskConfig, WebActivity, WebActivityTaskConfig,
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
        name: legacy_device
            .and_then(|map| scalar_string(map, "name"))
            .unwrap_or(device_defaults.name),
        model: legacy_device
            .and_then(|map| scalar_string(map, "model"))
            .unwrap_or(device_defaults.model),
        id: legacy_device
            .and_then(|map| scalar_string(map, "id"))
            .unwrap_or_default(),
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

    let selected_games = legacy_selected_games(cn, cn_enabled, true);
    let hoyolab_games = legacy_selected_games(overseas, overseas_enabled, false);
    let china_checkin = ChinaCheckinConfig {
        user_agent: cn
            .and_then(|map| scalar_string(map, "useragent"))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(super::default_china_user_agent),
        role_blacklist: migrate_role_blacklist(cn, &mut warnings),
    };
    let hoyolab = HoyolabConfig {
        cookie: SecretString::new(
            overseas
                .and_then(|map| scalar_string(map, "cookie"))
                .unwrap_or_default(),
        ),
        language: migrate_hoyolab_language(overseas, &mut warnings),
        user_agent: super::default_hoyolab_user_agent(),
        games: hoyolab_games,
    };

    let bbs = mapping(root, "mihoyobbs");
    let bbs_enabled = bbs.and_then(|map| boolean(map, "enable")).unwrap_or(true);
    let bbs_forums = legacy_bbs_forums(bbs, &mut warnings);
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
    let legacy_cloud_cn = cloud_games.and_then(|map| mapping(map, "cn"));
    let cloud_genshin = legacy_cloud_cn
        .and_then(|map| mapping(map, "genshin"))
        .or_else(|| cloud_games.and_then(|map| mapping(map, "genshin")));
    let cloud_zzz = legacy_cloud_cn.and_then(|map| mapping(map, "zzz"));
    let legacy_cloud_os = cloud_games.and_then(|map| mapping(map, "os"));
    let cloud_os_genshin = legacy_cloud_os.and_then(|map| mapping(map, "genshin"));
    let cloud_games_config = CloudGamesConfig {
        china: ChinaCloudGamesConfig {
            genshin: migrate_cloud_entry(cloud_genshin, "cloud_games.cn.genshin", &mut warnings),
            zenless_zone_zero: migrate_cloud_entry(cloud_zzz, "cloud_games.cn.zzz", &mut warnings),
        },
        overseas: OverseasCloudGamesConfig {
            language: migrate_cloud_language(legacy_cloud_os, &mut warnings),
            genshin: migrate_cloud_entry(cloud_os_genshin, "cloud_games.os.genshin", &mut warnings),
        },
    };
    let web_activity = migrate_web_activity(root, &mut warnings);

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
            remark: None,
            enabled: boolean(root, "enable").unwrap_or(true),
            credentials: CredentialConfig {
                cookie: SecretString::new(cookie),
                stoken: SecretString::new(stoken),
            },
            device,
            proxy: ProxyConfig::default(),
            china_checkin,
            hoyolab: Some(hoyolab),
            cloud_games: cloud_games_config,
            tasks: TaskConfig {
                china_game_checkin: cn_enabled,
                hoyolab_checkin: overseas_enabled,
                bbs: super::BbsTaskConfig {
                    enabled: bbs_enabled,
                    sign: bbs.and_then(|map| boolean(map, "checkin")).unwrap_or(true),
                    forums: bbs_forums,
                    read: bbs.and_then(|map| boolean(map, "read")).unwrap_or(true),
                    like: bbs.and_then(|map| boolean(map, "like")).unwrap_or(true),
                    cancel_like: bbs
                        .and_then(|map| boolean(map, "cancel_like"))
                        .unwrap_or(true),
                    share: bbs.and_then(|map| boolean(map, "share")).unwrap_or(true),
                },
                china_cloud_game: cloud_cn_enabled,
                overseas_cloud_game: cloud_os_enabled,
                web_activity,
            },
            games: selected_games,
        }],
        notifications: NotificationsConfig::default(),
    };

    Ok(LoadedConfig {
        config,
        warnings,
        source: ConfigSource::PythonLegacy(version),
    })
}

fn legacy_bbs_forums(bbs: Option<&Mapping>, warnings: &mut Vec<String>) -> Vec<u8> {
    let defaults = super::default_bbs_forums();
    let Some(value) = bbs.and_then(|map| get(map, "checkin_list")) else {
        return defaults;
    };
    let Some(values) = value.as_sequence() else {
        warnings.push("mihoyobbs.checkin_list 不是列表，已使用默认值 [5, 2]".to_owned());
        return defaults;
    };

    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for value in values {
        let id = value
            .as_u64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            .and_then(|value| u8::try_from(value).ok());
        match id {
            Some(id) if crate::bbs::forum_by_id(id).is_some() && seen.insert(id) => {
                selected.push(id);
            }
            Some(id) if crate::bbs::forum_by_id(id).is_some() => {
                warnings.push(format!(
                    "mihoyobbs.checkin_list 中的板块 ID {id} 重复，已去重"
                ));
            }
            Some(id) => warnings.push(format!(
                "mihoyobbs.checkin_list 中的板块 ID {id} 不受支持，未迁移"
            )),
            None => warnings.push("mihoyobbs.checkin_list 包含无效板块 ID，未迁移".to_owned()),
        }
    }
    if selected.is_empty() {
        warnings.push("mihoyobbs.checkin_list 没有有效板块，已使用默认值 [5, 2]".to_owned());
        defaults
    } else {
        selected
    }
}

fn warn_about_lossy_fields(root: &Mapping, warnings: &mut Vec<String>) {
    if mapping(root, "competition").is_some() {
        warnings.push("旧版 competition 功能已移除，未迁移".to_owned());
    }
    if scalar_string(root, "push").is_some_and(|value| !value.trim().is_empty()) {
        warnings.push("旧版 push.ini 文件引用无法转换为内联通知配置，未迁移".to_owned());
    }
    if let Some(games) = mapping(root, "games") {
        if mapping(games, "os").is_some_and(has_blacklists) {
            warnings.push(
                "旧版国际服角色黑名单未迁移：原 Python 国际服签到接口未按 UID 执行黑名单"
                    .to_owned(),
            );
        }
    }
}

fn has_blacklists(region: &Mapping) -> bool {
    region.values().any(|game| {
        game.as_mapping()
            .and_then(|game| get(game, "black_list"))
            .and_then(Value::as_sequence)
            .is_some_and(|items| !items.is_empty())
    })
}

fn legacy_selected_games(
    region: Option<&Mapping>,
    enabled: bool,
    include_honkai2: bool,
) -> Vec<Game> {
    if !enabled {
        return Vec::new();
    }
    [
        ("genshin", Game::Genshin),
        ("honkai2", Game::Honkai2),
        ("honkai3rd", Game::Honkai3rd),
        ("tears_of_themis", Game::TearsOfThemis),
        ("honkai_sr", Game::StarRail),
        ("zzz", Game::ZenlessZoneZero),
    ]
    .into_iter()
    .filter(|(_, game)| include_honkai2 || *game != Game::Honkai2)
    .filter(|(name, _)| game_checkin(region, name))
    .map(|(_, game)| game)
    .collect()
}

fn migrate_role_blacklist(
    region: Option<&Mapping>,
    warnings: &mut Vec<String>,
) -> RoleBlacklistConfig {
    RoleBlacklistConfig {
        genshin: legacy_blacklist(region, "genshin", warnings),
        honkai2: legacy_blacklist(region, "honkai2", warnings),
        honkai3rd: legacy_blacklist(region, "honkai3rd", warnings),
        tears_of_themis: legacy_blacklist(region, "tears_of_themis", warnings),
        star_rail: legacy_blacklist(region, "honkai_sr", warnings),
        zenless_zone_zero: legacy_blacklist(region, "zzz", warnings),
    }
}

fn legacy_blacklist(
    region: Option<&Mapping>,
    game: &str,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let Some(items) = region
        .and_then(|region| mapping(region, game))
        .and_then(|game| get(game, "black_list"))
        .and_then(Value::as_sequence)
    else {
        return Vec::new();
    };
    let mut output = Vec::new();
    for item in items {
        let Some(uid) = (match item {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        }) else {
            warnings.push(format!(
                "旧版 games.cn.{game}.black_list 包含无效 UID，已忽略"
            ));
            continue;
        };
        let uid = uid.trim();
        if uid.is_empty() {
            warnings.push(format!(
                "旧版 games.cn.{game}.black_list 包含空 UID，已忽略"
            ));
        } else if !output.iter().any(|value| value == uid) {
            output.push(uid.to_owned());
        }
    }
    output
}

fn migrate_hoyolab_language(value: Option<&Mapping>, warnings: &mut Vec<String>) -> String {
    let language = value
        .and_then(|map| scalar_string(map, "lang"))
        .unwrap_or_else(super::default_hoyolab_language);
    if matches!(language.as_str(), "zh-cn" | "en-us" | "ja-jp" | "ko-kr") {
        language
    } else {
        warnings.push(format!(
            "旧版 games.os.lang={language:?} 不受支持，已改为 zh-cn"
        ));
        super::default_hoyolab_language()
    }
}

fn migrate_web_activity(root: &Mapping, warnings: &mut Vec<String>) -> WebActivityTaskConfig {
    let web = mapping(root, "web_activity");
    let enabled = web.and_then(|map| boolean(map, "enable")).unwrap_or(false);
    let mut activities = Vec::new();
    if let Some(values) = web
        .and_then(|map| get(map, "activities"))
        .and_then(Value::as_sequence)
    {
        for value in values {
            match value.as_str() {
                Some("genshin_mizone") if !activities.contains(&WebActivity::GenshinMizone) => {
                    activities.push(WebActivity::GenshinMizone);
                }
                Some("genshin_mizone") => {
                    warnings.push("旧版 web_activity.activities 包含重复活动，已去重".to_owned())
                }
                Some(name) => warnings.push(format!("旧版 Web 活动 {name:?} 不受支持，未迁移")),
                None => warnings
                    .push("旧版 web_activity.activities 包含无效活动名称，未迁移".to_owned()),
            }
        }
    }
    WebActivityTaskConfig {
        enabled,
        activities,
    }
}

fn migrate_cloud_entry(
    value: Option<&Mapping>,
    path: &str,
    warnings: &mut Vec<String>,
) -> CloudGameEntryConfig {
    let requested = value
        .and_then(|map| boolean(map, "enable"))
        .unwrap_or(false);
    let token = value
        .and_then(|map| scalar_string(map, "token"))
        .filter(|token| !token.trim().is_empty())
        .map(SecretString::new);
    if requested && token.is_none() {
        warnings.push(format!("旧版 {path} 已启用但 Token 为空，已关闭该云游戏"));
    }
    CloudGameEntryConfig {
        enabled: requested && token.is_some(),
        token,
    }
}

fn migrate_cloud_language(value: Option<&Mapping>, warnings: &mut Vec<String>) -> String {
    let language = value
        .and_then(|map| scalar_string(map, "lang"))
        .unwrap_or_else(super::default_hoyolab_language);
    if matches!(language.as_str(), "zh-cn" | "en-us" | "ja-jp" | "ko-kr") {
        language
    } else {
        warnings.push(format!(
            "旧版 cloud_games.os.lang={language:?} 不受支持，已改为 zh-cn"
        ));
        super::default_hoyolab_language()
    }
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
        assert!(account.tasks.bbs.enabled);
        assert_eq!(account.tasks.bbs.forums, vec![5, 2]);
        assert_eq!(account.games, vec![Game::Genshin, Game::StarRail]);
        assert_eq!(account.device.id, "fixture-device-id");
        assert_eq!(account.device.name, "Fixture Device");
        assert_eq!(account.device.model, "Fixture Model");
        assert_eq!(account.device.fp, "fixture-device-fp");
        assert_eq!(migrated.config.runtime.retry_count, 4);
        assert_eq!(migrated.config.runtime.task_max_attempts, 3);
        assert!(!format!("{:?}", migrated.config).contains("fixture-cookie-token"));
        assert!(
            !migrated
                .warnings
                .iter()
                .any(|warning| warning.contains("device"))
        );
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
        let cloud = &migrated.config.accounts[0].cloud_games.china.genshin;
        assert!(cloud.enabled);
        assert_eq!(
            cloud.token.as_ref().unwrap().expose_secret(),
            "fixture-cloud-token"
        );
        assert!(
            !migrated
                .warnings
                .iter()
                .any(|warning| warning.contains("Token 尚无对应"))
        );
        assert_eq!(
            migrated.config.accounts[0]
                .china_checkin
                .role_blacklist
                .genshin,
            vec!["999999"]
        );
    }

    #[test]
    fn migrates_region_specific_games_cookie_language_and_user_agent() {
        let source = r#"
version: 15
account:
  cookie: domestic-cookie
  stoken: domestic-stoken
games:
  cn:
    enable: true
    useragent: custom-cn-agent
    genshin:
      checkin: true
      black_list: [10001]
    honkai_sr:
      checkin: false
  os:
    enable: true
    cookie: overseas-cookie
    lang: ja-jp
    genshin:
      checkin: false
    honkai_sr:
      checkin: true
      black_list: [20002]
"#;
        let value: Value = serde_yaml_ng::from_str(source).unwrap();
        let migrated = migrate_value(&value, "regions").unwrap();
        let account = &migrated.config.accounts[0];
        assert_eq!(account.games, vec![Game::Genshin]);
        assert_eq!(account.china_checkin.user_agent, "custom-cn-agent");
        assert_eq!(account.china_checkin.role_blacklist.genshin, vec!["10001"]);
        let hoyolab = account.hoyolab.as_ref().unwrap();
        assert_eq!(hoyolab.games, vec![Game::StarRail]);
        assert_eq!(hoyolab.language, "ja-jp");
        assert_eq!(hoyolab.cookie.expose_secret(), "overseas-cookie");
        assert!(!format!("{account:?}").contains("overseas-cookie"));
        assert!(
            migrated
                .warnings
                .iter()
                .any(|warning| warning.contains("国际服角色黑名单未迁移"))
        );
        assert!(
            !migrated
                .warnings
                .iter()
                .any(|warning| warning.contains("独立 Cookie"))
        );
    }

    #[test]
    fn migrates_v15_cloud_tokens_and_language() {
        let source = r#"
version: 15
account:
  cookie: fixture-cookie
  stoken: fixture-stoken
cloud_games:
  cn:
    enable: true
    genshin:
      enable: true
      token: fixture-cn-token
    zzz:
      enable: true
      token: fixture-zzz-token
  os:
    enable: true
    lang: en-us
    genshin:
      enable: true
      token: fixture-os-token
"#;
        let value: Value = serde_yaml_ng::from_str(source).unwrap();
        let migrated = migrate_value(&value, "v15").unwrap();
        let cloud = &migrated.config.accounts[0].cloud_games;
        assert!(migrated.config.accounts[0].tasks.china_cloud_game);
        assert!(migrated.config.accounts[0].tasks.overseas_cloud_game);
        assert!(cloud.china.genshin.enabled);
        assert!(cloud.china.zenless_zone_zero.enabled);
        assert!(cloud.overseas.genshin.enabled);
        assert_eq!(cloud.overseas.language, "en-us");
        assert_eq!(
            cloud
                .china
                .genshin
                .token
                .as_ref()
                .map(SecretString::expose_secret),
            Some("fixture-cn-token")
        );
        assert_eq!(
            cloud
                .china
                .zenless_zone_zero
                .token
                .as_ref()
                .map(SecretString::expose_secret),
            Some("fixture-zzz-token")
        );
        assert_eq!(
            cloud
                .overseas
                .genshin
                .token
                .as_ref()
                .map(SecretString::expose_secret),
            Some("fixture-os-token")
        );
    }

    #[test]
    fn migrates_known_web_activity_and_warns_about_unknown_names() {
        let source = r#"
version: 15
account:
  cookie: fixture-cookie
  stoken: fixture-stoken
mihoyobbs:
  enable: false
web_activity:
  enable: true
  activities: [genshin_mizone, unknown_activity]
"#;
        let value: Value = serde_yaml_ng::from_str(source).unwrap();
        let migrated = migrate_value(&value, "web").unwrap();
        let web = &migrated.config.accounts[0].tasks.web_activity;
        assert!(web.enabled);
        assert_eq!(web.activities, vec![WebActivity::GenshinMizone]);
        assert!(
            migrated
                .warnings
                .iter()
                .any(|warning| warning.contains("unknown_activity"))
        );
    }

    #[test]
    fn migrates_and_sanitizes_legacy_bbs_forums() {
        let source = include_str!("fixtures/legacy_v15.yaml")
            .replace("checkin_list: [5, 2]", "checkin_list: [1, '6', 7, 1]");
        let value: Value = serde_yaml_ng::from_str(&source).unwrap();
        let migrated = migrate_value(&value, "legacy-forums").unwrap();
        assert_eq!(migrated.config.accounts[0].tasks.bbs.forums, vec![1, 6]);
        assert!(
            migrated
                .warnings
                .iter()
                .any(|warning| warning.contains("ID 7") && warning.contains("不受支持"))
        );
        assert!(
            migrated
                .warnings
                .iter()
                .any(|warning| warning.contains("ID 1") && warning.contains("重复"))
        );
    }
}
