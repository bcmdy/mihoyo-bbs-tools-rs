use std::{env, fs, path::Path};

use serde_json::Value;

use super::{
    AccountConfig, BbsTaskConfig, CaptchaConfig, ChinaCheckinConfig, ChinaCloudGamesConfig,
    CloudGameEntryConfig, CloudGamesConfig, Config, ConfigError, ConfigSource, CredentialConfig,
    DeviceConfig, Game, HoyolabConfig, LoadedConfig, NotificationProvider, NotificationsConfig,
    OverseasCloudGamesConfig, ProxyConfig, RoleBlacklistConfig, RuntimeConfig, SmtpTlsMode,
    TaskConfig, WebActivity, WebActivityTaskConfig, default_china_user_agent,
    default_hoyolab_user_agent, expand_string, validate,
};
use crate::auth::{CookieJar, SecretString};
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum DacapoError {
    #[error("无法读取 DaCapo 配置 {path}: {source}")]
    Read {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("DaCapo JSON 无效: {0}")]
    Json(#[from] serde_json::Error),
    #[error("DaCapo 字段 {field} 无效：{message}")]
    Field {
        field: &'static str,
        message: &'static str,
    },
    #[error("DaCapo Cookie 格式无效")]
    InvalidCookie,
    #[error(transparent)]
    Config(#[from] ConfigError),
}

pub fn load_dacapo(path: &Path) -> Result<LoadedConfig, DacapoError> {
    let source = fs::read_to_string(path).map_err(|source| DacapoError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let account_name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(|name| format!("DaCapo:{}", name.trim()))
        .unwrap_or_else(|| "DaCapo:account".to_owned());
    parse_dacapo(&source, &account_name)
}

fn parse_dacapo(source: &str, account_name: &str) -> Result<LoadedConfig, DacapoError> {
    let mut root: Value = serde_json::from_str(source)?;
    expand_environment(&mut root)?;
    let access = Access::new(&root);

    let cookie = access
        .string(
            &["Project", "General", "账号配置", "米游社Cookie"],
            "米游社Cookie",
        )?
        .filter(|value| !value.is_empty())
        .ok_or(DacapoError::Field {
            field: "米游社Cookie",
            message: "不能为空",
        })?;
    let stuid = access
        .string(&["Project", "General", "账号配置", "stuid"], "stuid")?
        .unwrap_or_default();
    let mid = access
        .string(&["Project", "General", "账号配置", "mid"], "mid")?
        .unwrap_or_default();
    let separate_stoken = access
        .string(&["Project", "General", "账号配置", "stoken"], "stoken")?
        .unwrap_or_default();
    let (cookie, stoken) = merge_credentials(&cookie, &stuid, &mid, &separate_stoken)?;

    let retries = access.u64(
        &["日常", "米游社", "国服游戏", "重试次数"],
        "国服游戏.重试次数",
        3,
    )?;
    let retries = u32::try_from(retries).map_err(|_| DacapoError::Field {
        field: "国服游戏.重试次数",
        message: "必须在 1 到 10 之间",
    })?;
    if !(1..=10).contains(&retries) {
        return Err(DacapoError::Field {
            field: "国服游戏.重试次数",
            message: "必须在 1 到 10 之间",
        });
    }

    let bbs = build_bbs(&access)?;
    let china_enabled = access.boolean(
        &["日常", "米游社", "国服游戏", "启用国服签到"],
        "国服游戏.启用国服签到",
        true,
    )?;
    let china_games = build_china_games(&access)?;
    let china_checkin = ChinaCheckinConfig {
        user_agent: access
            .string(
                &["日常", "米游社", "国服游戏", "User Agent"],
                "国服游戏.User Agent",
            )?
            .filter(|value| !value.is_empty())
            .unwrap_or_else(default_china_user_agent),
        role_blacklist: build_china_blacklist(&access)?,
    };

    let hoyolab_enabled = access.boolean(
        &["日常", "米游社", "国际服游戏", "启用国际服签到"],
        "国际服游戏.启用国际服签到",
        false,
    )?;
    let hoyolab = HoyolabConfig {
        cookie: SecretString::new(
            access
                .string(
                    &["日常", "米游社", "国际服游戏", "国际服Cookie"],
                    "国际服游戏.国际服Cookie",
                )?
                .unwrap_or_default(),
        ),
        language: access
            .string(
                &["日常", "米游社", "国际服游戏", "语言设置"],
                "国际服游戏.语言设置",
            )?
            .unwrap_or_else(|| "zh-cn".to_owned()),
        user_agent: default_hoyolab_user_agent(),
        games: build_hoyolab_games(&access)?,
    };

    let (cloud_games, china_cloud_game, overseas_cloud_game) = build_cloud_games(&access)?;
    let web_activity = build_web_activity(&access)?;
    let notifications = build_notifications(&access)?;
    let mut warnings =
        vec!["DaCapo JSON 已在内存中转换；凭据刷新只对本轮有效，不会写回输入文件".to_owned()];
    if has_hoyolab_blacklist(&access)? {
        warnings.push(
            "DaCapo 国际服 UID 黑名单未迁移：原 Python 国际服签到也未执行这些黑名单".to_owned(),
        );
    }

    let config = Config {
        version: super::CURRENT_CONFIG_VERSION,
        runtime: RuntimeConfig {
            game_checkin_max_attempts: retries,
            ..RuntimeConfig::default()
        },
        captcha: CaptchaConfig::default(),
        accounts: vec![AccountConfig {
            name: account_name.to_owned(),
            remark: Some("DaCapo".to_owned()),
            enabled: true,
            credentials: CredentialConfig {
                cookie: SecretString::new(cookie),
                stoken: SecretString::new(stoken),
            },
            device: build_device(&access)?,
            proxy: ProxyConfig::default(),
            china_checkin,
            hoyolab: Some(hoyolab),
            cloud_games,
            tasks: TaskConfig {
                china_game_checkin: china_enabled,
                hoyolab_checkin: hoyolab_enabled,
                bbs,
                china_cloud_game,
                overseas_cloud_game,
                web_activity,
            },
            games: china_games,
        }],
        notifications,
    };
    validate(&config)?;
    Ok(LoadedConfig {
        config,
        warnings,
        source: ConfigSource::Dacapo,
    })
}

fn merge_credentials(
    cookie: &str,
    stuid: &str,
    mid: &str,
    separate_stoken: &str,
) -> Result<(String, String), DacapoError> {
    let mut jar = CookieJar::parse(cookie).map_err(|_| DacapoError::InvalidCookie)?;
    if jar.is_empty() {
        return Err(DacapoError::InvalidCookie);
    }
    if jar.uid().is_none() && !stuid.is_empty() {
        if !stuid.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(DacapoError::Field {
                field: "stuid",
                message: "必须只包含数字",
            });
        }
        jar.insert("account_id", stuid.to_owned())
            .map_err(|_| DacapoError::InvalidCookie)?;
    }
    if jar.mid().is_none() && !mid.is_empty() {
        jar.insert("account_mid_v2", mid.to_owned())
            .map_err(|_| DacapoError::InvalidCookie)?;
    }
    let stoken = if separate_stoken.is_empty() {
        jar.get("stoken").unwrap_or_default().to_owned()
    } else {
        separate_stoken.to_owned()
    };
    Ok((jar.to_header(), stoken))
}

fn build_device(access: &Access<'_>) -> Result<DeviceConfig, DacapoError> {
    let defaults = DeviceConfig::default();
    Ok(DeviceConfig {
        name: access
            .string(
                &["Project", "General", "设备信息", "设备名称"],
                "设备信息.设备名称",
            )?
            .filter(|value| !value.is_empty())
            .unwrap_or(defaults.name),
        model: access
            .string(
                &["Project", "General", "设备信息", "设备型号"],
                "设备信息.设备型号",
            )?
            .filter(|value| !value.is_empty())
            .unwrap_or(defaults.model),
        id: access
            .string(
                &["Project", "General", "设备信息", "设备ID"],
                "设备信息.设备ID",
            )?
            .unwrap_or_default(),
        fp: access
            .string(
                &["Project", "General", "设备信息", "设备指纹"],
                "设备信息.设备指纹",
            )?
            .unwrap_or_default(),
    })
}

fn build_bbs(access: &Access<'_>) -> Result<BbsTaskConfig, DacapoError> {
    let enabled = access.boolean(
        &["日常", "米游社", "米游社BBS", "启用米游社签到"],
        "米游社BBS.启用米游社签到",
        true,
    )?;
    Ok(BbsTaskConfig {
        enabled,
        sign: access.boolean(
            &["日常", "米游社", "米游社BBS", "启用版块签到"],
            "米游社BBS.启用版块签到",
            true,
        )?,
        forums: access.forums(
            &["日常", "米游社", "米游社BBS", "签到版块列表"],
            "米游社BBS.签到版块列表",
        )?,
        read: access.boolean(
            &["日常", "米游社", "米游社BBS", "启用看帖"],
            "米游社BBS.启用看帖",
            true,
        )?,
        like: access.boolean(
            &["日常", "米游社", "米游社BBS", "启用点赞"],
            "米游社BBS.启用点赞",
            true,
        )?,
        cancel_like: access.boolean(
            &["日常", "米游社", "米游社BBS", "启用取消点赞"],
            "米游社BBS.启用取消点赞",
            true,
        )?,
        share: access.boolean(
            &["日常", "米游社", "米游社BBS", "启用分享"],
            "米游社BBS.启用分享",
            true,
        )?,
    })
}

fn build_china_games(access: &Access<'_>) -> Result<Vec<Game>, DacapoError> {
    game_choices(
        access,
        "国服游戏",
        &[
            ("原神签到", Game::Genshin, true),
            ("崩坏2签到", Game::Honkai2, false),
            ("崩坏3签到", Game::Honkai3rd, false),
            ("未定事件簿签到", Game::TearsOfThemis, false),
            ("星穹铁道签到", Game::StarRail, false),
            ("绝区零签到", Game::ZenlessZoneZero, false),
        ],
    )
}

fn build_hoyolab_games(access: &Access<'_>) -> Result<Vec<Game>, DacapoError> {
    game_choices(
        access,
        "国际服游戏",
        &[
            ("国际服原神签到", Game::Genshin, false),
            ("国际服崩坏3签到", Game::Honkai3rd, false),
            ("国际服未定事件簿签到", Game::TearsOfThemis, false),
            ("国际服星穹铁道签到", Game::StarRail, false),
            ("国际服绝区零签到", Game::ZenlessZoneZero, false),
        ],
    )
}

fn game_choices(
    access: &Access<'_>,
    group: &'static str,
    choices: &[(&'static str, Game, bool)],
) -> Result<Vec<Game>, DacapoError> {
    let mut output = Vec::new();
    for &(field, game, default) in choices {
        let path = ["日常", "米游社", group, field];
        if access.boolean(&path, field, default)? {
            output.push(game);
        }
    }
    Ok(output)
}

fn build_china_blacklist(access: &Access<'_>) -> Result<RoleBlacklistConfig, DacapoError> {
    Ok(RoleBlacklistConfig {
        genshin: access.csv(
            &["日常", "米游社", "国服游戏", "原神黑名单"],
            "国服游戏.原神黑名单",
        )?,
        honkai2: access.csv(
            &["日常", "米游社", "国服游戏", "崩坏2黑名单"],
            "国服游戏.崩坏2黑名单",
        )?,
        honkai3rd: access.csv(
            &["日常", "米游社", "国服游戏", "崩坏3黑名单"],
            "国服游戏.崩坏3黑名单",
        )?,
        tears_of_themis: access.csv(
            &["日常", "米游社", "国服游戏", "未定事件簿黑名单"],
            "国服游戏.未定事件簿黑名单",
        )?,
        star_rail: access.csv(
            &["日常", "米游社", "国服游戏", "星穹铁道黑名单"],
            "国服游戏.星穹铁道黑名单",
        )?,
        zenless_zone_zero: access.csv(
            &["日常", "米游社", "国服游戏", "绝区零黑名单"],
            "国服游戏.绝区零黑名单",
        )?,
    })
}

fn has_hoyolab_blacklist(access: &Access<'_>) -> Result<bool, DacapoError> {
    for field in [
        "国际服原神黑名单",
        "国际服崩坏3黑名单",
        "国际服未定事件簿黑名单",
        "国际服星穹铁道黑名单",
        "国际服绝区零黑名单",
    ] {
        if !access
            .csv(&["日常", "米游社", "国际服游戏", field], field)?
            .is_empty()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn build_cloud_games(access: &Access<'_>) -> Result<(CloudGamesConfig, bool, bool), DacapoError> {
    let china_enabled = access.boolean(
        &["日常", "米游社", "云游戏", "启用云游戏签到"],
        "云游戏.启用云游戏签到",
        false,
    )?;
    let overseas_enabled = access.boolean(
        &["日常", "米游社", "云游戏", "启用国际服云游戏"],
        "云游戏.启用国际服云游戏",
        false,
    )?;
    Ok((
        CloudGamesConfig {
            china: ChinaCloudGamesConfig {
                genshin: access.cloud_entry("启用云原神", "云原神Token")?,
                zenless_zone_zero: access.cloud_entry("启用云绝区零", "云绝区零Token")?,
            },
            overseas: OverseasCloudGamesConfig {
                language: access
                    .string(
                        &["日常", "米游社", "云游戏", "国际服云游戏语言"],
                        "云游戏.国际服云游戏语言",
                    )?
                    .unwrap_or_else(|| "zh-cn".to_owned()),
                genshin: access.cloud_entry("启用国际服云原神", "国际服云原神Token")?,
            },
        },
        china_enabled,
        overseas_enabled,
    ))
}

fn build_web_activity(access: &Access<'_>) -> Result<WebActivityTaskConfig, DacapoError> {
    let enabled = access.boolean(
        &["日常", "米游社", "网页活动", "启用网页活动"],
        "网页活动.启用网页活动",
        false,
    )?;
    let mut activities = Vec::new();
    for activity in access.csv(
        &["日常", "米游社", "网页活动", "活动列表"],
        "网页活动.活动列表",
    )? {
        match activity.as_str() {
            "genshin_mizone" => activities.push(WebActivity::GenshinMizone),
            _ => {
                return Err(DacapoError::Field {
                    field: "网页活动.活动列表",
                    message: "包含不支持的活动名称",
                });
            }
        }
    }
    Ok(WebActivityTaskConfig {
        enabled,
        activities,
    })
}

fn build_notifications(access: &Access<'_>) -> Result<NotificationsConfig, DacapoError> {
    let enabled = access.boolean(
        &["Project", "General", "推送设置", "启用推送"],
        "推送设置.启用推送",
        false,
    )?;
    let error_only = access.boolean(
        &["Project", "General", "推送设置", "仅错误时推送"],
        "推送设置.仅错误时推送",
        false,
    )?;
    let block_keywords = access.csv(
        &["Project", "General", "推送设置", "屏蔽关键词"],
        "推送设置.屏蔽关键词",
    )?;
    if !enabled {
        return Ok(NotificationsConfig {
            enabled,
            error_only,
            block_keywords,
            providers: Vec::new(),
        });
    }
    let service = access
        .string(
            &["Project", "General", "推送设置", "推送服务"],
            "推送设置.推送服务",
        )?
        .unwrap_or_else(|| "pushplus".to_owned());
    let token = access
        .string(
            &["Project", "General", "推送设置", "推送Token"],
            "推送设置.推送Token",
        )?
        .unwrap_or_default();
    let topic = access
        .string(
            &["Project", "General", "推送设置", "推送群组"],
            "推送设置.推送群组",
        )?
        .filter(|value| !value.is_empty());
    let provider = dacapo_provider(access, &service, &token, topic)?;
    Ok(NotificationsConfig {
        enabled,
        error_only,
        block_keywords,
        providers: vec![provider],
    })
}

fn dacapo_provider(
    access: &Access<'_>,
    service: &str,
    token: &str,
    topic: Option<String>,
) -> Result<NotificationProvider, DacapoError> {
    if service == "wintoast" {
        return Ok(NotificationProvider::WindowsToast {
            title_prefix: access
                .string(
                    &["Project", "General", "推送设置", "Windows标题前缀"],
                    "推送设置.Windows标题前缀",
                )?
                .unwrap_or_else(|| "MihoyoBBSTools RS".to_owned()),
        });
    }
    if token.is_empty() {
        return Err(DacapoError::Field {
            field: "推送设置.推送Token",
            message: "所选推送服务需要 Token",
        });
    }
    let secret = || SecretString::new(token.to_owned());
    match service {
        "pushplus" => Ok(NotificationProvider::Pushplus {
            token: secret(),
            topic,
        }),
        "ftqq" => Ok(NotificationProvider::Ftqq {
            sendkey: secret(),
            api_url: None,
        }),
        "dingrobot" => Ok(NotificationProvider::Dingrobot {
            webhook: secret(),
            secret: access
                .string(
                    &["Project", "General", "推送设置", "钉钉加签 Secret"],
                    "推送设置.钉钉加签 Secret",
                )?
                .filter(|value| !value.is_empty())
                .map(SecretString::new),
        }),
        "feishubot" => Ok(NotificationProvider::Feishubot { webhook: secret() }),
        "bark" => Ok(NotificationProvider::Bark {
            token: secret(),
            api_url: access.url(
                &["Project", "General", "推送设置", "Bark API地址"],
                "推送设置.Bark API地址",
                false,
            )?,
            icon: access
                .string(
                    &["Project", "General", "推送设置", "Bark图标"],
                    "推送设置.Bark图标",
                )?
                .filter(|value| !value.is_empty()),
        }),
        "pushdeer" => Ok(NotificationProvider::Pushdeer {
            token: secret(),
            api_url: access.url(
                &["Project", "General", "推送设置", "PushDeer API地址"],
                "推送设置.PushDeer API地址",
                false,
            )?,
        }),
        "webhook" => Ok(NotificationProvider::Webhook { url: secret() }),
        "qmsg" => Ok(NotificationProvider::Qmsg {
            key: secret(),
            api_url: access.url(
                &["Project", "General", "推送设置", "Qmsg API地址"],
                "推送设置.Qmsg API地址",
                false,
            )?,
        }),
        "discord" => Ok(NotificationProvider::Discord { webhook: secret() }),
        "serverchan3" => Ok(NotificationProvider::Serverchan3 {
            sendkey: secret(),
            tags: topic,
        }),
        "telegram" => Ok(NotificationProvider::Telegram {
            bot_token: secret(),
            chat_id: access.required_string(
                &["Project", "General", "推送设置", "Telegram Chat ID"],
                "推送设置.Telegram Chat ID",
            )?,
            api_url: access
                .url(
                    &["Project", "General", "推送设置", "Telegram API地址"],
                    "推送设置.Telegram API地址",
                    false,
                )?
                .unwrap_or_else(super::default_telegram_api_url),
            proxy: access
                .string(
                    &["Project", "General", "推送设置", "Telegram代理"],
                    "推送设置.Telegram代理",
                )?
                .filter(|value| !value.is_empty())
                .map(SecretString::new),
        }),
        "wecom" => Ok(NotificationProvider::Wecom {
            corp_id: SecretString::new(access.required_string(
                &["Project", "General", "推送设置", "企业微信 Corp ID"],
                "推送设置.企业微信 Corp ID",
            )?),
            agent_id: access.required_string(
                &["Project", "General", "推送设置", "企业微信 Agent ID"],
                "推送设置.企业微信 Agent ID",
            )?,
            secret: secret(),
            to_user: access
                .string(
                    &["Project", "General", "推送设置", "企业微信 ToUser"],
                    "推送设置.企业微信 ToUser",
                )?
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "@all".to_owned()),
            api_url: None,
        }),
        "gotify" => Ok(NotificationProvider::Gotify {
            token: secret(),
            api_url: access.required_url(
                &["Project", "General", "推送设置", "Gotify API地址"],
                "推送设置.Gotify API地址",
            )?,
            priority: access.i64(
                &["Project", "General", "推送设置", "Gotify优先级"],
                "推送设置.Gotify优先级",
                0,
            )?,
        }),
        "smtp" => Ok(NotificationProvider::Smtp {
            host: access.required_string(
                &["Project", "General", "推送设置", "SMTP主机"],
                "推送设置.SMTP主机",
            )?,
            port: access.u16(
                &["Project", "General", "推送设置", "SMTP端口"],
                "推送设置.SMTP端口",
                465,
            )?,
            from: access.required_string(
                &["Project", "General", "推送设置", "SMTP发件人"],
                "推送设置.SMTP发件人",
            )?,
            to: access.required_string(
                &["Project", "General", "推送设置", "SMTP收件人"],
                "推送设置.SMTP收件人",
            )?,
            username: SecretString::new(access.required_string(
                &["Project", "General", "推送设置", "SMTP用户名"],
                "推送设置.SMTP用户名",
            )?),
            password: secret(),
            subject: access
                .string(
                    &["Project", "General", "推送设置", "SMTP主题"],
                    "推送设置.SMTP主题",
                )?
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "MihoyoBBSTools RS".to_owned()),
            tls: access.smtp_tls()?,
            timeout_seconds: access.optional_u64(
                &["Project", "General", "推送设置", "SMTP超时秒数"],
                "推送设置.SMTP超时秒数",
            )?,
        }),
        "wxpusher" => {
            let uids = access.csv(
                &["Project", "General", "推送设置", "WxPusher UIDs"],
                "推送设置.WxPusher UIDs",
            )?;
            let topic_ids = access.i64_csv(
                &["Project", "General", "推送设置", "WxPusher Topic IDs"],
                "推送设置.WxPusher Topic IDs",
            )?;
            Ok(NotificationProvider::Wxpusher {
                app_token: secret(),
                uids,
                topic_ids,
                api_url: access.url(
                    &["Project", "General", "推送设置", "WxPusher API地址"],
                    "推送设置.WxPusher API地址",
                    false,
                )?,
            })
        }
        _ => Err(DacapoError::Field {
            field: "推送设置.推送服务",
            message: "不支持的推送服务",
        }),
    }
}

fn expand_environment(value: &mut Value) -> Result<(), DacapoError> {
    match value {
        Value::String(text) => *text = expand_string(text, &|name| env::var(name).ok())?,
        Value::Array(values) => {
            for value in values {
                expand_environment(value)?;
            }
        }
        Value::Object(map) => {
            for value in map.values_mut() {
                expand_environment(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

struct Access<'a> {
    root: &'a Value,
}

impl<'a> Access<'a> {
    fn new(root: &'a Value) -> Self {
        Self { root }
    }

    fn field(&self, path: &[&str]) -> Option<&'a Value> {
        let mut current = self.root;
        for segment in path {
            current = current.as_object()?.get(*segment)?;
        }
        if let Some(value) = current.as_object().and_then(|map| map.get("value")) {
            current = value;
        }
        (!current.is_null()).then_some(current)
    }

    fn string(&self, path: &[&str], field: &'static str) -> Result<Option<String>, DacapoError> {
        let Some(value) = self.field(path) else {
            return Ok(None);
        };
        let value = match value {
            Value::String(value) => value.clone(),
            Value::Number(value) => value.to_string(),
            Value::Bool(value) => value.to_string(),
            _ => {
                return Err(DacapoError::Field {
                    field,
                    message: "必须是文本或标量",
                });
            }
        };
        Ok(Some(value.trim().to_owned()))
    }

    fn required_string(&self, path: &[&str], field: &'static str) -> Result<String, DacapoError> {
        self.string(path, field)?
            .filter(|value| !value.is_empty())
            .ok_or(DacapoError::Field {
                field,
                message: "不能为空",
            })
    }

    fn url(
        &self,
        path: &[&str],
        field: &'static str,
        required: bool,
    ) -> Result<Option<Url>, DacapoError> {
        let raw = self.string(path, field)?.unwrap_or_default();
        if raw.is_empty() {
            return if required {
                Err(DacapoError::Field {
                    field,
                    message: "不能为空",
                })
            } else {
                Ok(None)
            };
        }
        Url::parse(&raw).map(Some).map_err(|_| DacapoError::Field {
            field,
            message: "必须是有效 URL",
        })
    }

    fn required_url(&self, path: &[&str], field: &'static str) -> Result<Url, DacapoError> {
        self.url(path, field, true)?.ok_or(DacapoError::Field {
            field,
            message: "不能为空",
        })
    }

    fn boolean(
        &self,
        path: &[&str],
        field: &'static str,
        default: bool,
    ) -> Result<bool, DacapoError> {
        let Some(value) = self.field(path) else {
            return Ok(default);
        };
        match value {
            Value::Bool(value) => Ok(*value),
            Value::Number(value) if value.as_u64() == Some(0) => Ok(false),
            Value::Number(value) if value.as_u64() == Some(1) => Ok(true),
            Value::String(value) if value.eq_ignore_ascii_case("true") || value == "1" => Ok(true),
            Value::String(value) if value.eq_ignore_ascii_case("false") || value == "0" => {
                Ok(false)
            }
            _ => Err(DacapoError::Field {
                field,
                message: "必须是布尔值",
            }),
        }
    }

    fn u64(&self, path: &[&str], field: &'static str, default: u64) -> Result<u64, DacapoError> {
        let Some(value) = self.field(path) else {
            return Ok(default);
        };
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
            .ok_or(DacapoError::Field {
                field,
                message: "必须是非负整数",
            })
    }

    fn optional_u64(&self, path: &[&str], field: &'static str) -> Result<Option<u64>, DacapoError> {
        let Some(value) = self.field(path) else {
            return Ok(None);
        };
        if value.as_str().is_some_and(|value| value.trim().is_empty()) {
            return Ok(None);
        }
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
            .map(Some)
            .ok_or(DacapoError::Field {
                field,
                message: "必须是非负整数",
            })
    }

    fn u16(&self, path: &[&str], field: &'static str, default: u16) -> Result<u16, DacapoError> {
        let value = self.u64(path, field, u64::from(default))?;
        u16::try_from(value).map_err(|_| DacapoError::Field {
            field,
            message: "必须是 0 到 65535 之间的整数",
        })
    }

    fn i64(&self, path: &[&str], field: &'static str, default: i64) -> Result<i64, DacapoError> {
        let Some(value) = self.field(path) else {
            return Ok(default);
        };
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
            .ok_or(DacapoError::Field {
                field,
                message: "必须是整数",
            })
    }

    fn csv(&self, path: &[&str], field: &'static str) -> Result<Vec<String>, DacapoError> {
        let Some(value) = self.field(path) else {
            return Ok(Vec::new());
        };
        let raw = match value {
            Value::String(value) => value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            Value::Array(values) => values
                .iter()
                .map(|value| match value {
                    Value::String(value) => Ok(value.trim().to_owned()),
                    Value::Number(value) => Ok(value.to_string()),
                    _ => Err(DacapoError::Field {
                        field,
                        message: "列表只能包含文本或数字",
                    }),
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => {
                return Err(DacapoError::Field {
                    field,
                    message: "必须是逗号分隔文本或列表",
                });
            }
        };
        let mut output = Vec::new();
        for value in raw.into_iter().filter(|value| !value.is_empty()) {
            if !output.contains(&value) {
                output.push(value);
            }
        }
        Ok(output)
    }

    fn i64_csv(&self, path: &[&str], field: &'static str) -> Result<Vec<i64>, DacapoError> {
        self.csv(path, field)?
            .into_iter()
            .map(|value| {
                value.parse().map_err(|_| DacapoError::Field {
                    field,
                    message: "必须是逗号分隔的整数列表",
                })
            })
            .collect()
    }

    fn forums(&self, path: &[&str], field: &'static str) -> Result<Vec<u8>, DacapoError> {
        let values = self.csv(path, field)?;
        if values.is_empty() {
            return Ok(super::default_bbs_forums());
        }
        values
            .into_iter()
            .map(|value| {
                value.parse().map_err(|_| DacapoError::Field {
                    field,
                    message: "必须是逗号分隔的板块数字",
                })
            })
            .collect()
    }

    fn cloud_entry(
        &self,
        enabled_field: &'static str,
        token_field: &'static str,
    ) -> Result<CloudGameEntryConfig, DacapoError> {
        let enabled = self.boolean(
            &["日常", "米游社", "云游戏", enabled_field],
            enabled_field,
            false,
        )?;
        let token = self
            .string(&["日常", "米游社", "云游戏", token_field], token_field)?
            .filter(|value| !value.is_empty())
            .map(SecretString::new);
        Ok(CloudGameEntryConfig { enabled, token })
    }

    fn smtp_tls(&self) -> Result<SmtpTlsMode, DacapoError> {
        let value = self
            .string(
                &["Project", "General", "推送设置", "SMTP TLS"],
                "推送设置.SMTP TLS",
            )?
            .unwrap_or_else(|| "implicit".to_owned());
        match value.as_str() {
            "none" => Ok(SmtpTlsMode::None),
            "starttls" => Ok(SmtpTlsMode::Starttls),
            "implicit" => Ok(SmtpTlsMode::Implicit),
            _ => Err(DacapoError::Field {
                field: "推送设置.SMTP TLS",
                message: "只支持 none、starttls 或 implicit",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal(extra: &str) -> String {
        format!(
            r#"{{
  "Project": {{"General": {{
    "账号配置": {{
      "米游社Cookie": {{"value": "cookie_token=secret"}},
      "stuid": "123456",
      "stoken": {{"value": "v2_secret"}},
      "mid": "mid-value"
    }}
  }}}},
  "日常": {{"米游社": {{
    "米游社BBS": {{"启用米游社签到": false}},
    "国服游戏": {{"重试次数": {{"value": "3"}}, "启用国服签到": false}},
    "国际服游戏": {{"启用国际服签到": false}},
    "云游戏": {{}},
    "网页活动": {{}}
  }}}}
  {extra}
}}"#
        )
    }

    #[test]
    fn direct_and_wrapped_values_convert_without_temp_files() {
        let loaded = parse_dacapo(&minimal(""), "DaCapo:test").unwrap();
        assert_eq!(loaded.source, ConfigSource::Dacapo);
        assert_eq!(loaded.config.runtime.game_checkin_max_attempts, 3);
        let account = &loaded.config.accounts[0];
        assert_eq!(account.name, "DaCapo:test");
        assert_eq!(account.credentials.stoken.expose_secret(), "v2_secret");
        let cookie = account.credentials.cookie.expose_secret();
        assert!(cookie.contains("account_id=123456"));
        assert!(cookie.contains("account_mid_v2=mid-value"));
        assert!(!format!("{loaded:?}").contains("cookie_token=secret"));
    }

    #[test]
    fn game_retry_blacklist_cloud_and_activity_fields_are_mapped() {
        let source = minimal(
            r#",
  "unused": true"#,
        )
        .replace(
            r#""国服游戏": {"重试次数": {"value": "3"}, "启用国服签到": false}"#,
            r#""国服游戏": {
      "重试次数": "5",
      "启用国服签到": true,
      "原神签到": true,
      "原神黑名单": "10001,10002"
    }"#,
        )
        .replace(
            r#""云游戏": {}"#,
            r#""云游戏": {
      "启用云游戏签到": true,
      "启用云原神": true,
      "云原神Token": "cloud-secret"
    }"#,
        )
        .replace(
            r#""网页活动": {}"#,
            r#""网页活动": {"启用网页活动": true, "活动列表": "genshin_mizone"}"#,
        );
        let loaded = parse_dacapo(&source, "DaCapo:test").unwrap();
        let account = &loaded.config.accounts[0];
        assert_eq!(loaded.config.runtime.game_checkin_max_attempts, 5);
        assert_eq!(account.games, vec![Game::Genshin]);
        assert_eq!(
            account.china_checkin.role_blacklist.genshin,
            vec!["10001".to_owned(), "10002".to_owned()]
        );
        assert!(account.tasks.china_cloud_game);
        assert!(account.cloud_games.china.genshin.enabled);
        assert!(account.tasks.web_activity.enabled);
    }

    #[test]
    fn insufficient_notification_fields_and_unknown_activity_are_explicit_errors() {
        let telegram = minimal("").replace(
            r#""账号配置": {"#,
            r#""推送设置": {"启用推送": true, "推送服务": "telegram", "推送Token": "secret"},
    "账号配置": {"#,
        );
        assert!(matches!(
            parse_dacapo(&telegram, "DaCapo:test"),
            Err(DacapoError::Field {
                field: "推送设置.Telegram Chat ID",
                ..
            })
        ));

        let activity = minimal("").replace(
            r#""网页活动": {}"#,
            r#""网页活动": {"启用网页活动": true, "活动列表": "unknown"}"#,
        );
        assert!(matches!(
            parse_dacapo(&activity, "DaCapo:test"),
            Err(DacapoError::Field {
                field: "网页活动.活动列表",
                ..
            })
        ));
    }

    #[test]
    fn windows_toast_does_not_require_a_token() {
        let source = minimal("").replace(
            r#""账号配置": {"#,
            r#""推送设置": {"启用推送": true, "推送服务": "wintoast", "Windows标题前缀": "DaCapo"},
    "账号配置": {"#,
        );
        let loaded = parse_dacapo(&source, "DaCapo:test").unwrap();
        assert!(matches!(
            &loaded.config.notifications.providers[0],
            NotificationProvider::WindowsToast { title_prefix } if title_prefix == "DaCapo"
        ));
    }

    #[test]
    fn errors_and_debug_never_include_cookie_contents() {
        let malformed = minimal("").replace("cookie_token=secret", "broken-secret-segment");
        let error = parse_dacapo(&malformed, "DaCapo:test").unwrap_err();
        assert!(!error.to_string().contains("broken-secret-segment"));
    }

    #[test]
    fn distributed_dacapo_template_is_valid_yaml_and_uses_rust_command() {
        let template: serde_json::Value =
            serde_yaml_ng::from_str(include_str!("../../integrations/dacapo/template.yml"))
                .unwrap();
        assert_eq!(
            template["日常"]["米游社"]["_Base"]["command"]["value"].as_str(),
            Some("./MihoyoBBSToolsRS dacapo")
        );
    }
}
