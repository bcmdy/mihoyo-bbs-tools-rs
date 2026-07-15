use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::Command,
};

use serde_yaml_ng::{Mapping, Value};

use crate::auth::CookieJar;
#[cfg(not(test))]
use crate::http::HttpClient;
#[cfg(not(test))]
use reqwest::header::{COOKIE, HeaderMap, HeaderValue, USER_AGENT};
#[cfg(not(test))]
use serde::Deserialize;
use url::Url;

use super::{
    CURRENT_CONFIG_VERSION, Config, ConfigError, hydrate_stokens_from_cookies, load,
    open_secure_new, validate,
};

pub fn edit_file(path: &Path) -> Result<(), ConfigError> {
    let original = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let temporary = temporary_path(path);
    secure_write_new(&temporary, &original)?;
    let editor = env::var_os("VISUAL")
        .or_else(|| env::var_os("EDITOR"))
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "notepad".into()
            } else {
                "vi".into()
            }
        });
    let status = Command::new(editor)
        .arg(&temporary)
        .status()
        .map_err(|_| ConfigError::Edit("无法启动编辑器，请设置 EDITOR 或 VISUAL".to_owned()))?;
    if !status.success() {
        let _ = fs::remove_file(&temporary);
        return Err(ConfigError::Edit(
            "编辑器未正常退出，原配置未修改".to_owned(),
        ));
    }
    load(&temporary).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        ConfigError::Edit(format!("修改后的配置未通过校验，原配置未修改：{error}"))
    })?;
    let updated = fs::read_to_string(&temporary).map_err(|source| ConfigError::Read {
        path: temporary.clone(),
        source,
    })?;
    let _ = fs::remove_file(temporary);
    replace_validated(path, &updated)
}

pub async fn add_account_from_stdin(
    path: &Path,
    name: Option<&str>,
) -> Result<String, ConfigError> {
    eprintln!("请输入完整 Cookie（输入内容不会写入日志）：");
    let mut cookie = String::new();
    io::stdin()
        .lock()
        .read_line(&mut cookie)
        .map_err(|_| ConfigError::Edit("无法从标准输入读取 Cookie".to_owned()))?;
    add_account(path, name, cookie.trim()).await
}

pub async fn add_account(
    path: &Path,
    name: Option<&str>,
    cookie: &str,
) -> Result<String, ConfigError> {
    let jar =
        CookieJar::parse(cookie).map_err(|_| ConfigError::Edit("Cookie 格式无效".to_owned()))?;
    let stoken = jar
        .get("stoken")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ConfigError::Edit("Cookie 中缺少 stoken，请重新获取完整 Cookie".to_owned())
        })?;
    let nickname = fetch_nickname(cookie, jar.uid().unwrap_or_default()).await?;
    let mut account_name = format_account_name(&nickname);

    let existed = path.exists();
    let mut root = if existed {
        read_raw(path)?
    } else {
        empty_config_value()
    };
    {
        let accounts = accounts_mut(&mut root)?;
        if accounts
            .iter()
            .any(|account| account_name_of(account) == Some(account_name.as_str()))
        {
            account_name = format!(
                "{}-{}",
                account_name,
                uid_suffix(jar.uid().unwrap_or_default())
            );
            if accounts
                .iter()
                .any(|account| account_name_of(account) == Some(account_name.as_str()))
            {
                return Err(ConfigError::Edit(
                    "米游社昵称重复，且尾号名称仍冲突".to_owned(),
                ));
            }
        }
        let mut credentials = Mapping::new();
        credentials.insert(key("cookie"), Value::String(cookie.to_owned()));
        credentials.insert(key("stoken"), Value::String(stoken.to_owned()));
        let mut account = Mapping::new();
        account.insert(key("name"), Value::String(account_name.clone()));
        if let Some(remark) = name.map(str::trim).filter(|value| !value.is_empty()) {
            account.insert(key("remark"), Value::String(remark.to_owned()));
        }
        account.insert(key("enabled"), Value::Bool(true));
        account.insert(key("credentials"), Value::Mapping(credentials));
        account.insert(
            key("device"),
            serde_yaml_ng::to_value(super::DeviceConfig::default()).expect("默认设备配置可序列化"),
        );
        account.insert(
            key("proxy"),
            serde_yaml_ng::to_value(super::ProxyConfig::default()).expect("默认代理配置可序列化"),
        );
        account.insert(
            key("china_checkin"),
            serde_yaml_ng::to_value(super::ChinaCheckinConfig::default())
                .expect("默认国内签到配置可序列化"),
        );
        account.insert(
            key("hoyolab"),
            serde_yaml_ng::to_value(super::HoyolabConfig::default())
                .expect("默认 HoYoLAB 配置可序列化"),
        );
        account.insert(
            key("cloud_games"),
            serde_yaml_ng::to_value(super::CloudGamesConfig::default())
                .expect("默认云游戏配置可序列化"),
        );
        account.insert(key("tasks"), default_tasks());
        account.insert(key("games"), default_games());
        accounts.push(Value::Mapping(account));
    }
    let updated = validate_and_serialize(&root)?;
    if existed {
        replace_validated(path, &updated)?;
    } else {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|source| write_error(path, source))?;
        }
        let mut file = open_secure_new(path).map_err(|source| write_error(path, source))?;
        file.write_all(updated.as_bytes())
            .map_err(|source| write_error(path, source))?;
    }
    Ok(account_name)
}

#[cfg(not(test))]
#[derive(Deserialize)]
struct ProfileEnvelope {
    retcode: i64,
    data: Option<ProfileData>,
}
#[cfg(not(test))]
#[derive(Deserialize)]
struct ProfileData {
    user_info: ProfileInfo,
}
#[cfg(not(test))]
#[derive(Deserialize)]
struct ProfileInfo {
    nickname: String,
}

#[cfg(test)]
async fn fetch_nickname(cookie: &str, _uid: &str) -> Result<String, ConfigError> {
    if cookie.is_empty() {
        Err(ConfigError::Edit("Cookie 不能为空".to_owned()))
    } else {
        Ok("测试昵称".to_owned())
    }
}

#[cfg(not(test))]
async fn fetch_nickname(cookie: &str, uid: &str) -> Result<String, ConfigError> {
    let url = profile_url(uid);
    let mut headers = HeaderMap::new();
    headers.insert(
        COOKIE,
        HeaderValue::from_str(cookie)
            .map_err(|_| ConfigError::Edit("Cookie 包含无效请求头字符".to_owned()))?,
    );
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("Mozilla/5.0 miHoYoBBS/2.84.1"),
    );
    let client = HttpClient::builder()
        .build()
        .map_err(|_| ConfigError::Edit("昵称查询客户端初始化失败".to_owned()))?;
    let response: ProfileEnvelope = client
        .get_json_with(url, headers, &[])
        .await
        .map_err(|_| ConfigError::Edit("无法获取米游社昵称，请检查 Cookie 和网络".to_owned()))?;
    let retcode = response.retcode;
    let nickname = response
        .data
        .map(|v| v.user_info.nickname)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| ConfigError::Edit(format!("米游社昵称查询失败（代码 {retcode}）")))?;
    Ok(nickname)
}

fn profile_url(uid: &str) -> Url {
    let mut url = Url::parse("https://bbs-api.miyoushe.com/user/api/getUserFullInfo")
        .expect("valid MiHoYo profile URL");
    url.query_pairs_mut().append_pair("uid", uid);
    url
}

fn format_account_name(nickname: &str) -> String {
    format!("mys用户:{}", nickname.trim())
}

pub fn remove_account(path: &Path, name: &str) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let accounts = accounts_mut(root)?;
        let old_len = accounts.len();
        accounts.retain(|account| account_name_of(account) != Some(name));
        if accounts.len() == old_len {
            return Err(ConfigError::Edit(format!("未找到账号 {name:?}")));
        }
        if accounts.is_empty() {
            return Err(ConfigError::Edit("不能删除最后一个账号".to_owned()));
        }
        Ok(())
    })
}

pub fn set_account_tasks(
    path: &Path,
    name: &str,
    selected: &[u8],
    bbs: &[u8],
    forums: &[u8],
) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let account = find_account_mut(root, name)?;
        let tasks = account
            .entry(key("tasks"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("tasks 必须是对象".to_owned()))?;
        tasks.insert(
            key("china_game_checkin"),
            Value::Bool(selected.contains(&1)),
        );
        tasks.insert(key("hoyolab_checkin"), Value::Bool(selected.contains(&2)));
        let mut detail = Mapping::new();
        detail.insert(key("enabled"), Value::Bool(selected.contains(&3)));
        for (number, field) in [
            (1, "sign"),
            (2, "read"),
            (3, "like"),
            (4, "cancel_like"),
            (5, "share"),
        ] {
            detail.insert(key(field), Value::Bool(bbs.contains(&number)));
        }
        let forums = if forums.is_empty() {
            super::default_bbs_forums()
        } else {
            forums.to_vec()
        };
        detail.insert(
            key("forums"),
            Value::Sequence(
                forums
                    .into_iter()
                    .map(|forum| Value::Number(forum.into()))
                    .collect(),
            ),
        );
        tasks.insert(key("bbs"), Value::Mapping(detail));
        tasks.insert(key("china_cloud_game"), Value::Bool(selected.contains(&4)));
        tasks.insert(
            key("overseas_cloud_game"),
            Value::Bool(selected.contains(&5)),
        );
        let activities = tasks
            .get(key("web_activity"))
            .and_then(Value::as_mapping)
            .and_then(|web| web.get(key("activities")))
            .cloned()
            .unwrap_or_else(default_web_activities);
        let mut web_activity = Mapping::new();
        web_activity.insert(key("enabled"), Value::Bool(selected.contains(&6)));
        web_activity.insert(key("activities"), activities);
        tasks.insert(key("web_activity"), Value::Mapping(web_activity));
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
pub fn set_account_cloud_games(
    path: &Path,
    name: &str,
    china_genshin_enabled: bool,
    china_genshin_token: Option<&str>,
    china_zzz_enabled: bool,
    china_zzz_token: Option<&str>,
    overseas_language: &str,
    overseas_genshin_enabled: bool,
    overseas_genshin_token: Option<&str>,
) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let account = find_account_mut(root, name)?;
        let cloud_games = account
            .entry(key("cloud_games"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("cloud_games 必须是对象".to_owned()))?;
        let china = mapping_entry(cloud_games, "china")?;
        set_cloud_game_entry(china, "genshin", china_genshin_enabled, china_genshin_token)?;
        set_cloud_game_entry(
            china,
            "zenless_zone_zero",
            china_zzz_enabled,
            china_zzz_token,
        )?;
        let overseas = mapping_entry(cloud_games, "overseas")?;
        overseas.insert(key("language"), Value::String(overseas_language.to_owned()));
        set_cloud_game_entry(
            overseas,
            "genshin",
            overseas_genshin_enabled,
            overseas_genshin_token,
        )?;
        Ok(())
    })
}

fn mapping_entry<'a>(map: &'a mut Mapping, name: &str) -> Result<&'a mut Mapping, ConfigError> {
    map.entry(key(name))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .ok_or_else(|| ConfigError::Edit(format!("{name} 必须是对象")))
}

fn set_cloud_game_entry(
    parent: &mut Mapping,
    name: &str,
    enabled: bool,
    token: Option<&str>,
) -> Result<(), ConfigError> {
    let entry = mapping_entry(parent, name)?;
    entry.insert(key("enabled"), Value::Bool(enabled));
    entry.insert(
        key("token"),
        token
            .filter(|value| !value.trim().is_empty())
            .map(|value| Value::String(value.to_owned()))
            .unwrap_or(Value::Null),
    );
    Ok(())
}

pub fn set_account_games(path: &Path, name: &str, games: &[u8]) -> Result<(), ConfigError> {
    const NAMES: [&str; 6] = [
        "genshin",
        "honkai2",
        "honkai3rd",
        "tears_of_themis",
        "star_rail",
        "zenless_zone_zero",
    ];
    mutate_raw(path, |root| {
        let account = find_account_mut(root, name)?;
        account.insert(
            key("games"),
            Value::Sequence(
                games
                    .iter()
                    .filter_map(|n| NAMES.get((*n as usize).saturating_sub(1)))
                    .map(|v| Value::String((*v).to_owned()))
                    .collect(),
            ),
        );
        Ok(())
    })
}

pub fn set_account_china_checkin(
    path: &Path,
    name: &str,
    user_agent: &str,
    role_blacklist: &super::RoleBlacklistConfig,
) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let account = find_account_mut(root, name)?;
        let china = account
            .entry(key("china_checkin"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("china_checkin 必须是对象".to_owned()))?;
        china.insert(key("user_agent"), Value::String(user_agent.to_owned()));
        china.insert(
            key("role_blacklist"),
            serde_yaml_ng::to_value(role_blacklist)
                .map_err(|error| ConfigError::Edit(error.to_string()))?,
        );
        Ok(())
    })
}

pub fn set_account_hoyolab(
    path: &Path,
    name: &str,
    cookie: &str,
    language: &str,
    user_agent: &str,
    games: &[u8],
) -> Result<(), ConfigError> {
    const NAMES: [&str; 6] = [
        "genshin",
        "honkai2",
        "honkai3rd",
        "tears_of_themis",
        "star_rail",
        "zenless_zone_zero",
    ];
    mutate_raw(path, |root| {
        let account = find_account_mut(root, name)?;
        let hoyolab = account
            .entry(key("hoyolab"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("hoyolab 必须是对象".to_owned()))?;
        hoyolab.insert(key("cookie"), Value::String(cookie.to_owned()));
        hoyolab.insert(key("language"), Value::String(language.to_owned()));
        hoyolab.insert(key("user_agent"), Value::String(user_agent.to_owned()));
        hoyolab.insert(
            key("games"),
            Value::Sequence(
                games
                    .iter()
                    .filter_map(|number| NAMES.get((*number as usize).saturating_sub(1)))
                    .filter(|name| **name != "honkai2")
                    .map(|name| Value::String((*name).to_owned()))
                    .collect(),
            ),
        );
        Ok(())
    })
}

pub fn set_runtime(
    path: &Path,
    timezone: &str,
    timeout: u64,
    retry: u32,
    game_checkin_max_attempts: u32,
    delay: u64,
    level: &str,
) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let map = root
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("配置根节点无效".into()))?;
        let runtime = map
            .entry(key("runtime"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("runtime 必须是对象".into()))?;
        runtime.insert(key("timezone"), Value::String(timezone.into()));
        runtime.insert(
            key("request_timeout_seconds"),
            Value::Number(timeout.into()),
        );
        runtime.insert(key("retry_count"), Value::Number(retry.into()));
        runtime.insert(
            key("game_checkin_max_attempts"),
            Value::Number(game_checkin_max_attempts.into()),
        );
        runtime.insert(key("random_delay_seconds"), Value::Number(delay.into()));
        runtime.insert(key("log_level"), Value::String(level.into()));
        Ok(())
    })
}

pub fn set_logging(
    path: &Path,
    enabled: bool,
    directory: &str,
    file_prefix: &str,
) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let runtime = root
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("配置根节点无效".into()))?
            .entry(key("runtime"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("runtime 必须是对象".into()))?;
        let logging = runtime
            .entry(key("logging"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("runtime.logging 必须是对象".into()))?;
        logging.insert(key("enabled"), Value::Bool(enabled));
        logging.insert(key("directory"), Value::String(directory.to_owned()));
        logging.insert(key("file_prefix"), Value::String(file_prefix.to_owned()));
        Ok(())
    })
}

pub fn set_schedule(
    path: &Path,
    enabled: bool,
    interval_minutes: u64,
    run_on_start: bool,
) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let runtime = root
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("配置根节点无效".into()))?
            .entry(key("runtime"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("runtime 必须是对象".into()))?;
        let schedule = runtime
            .entry(key("schedule"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("runtime.schedule 必须是对象".into()))?;
        schedule.insert(key("enabled"), Value::Bool(enabled));
        schedule.insert(
            key("interval_minutes"),
            Value::Number(interval_minutes.into()),
        );
        schedule.insert(key("run_on_start"), Value::Bool(run_on_start));
        Ok(())
    })
}

pub fn set_account_general(
    path: &Path,
    name: &str,
    enabled: bool,
    remark: Option<&str>,
) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let account = find_account_mut(root, name)?;
        account.insert(key("enabled"), Value::Bool(enabled));
        account.insert(
            key("remark"),
            remark
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_owned()))
                .unwrap_or(Value::Null),
        );
        Ok(())
    })
}

pub fn set_account_device(
    path: &Path,
    name: &str,
    device_name: &str,
    model: &str,
    id: &str,
    fp: &str,
) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let account = find_account_mut(root, name)?;
        let device = account
            .entry(key("device"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("device 必须是对象".into()))?;
        for (field, value) in [
            ("name", device_name),
            ("model", model),
            ("id", id),
            ("fp", fp),
        ] {
            device.insert(key(field), Value::String(value.to_owned()));
        }
        Ok(())
    })
}

pub fn set_account_proxy(path: &Path, name: &str, proxy: Option<&str>) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let account = find_account_mut(root, name)?;
        let value = account
            .entry(key("proxy"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("proxy 必须是对象".into()))?;
        value.insert(
            key("url"),
            proxy
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_owned()))
                .unwrap_or(Value::Null),
        );
        Ok(())
    })
}

pub async fn replace_account_cookie(
    path: &Path,
    old_name: &str,
    cookie: &str,
) -> Result<String, ConfigError> {
    let jar =
        CookieJar::parse(cookie).map_err(|_| ConfigError::Edit("Cookie 格式无效".to_owned()))?;
    let stoken = jar
        .get("stoken")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ConfigError::Edit("Cookie 中缺少 stoken".to_owned()))?;
    let nickname = fetch_nickname(cookie, jar.uid().unwrap_or_default()).await?;
    let mut new_name = format_account_name(&nickname);
    mutate_raw(path, |root| {
        let accounts = accounts_mut(root)?;
        let index = accounts
            .iter()
            .position(|value| account_name_of(value) == Some(old_name))
            .ok_or_else(|| ConfigError::Edit(format!("未找到账号 {old_name:?}")))?;
        if accounts.iter().enumerate().any(|(other, account)| {
            other != index && account_name_of(account) == Some(new_name.as_str())
        }) {
            new_name = format!("{}-{}", new_name, uid_suffix(jar.uid().unwrap_or_default()));
        }
        if accounts.iter().enumerate().any(|(other, account)| {
            other != index && account_name_of(account) == Some(new_name.as_str())
        }) {
            return Err(ConfigError::Edit("米游社昵称与 UID 尾号仍然冲突".into()));
        }
        let account = accounts[index]
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("账号节点必须是对象".into()))?;
        account.insert(key("name"), Value::String(new_name.clone()));
        let credentials = account
            .entry(key("credentials"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("credentials 必须是对象".into()))?;
        credentials.insert(key("cookie"), Value::String(cookie.to_owned()));
        credentials.insert(key("stoken"), Value::String(stoken.to_owned()));
        Ok(())
    })?;
    Ok(new_name)
}

/// 仅持久化运行期间自动刷新的 Cookie，不查询昵称，也不改动账号名称或 SToken。
pub fn persist_refreshed_cookie(
    path: &Path,
    account_name: &str,
    cookie: &str,
) -> Result<bool, ConfigError> {
    CookieJar::parse(cookie).map_err(|_| ConfigError::Edit("刷新后的 Cookie 格式无效".into()))?;
    let raw = read_raw(path)?;
    let current_cookie = raw
        .as_mapping()
        .and_then(|root| root.get(key("accounts")))
        .and_then(Value::as_sequence)
        .and_then(|accounts| {
            accounts
                .iter()
                .find(|account| account_name_of(account) == Some(account_name))
        })
        .and_then(Value::as_mapping)
        .and_then(|account| account.get(key("credentials")))
        .and_then(Value::as_mapping)
        .and_then(|credentials| credentials.get(key("cookie")))
        .and_then(Value::as_str)
        .ok_or_else(|| ConfigError::Edit(format!("未找到账号 {account_name:?} 的 Cookie")))?;
    if current_cookie.contains("${") {
        return Ok(false);
    }
    mutate_raw(path, |root| {
        let account = find_account_mut(root, account_name)?;
        let credentials = account
            .entry(key("credentials"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("credentials 必须是对象".into()))?;
        credentials.insert(key("cookie"), Value::String(cookie.to_owned()));
        Ok(())
    })?;
    Ok(true)
}

pub fn set_notification_provider(
    path: &Path,
    index: Option<usize>,
    provider_type: &str,
    fields: &[(String, Option<String>)],
) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let notifications = root
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("配置根节点无效".into()))?
            .entry(key("notifications"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("notifications 必须是对象".into()))?;
        let providers = notifications
            .entry(key("providers"))
            .or_insert_with(|| Value::Sequence(Vec::new()))
            .as_sequence_mut()
            .ok_or_else(|| ConfigError::Edit("notifications.providers 必须是列表".into()))?;
        let mut created = Mapping::new();
        created.insert(key("type"), Value::String(provider_type.to_owned()));
        let provider = if let Some(index) = index {
            providers
                .get_mut(index)
                .and_then(Value::as_mapping_mut)
                .ok_or_else(|| ConfigError::Edit("通知渠道不存在".into()))?
        } else {
            providers.push(Value::Mapping(created));
            providers
                .last_mut()
                .and_then(Value::as_mapping_mut)
                .expect("刚添加的通知渠道是对象")
        };
        provider.insert(key("type"), Value::String(provider_type.to_owned()));
        for (field, value) in fields {
            let Some(value) = value else { continue };
            provider.insert(key(field), notification_field_value(field, value)?);
        }
        Ok(())
    })
}

pub fn remove_notification_provider(path: &Path, index: usize) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let notifications = root
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("配置根节点无效".into()))?
            .entry(key("notifications"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("notifications 必须是对象".into()))?;
        let providers = notifications
            .entry(key("providers"))
            .or_insert_with(|| Value::Sequence(Vec::new()))
            .as_sequence_mut()
            .ok_or_else(|| ConfigError::Edit("notifications.providers 必须是列表".into()))?;
        if index >= providers.len() {
            return Err(ConfigError::Edit("通知渠道不存在".into()));
        }
        providers.remove(index);
        if providers.is_empty() {
            notifications.insert(key("enabled"), Value::Bool(false));
        }
        Ok(())
    })
}

fn notification_field_value(field: &str, raw: &str) -> Result<Value, ConfigError> {
    if matches!(field, "uids" | "topic_ids") && raw.is_empty() {
        return Ok(Value::Sequence(Vec::new()));
    }
    if raw.is_empty() {
        return Ok(Value::Null);
    }
    match field {
        "priority" => raw
            .parse::<i64>()
            .map(|value| Value::Number(value.into()))
            .map_err(|_| ConfigError::Edit("priority 必须是整数".into())),
        "port" => raw
            .parse::<u16>()
            .map(|value| Value::Number(u64::from(value).into()))
            .map_err(|_| ConfigError::Edit("port 必须是 0 到 65535 之间的整数".into())),
        "timeout_seconds" => raw
            .parse::<u64>()
            .map(|value| Value::Number(value.into()))
            .map_err(|_| ConfigError::Edit("timeout_seconds 必须是非负整数".into())),
        "topic_ids" => raw
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse::<i64>()
                    .map(|number| Value::Number(number.into()))
                    .map_err(|_| ConfigError::Edit("topic_ids 必须是逗号分隔的整数".into()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Sequence),
        "uids" => Ok(Value::Sequence(
            raw.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_owned()))
                .collect(),
        )),
        _ => Ok(Value::String(raw.to_owned())),
    }
}
pub fn set_captcha_endpoint(path: &Path, endpoint: Option<&str>) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let map = root
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("配置根节点无效".into()))?;
        let captcha = map
            .entry(key("captcha"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("captcha 必须是对象".into()))?;
        captcha.insert(
            key("endpoint"),
            endpoint
                .map(|v| Value::String(v.into()))
                .unwrap_or(Value::Null),
        );
        Ok(())
    })
}
pub fn set_notification_options(
    path: &Path,
    enabled: bool,
    error_only: bool,
    keywords: Vec<String>,
) -> Result<(), ConfigError> {
    mutate_raw(path, |root| {
        let map = root
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("配置根节点无效".into()))?;
        let n = map
            .entry(key("notifications"))
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .ok_or_else(|| ConfigError::Edit("notifications 必须是对象".into()))?;
        n.insert(key("enabled"), Value::Bool(enabled));
        n.insert(key("error_only"), Value::Bool(error_only));
        n.insert(
            key("block_keywords"),
            Value::Sequence(keywords.into_iter().map(Value::String).collect()),
        );
        Ok(())
    })
}

fn find_account_mut<'a>(root: &'a mut Value, name: &str) -> Result<&'a mut Mapping, ConfigError> {
    accounts_mut(root)?
        .iter_mut()
        .find(|value| account_name_of(value) == Some(name))
        .and_then(Value::as_mapping_mut)
        .ok_or_else(|| ConfigError::Edit(format!("未找到账号 {name:?}")))
}

fn mutate_raw(
    path: &Path,
    mutate: impl FnOnce(&mut Value) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    let mut value = read_raw(path)?;
    mutate(&mut value)?;
    let updated = validate_and_serialize(&value)?;
    replace_validated(path, &updated)
}

fn replace_validated(path: &Path, updated: &str) -> Result<(), ConfigError> {
    let temporary = temporary_path(path);
    secure_write_new(&temporary, updated)?;
    load(&temporary).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        ConfigError::Edit(format!("修改后配置未通过校验，原配置未修改：{error}"))
    })?;
    let backup = path.with_extension(format!("yaml.{}.backup", std::process::id()));
    fs::rename(path, &backup).map_err(|source| write_error(path, source))?;
    if let Err(source) = fs::rename(&temporary, path) {
        let _ = fs::rename(&backup, path);
        let _ = fs::remove_file(&temporary);
        return Err(write_error(path, source));
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

fn read_raw(path: &Path) -> Result<Value, ConfigError> {
    let source = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_yaml_ng::from_str(&source).map_err(ConfigError::Yaml)
}

fn validate_and_serialize(value: &Value) -> Result<String, ConfigError> {
    let mut config: Config = serde_yaml_ng::from_value(value.clone())?;
    hydrate_stokens_from_cookies(&mut config);
    validate(&config)?;
    serde_yaml_ng::to_string(value).map_err(ConfigError::Serialize)
}

fn empty_config_value() -> Value {
    let mut root = Mapping::new();
    root.insert(key("version"), Value::Number(CURRENT_CONFIG_VERSION.into()));
    root.insert(
        key("runtime"),
        serde_yaml_ng::to_value(super::RuntimeConfig::default()).expect("默认运行配置可序列化"),
    );
    root.insert(
        key("captcha"),
        serde_yaml_ng::to_value(super::CaptchaConfig::default()).expect("默认验证码配置可序列化"),
    );
    root.insert(key("accounts"), Value::Sequence(Vec::new()));
    root.insert(
        key("notifications"),
        serde_yaml_ng::to_value(super::NotificationsConfig::default())
            .expect("默认通知配置可序列化"),
    );
    Value::Mapping(root)
}

fn accounts_mut(root: &mut Value) -> Result<&mut Vec<Value>, ConfigError> {
    root.as_mapping_mut()
        .and_then(|map| map.get_mut(key("accounts")))
        .and_then(Value::as_sequence_mut)
        .ok_or_else(|| ConfigError::Edit("accounts 必须是列表".to_owned()))
}

fn account_name_of(value: &Value) -> Option<&str> {
    value.as_mapping()?.get(key("name"))?.as_str()
}

fn default_tasks() -> Value {
    let mut bbs = Mapping::new();
    bbs.insert(key("enabled"), Value::Bool(true));
    bbs.insert(key("sign"), Value::Bool(true));
    bbs.insert(
        key("forums"),
        Value::Sequence(
            super::default_bbs_forums()
                .into_iter()
                .map(|forum| Value::Number(forum.into()))
                .collect(),
        ),
    );
    for field in ["read", "like", "cancel_like", "share"] {
        bbs.insert(key(field), Value::Bool(false));
    }
    let mut tasks = Mapping::new();
    tasks.insert(key("china_game_checkin"), Value::Bool(true));
    tasks.insert(key("hoyolab_checkin"), Value::Bool(false));
    tasks.insert(key("bbs"), Value::Mapping(bbs));
    tasks.insert(key("china_cloud_game"), Value::Bool(false));
    tasks.insert(key("overseas_cloud_game"), Value::Bool(false));
    let mut web_activity = Mapping::new();
    web_activity.insert(key("enabled"), Value::Bool(false));
    web_activity.insert(key("activities"), default_web_activities());
    tasks.insert(key("web_activity"), Value::Mapping(web_activity));
    Value::Mapping(tasks)
}

fn default_games() -> Value {
    Value::Sequence(vec![Value::String("genshin".to_owned())])
}

fn default_web_activities() -> Value {
    Value::Sequence(vec![Value::String("genshin_mizone".to_owned())])
}

fn uid_suffix(uid: &str) -> String {
    let suffix = uid.chars().rev().take(4).collect::<Vec<_>>();
    suffix.into_iter().rev().collect()
}

fn key(value: &str) -> Value {
    Value::String(value.to_owned())
}
fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension(format!("yaml.{}.editing", std::process::id()))
}

fn secure_write_new(path: &Path, content: &str) -> Result<(), ConfigError> {
    let mut file = open_secure_new(path).map_err(|source| write_error(path, source))?;
    file.write_all(content.as_bytes())
        .map_err(|source| write_error(path, source))
}
fn write_error(path: &Path, source: std::io::Error) -> ConfigError {
    ConfigError::Write {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_account_creates_missing_parent_and_valid_config() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested/config.yaml");
        let name = add_account(
            &path,
            Some("测试账号"),
            "account_id=123; account_mid_v2=mid; stoken=v2_secret",
        )
        .await
        .unwrap();
        assert_eq!(name, "mys用户:测试昵称");
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.config.accounts.len(), 1);
        assert_eq!(
            loaded.config.accounts[0].remark.as_deref(),
            Some("测试账号")
        );
        assert_eq!(
            loaded.config.accounts[0].games,
            vec![super::super::Game::Genshin]
        );
        assert!(!loaded.config.accounts[0].tasks.bbs.read);
        assert_eq!(loaded.config.accounts[0].tasks.bbs.forums, vec![5, 2]);
        let written = fs::read_to_string(path).unwrap();
        assert!(!written.contains("MIHOYO_COOKIE"));
        for field in [
            "timezone:",
            "game_checkin_max_attempts:",
            "logging:",
            "schedule:",
            "endpoint:",
            "stoken:",
            "device:",
            "proxy:",
            "china_checkin:",
            "role_blacklist:",
            "hoyolab:",
            "cloud_games:",
            "china:",
            "overseas:",
            "language:",
            "token:",
            "forums:",
            "activities:",
            "notifications:",
            "providers:",
        ] {
            assert!(written.contains(field), "新配置缺少字段 {field}");
        }
    }

    #[tokio::test]
    async fn invalid_cookie_does_not_create_parent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing/config.yaml");
        assert!(add_account(&path, None, "invalid").await.is_err());
        assert!(!path.parent().unwrap().exists());
    }

    #[tokio::test]
    async fn schedule_editor_persists_all_options() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        add_account(
            &path,
            None,
            "account_id=123; account_mid_v2=mid; stoken=v2_secret",
        )
        .await
        .unwrap();
        set_schedule(&path, true, 60, false).unwrap();
        let schedule = load(&path).unwrap().config.runtime.schedule;
        assert!(schedule.enabled);
        assert_eq!(schedule.interval_minutes, 60);
        assert!(!schedule.run_on_start);
    }

    #[tokio::test]
    async fn smtp_editor_persists_numeric_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        add_account(
            &path,
            None,
            "account_id=123; account_mid_v2=mid; stoken=v2_secret",
        )
        .await
        .unwrap();
        let fields = [
            ("host".to_owned(), Some("smtp.example.com".to_owned())),
            ("port".to_owned(), Some("465".to_owned())),
            ("from".to_owned(), Some("sender@example.com".to_owned())),
            ("to".to_owned(), Some("receiver@example.com".to_owned())),
            ("username".to_owned(), Some("smtp-user".to_owned())),
            ("password".to_owned(), Some("smtp-password".to_owned())),
            ("subject".to_owned(), Some("test".to_owned())),
            ("tls".to_owned(), Some("implicit".to_owned())),
            ("timeout_seconds".to_owned(), Some("30".to_owned())),
        ];
        set_notification_provider(&path, None, "smtp", &fields).unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("port: 465"));
        assert!(written.contains("timeout_seconds: 30"));
        assert!(matches!(
            load(&path).unwrap().config.notifications.providers[0],
            super::super::NotificationProvider::Smtp {
                port: 465,
                timeout_seconds: Some(30),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn task_editor_persists_cloud_and_web_switches() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        let name = add_account(
            &path,
            None,
            "account_id=123; account_mid_v2=mid; stoken=v2_secret",
        )
        .await
        .unwrap();
        set_account_tasks(&path, &name, &[4, 5, 6], &[], &[]).unwrap();
        let tasks = load(&path).unwrap().config.accounts[0].tasks.clone();
        assert!(tasks.china_cloud_game);
        assert!(tasks.overseas_cloud_game);
        assert!(tasks.web_activity.enabled);
        assert!(!tasks.china_game_checkin);
        assert!(!tasks.bbs.enabled);
    }

    #[tokio::test]
    async fn cloud_game_editor_sets_and_clears_sensitive_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        let name = add_account(
            &path,
            None,
            "account_id=123; account_mid_v2=mid; stoken=v2_secret",
        )
        .await
        .unwrap();
        set_account_cloud_games(
            &path,
            &name,
            true,
            Some("cn-token"),
            true,
            Some("zzz-token"),
            "ja-jp",
            true,
            Some("os-token"),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        let cloud = &loaded.config.accounts[0].cloud_games;
        assert!(cloud.china.genshin.enabled);
        assert_eq!(cloud.overseas.language, "ja-jp");
        assert_eq!(
            cloud
                .overseas
                .genshin
                .token
                .as_ref()
                .map(|v| v.expose_secret()),
            Some("os-token")
        );
        assert!(!format!("{:?}", cloud).contains("cn-token"));

        set_account_cloud_games(
            &path,
            &name,
            true,
            Some("cn-token"),
            false,
            None,
            "ja-jp",
            true,
            Some("os-token"),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert!(
            loaded.config.accounts[0]
                .cloud_games
                .china
                .zenless_zone_zero
                .token
                .is_none()
        );
    }

    #[tokio::test]
    async fn region_editors_persist_user_agents_blacklists_and_hoyolab_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        let name = add_account(
            &path,
            None,
            "account_id=123; account_mid_v2=mid; stoken=v2_secret",
        )
        .await
        .unwrap();
        let blacklist = super::super::RoleBlacklistConfig {
            genshin: vec!["10001".to_owned()],
            star_rail: vec!["20002".to_owned()],
            ..Default::default()
        };
        set_account_china_checkin(&path, &name, "custom-cn-agent", &blacklist).unwrap();
        set_account_hoyolab(
            &path,
            &name,
            "overseas-cookie",
            "ko-kr",
            "custom-os-agent",
            &[1, 3, 5],
        )
        .unwrap();

        let loaded = load(&path).unwrap();
        let account = &loaded.config.accounts[0];
        assert_eq!(account.china_checkin.user_agent, "custom-cn-agent");
        assert_eq!(account.china_checkin.role_blacklist, blacklist);
        let hoyolab = account.hoyolab.as_ref().unwrap();
        assert_eq!(hoyolab.cookie.expose_secret(), "overseas-cookie");
        assert_eq!(hoyolab.language, "ko-kr");
        assert_eq!(
            hoyolab.games,
            vec![
                super::super::Game::Genshin,
                super::super::Game::Honkai3rd,
                super::super::Game::StarRail,
            ]
        );
        assert!(!format!("{account:?}").contains("overseas-cookie"));
    }

    #[test]
    fn nickname_endpoint_uses_public_profile_api() {
        let url = profile_url("123456");
        assert_eq!(url.path(), "/user/api/getUserFullInfo");
        assert_eq!(url.query(), Some("uid=123456"));
        assert_eq!(format_account_name(" 测试昵称 "), "mys用户:测试昵称");
    }

    #[tokio::test]
    async fn refreshed_cookie_is_persisted_without_changing_account_identity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        let name = add_account(
            &path,
            Some("备注"),
            "account_id=123; account_mid_v2=mid; stoken=v2_secret; cookie_token=old",
        )
        .await
        .unwrap();

        assert!(
            persist_refreshed_cookie(
                &path,
                &name,
                "account_id=123; account_mid_v2=mid; stoken=v2_secret; cookie_token=new",
            )
            .unwrap()
        );
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.config.accounts[0].name, name);
        assert_eq!(loaded.config.accounts[0].remark.as_deref(), Some("备注"));
        assert!(
            loaded.config.accounts[0]
                .credentials
                .cookie
                .expose_secret()
                .contains("cookie_token=new")
        );
    }

    #[test]
    fn refreshed_cookie_never_replaces_environment_placeholder_with_secret() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        fs::write(
            &path,
            "accounts:\n  - name: env-account\n    credentials:\n      cookie: '${MIHOYO_COOKIE}'\n",
        )
        .unwrap();

        assert!(
            !persist_refreshed_cookie(
                &path,
                "env-account",
                "account_id=123; cookie_token=refreshed-secret",
            )
            .unwrap()
        );
        let written = fs::read_to_string(path).unwrap();
        assert!(written.contains("${MIHOYO_COOKIE}"));
        assert!(!written.contains("refreshed-secret"));
    }
}
