use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use serde_yaml_ng::{Mapping, Value};

use crate::auth::CookieJar;
#[cfg(not(test))]
use reqwest::header::{COOKIE, HeaderMap, HeaderValue, USER_AGENT};
#[cfg(not(test))]
use serde::Deserialize;
use url::Url;

use super::persist::{RealFileOps, ReplaceRequest, replace_atomically, with_path_lock};
use super::{
    CURRENT_CONFIG_VERSION, Config, ConfigError, hydrate_stokens_from_cookies, load,
    open_secure_new, validate,
};

pub struct EditSession {
    target: PathBuf,
    working: PathBuf,
    original: String,
    committed: bool,
}

impl EditSession {
    pub fn begin(path: &Path) -> Result<Self, ConfigError> {
        load(path)?;
        let original = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let working = path.with_extension(format!("yaml.{}.{}.session", std::process::id(), stamp));
        secure_write_new(&working, &original)?;
        Ok(Self {
            target: path.to_path_buf(),
            working,
            original,
            committed: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.working
    }

    pub fn has_changes(&self) -> Result<bool, ConfigError> {
        let current = fs::read_to_string(&self.working).map_err(|source| ConfigError::Read {
            path: self.working.clone(),
            source,
        })?;
        Ok(current != self.original)
    }

    pub fn summary(&self) -> Result<Vec<String>, ConfigError> {
        let before = load(&self.target)?.config;
        let after = load(&self.working)?.config;
        let mut changes = Vec::new();
        if serialized_differs(&before.runtime, &after.runtime)? {
            changes.push("全局运行设置已更新".to_owned());
        }
        if serialized_differs(&before.captcha, &after.captcha)? {
            changes.push("验证码设置已更新（敏感地址不显示）".to_owned());
        }
        for account in &before.accounts {
            match after
                .accounts
                .iter()
                .find(|candidate| candidate.name == account.name)
            {
                None => changes.push(format!("账号“{}”已删除", account.name)),
                Some(updated) if serialized_differs(account, updated)? => {
                    changes.push(format!("账号“{}”的设置已更新", account.name));
                }
                Some(_) => {}
            }
        }
        for account in &after.accounts {
            if !before
                .accounts
                .iter()
                .any(|candidate| candidate.name == account.name)
            {
                changes.push(format!("账号“{}”已添加", account.name));
            }
        }
        if serialized_differs(&before.notifications, &after.notifications)? {
            changes.push("通知设置已更新（敏感凭据不显示）".to_owned());
        }
        if changes.is_empty() && self.has_changes()? {
            changes.push("高级 YAML 内容或格式已更新".to_owned());
        }
        Ok(changes)
    }

    pub fn commit(mut self) -> Result<(), ConfigError> {
        let updated = fs::read_to_string(&self.working).map_err(|source| ConfigError::Read {
            path: self.working.clone(),
            source,
        })?;
        // 以会话开始时的原文作为基线：期间若有其它进程改过真实配置，
        // 这里会报告并发冲突并保留待保存副本，而不是静默覆盖对方写入。
        replace_validated_with_backup(&self.target, &updated, Some(&self.original))?;
        self.committed = true;
        let _ = fs::remove_file(&self.working);
        Ok(())
    }
}

impl Drop for EditSession {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.working);
        }
    }
}

fn serialized_differs<T: serde::Serialize>(before: &T, after: &T) -> Result<bool, ConfigError> {
    let before = serde_yaml_ng::to_value(before).map_err(ConfigError::Serialize)?;
    let after = serde_yaml_ng::to_value(after).map_err(ConfigError::Serialize)?;
    Ok(before != after)
}

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
    // 以启动编辑器前读到的原文作为基线：编辑期间若其它进程改过真实配置，
    // 这里会报告并发冲突并保留待保存副本，而不是静默覆盖对方写入。
    let backup = path.extension().and_then(|value| value.to_str()) != Some("session");
    replace_validated_inner(path, &updated, backup, Some(&original))
}

pub async fn add_account_from_stdin(
    path: &Path,
    name: Option<&str>,
) -> Result<String, ConfigError> {
    let cookie = super::input::prompt_secret("请输入完整 Cookie（输入内容以 * 显示）")?;
    if cookie.is_empty() {
        return Err(ConfigError::Edit("Cookie 不能为空".to_owned()));
    }
    let jar =
        CookieJar::parse(&cookie).map_err(|_| ConfigError::Edit("Cookie 格式无效".to_owned()))?;
    let account = add_account(path, name, &cookie).await?;
    println!(
        "已识别 UID：{}，SToken：已包含，MID：{}",
        jar.uid()
            .map(mask_identifier)
            .unwrap_or_else(|| "未识别".to_owned()),
        if jar.mid().is_some() {
            "已包含"
        } else {
            "未包含"
        }
    );
    Ok(account)
}

pub async fn add_account(
    path: &Path,
    name: Option<&str>,
    cookie: &str,
) -> Result<String, ConfigError> {
    let jar =
        CookieJar::parse(cookie).map_err(|_| ConfigError::Edit("Cookie 格式无效".to_owned()))?;
    let uid = jar.uid().ok_or_else(|| {
        ConfigError::Edit("Cookie 中未识别到 UID，请重新获取完整 Cookie".to_owned())
    })?;
    let stoken = jar
        .get("stoken")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ConfigError::Edit("Cookie 中缺少 stoken，请重新获取完整 Cookie".to_owned())
        })?;
    if stoken.starts_with("v2_") && jar.mid().is_none() {
        return Err(ConfigError::Edit(
            "Cookie 中已识别 UID 和 V2 SToken，但缺少 MID；请重新获取完整 Cookie".to_owned(),
        ));
    }
    let nickname = fetch_nickname(cookie, uid).await?;
    let mut account_name = format_account_name(&nickname);

    let original = if path.exists() {
        Some(read_text(path)?)
    } else {
        None
    };
    let mut root = match &original {
        Some(source) => parse_raw(source)?,
        None => empty_config_value(),
    };
    {
        let accounts = accounts_mut(&mut root)?;
        if accounts
            .iter()
            .any(|account| account_name_of(account) == Some(account_name.as_str()))
        {
            account_name = format!("{}-{}", account_name, uid_suffix(uid));
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
    if let Some(original) = original.as_deref() {
        replace_validated_inner(path, &updated, true, Some(original))?;
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
        HeaderValue::from_str(&crate::mihoyo::user_agent())
            .map_err(|_| ConfigError::Edit("User-Agent 包含无效请求头字符".to_owned()))?,
    );
    let client = crate::http::factory::for_config_lookup()
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
    task_max_attempts: u32,
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
            key("task_max_attempts"),
            Value::Number(task_max_attempts.into()),
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
    retention_days: Option<u64>,
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
        match retention_days {
            Some(days) => {
                logging.insert(key("retention_days"), Value::Number(days.into()));
            }
            None => {
                logging.remove(key("retention_days"));
            }
        }
        Ok(())
    })
}

pub fn set_schedule(
    path: &Path,
    enabled: bool,
    interval_minutes: u64,
    run_on_start: bool,
    poll_seconds: Option<u64>,
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
        match poll_seconds {
            Some(seconds) => {
                schedule.insert(key("poll_seconds"), Value::Number(seconds.into()));
            }
            None => {
                schedule.remove(key("poll_seconds"));
            }
        }
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
    mutate_raw_without_backup(path, |root| {
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
    if field == "title_prefix" && raw.is_empty() {
        return Ok(Value::String(String::new()));
    }
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

/// 读-改-写全部在路径锁内完成：先读取基线，再修改、校验并原子替换。
///
/// `create_backup_before_replace` 为真时在替换前生成受控时间戳备份。
fn mutate_locked(
    path: &Path,
    create_backup_before_replace: bool,
    mutate: impl FnOnce(&mut Value) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    with_path_lock(path, || {
        let original = read_text(path)?;
        let mut value = parse_raw(&original)?;
        mutate(&mut value)?;
        let updated = validate_and_serialize(&value)?;
        replace_locked(
            path,
            &updated,
            create_backup_before_replace,
            Some(&original),
        )
    })
}

fn mutate_raw(
    path: &Path,
    mutate: impl FnOnce(&mut Value) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    let backup = path.extension().and_then(|value| value.to_str()) != Some("session");
    mutate_locked(path, backup, mutate)
}

fn mutate_raw_without_backup(
    path: &Path,
    mutate: impl FnOnce(&mut Value) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    // 刷新写回：不建备份，但同样以同一次读-改-写内的原文作为基线，
    // 期间若真实配置被其它进程改写则报告冲突，避免静默覆盖。
    mutate_locked(path, false, mutate)
}

fn replace_validated_with_backup(
    path: &Path,
    updated: &str,
    expected: Option<&str>,
) -> Result<(), ConfigError> {
    replace_validated_inner(path, updated, true, expected)
}

fn replace_validated_inner(
    path: &Path,
    updated: &str,
    create_backup_before_replace: bool,
    expected: Option<&str>,
) -> Result<(), ConfigError> {
    // 「基线比较 + 原子替换」必须处于同一把进程内路径锁内，避免同进程并发写交错。
    with_path_lock(path, || {
        replace_locked(path, updated, create_backup_before_replace, expected)
    })
}

/// 在**已持有路径锁**的前提下执行原子替换。
///
/// 单独拆出这一层是为了让 [`mutate_locked`] 能把「读取基线 → 修改 → 校验 → 替换」
/// 整体放进同一把锁，避免 `std::sync::Mutex` 不可重入导致的死锁。
fn replace_locked(
    path: &Path,
    updated: &str,
    create_backup_before_replace: bool,
    expected: Option<&str>,
) -> Result<(), ConfigError> {
    let request = ReplaceRequest {
        target: path,
        content: updated,
        expected,
        create_backup: create_backup_before_replace,
        backup_keep: 5,
    };
    replace_atomically(&RealFileOps, &request, load_staged)
}

/// 校验暂存文件能通过完整配置加载（含版本、字段与业务校验）。
fn load_staged(path: &Path) -> Result<(), ConfigError> {
    load(path)
        .map(|_| ())
        .map_err(|error| ConfigError::Edit(format!("修改后配置未通过校验，原配置未修改：{error}")))
}

pub fn create_backup(path: &Path, keep: usize) -> Result<PathBuf, ConfigError> {
    if keep == 0 || keep > 50 {
        return Err(ConfigError::Edit(
            "配置备份保留数量必须在 1 到 50 之间".to_owned(),
        ));
    }
    load(path)?;
    let source = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let directory = backup_directory(path);
    fs::create_dir_all(&directory).map_err(|source| write_error(&directory, source))?;
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("config");
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let timestamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let mut backup = directory.join(format!("{stem}-{timestamp}.yaml"));
    let mut suffix = 1;
    while backup.exists() {
        backup = directory.join(format!("{stem}-{timestamp}-{suffix}.yaml"));
        suffix += 1;
    }
    secure_write_new(&backup, &source)?;
    prune_backups(&directory, stem, keep)?;
    Ok(backup)
}

#[derive(Clone, Debug)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub created: String,
    pub version: Option<u64>,
    pub account_count: Option<usize>,
}

pub fn list_backups(path: &Path) -> Result<Vec<BackupInfo>, ConfigError> {
    let directory = backup_directory(path);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("config");
    let prefix = format!("{stem}-");
    let mut paths = fs::read_dir(&directory)
        .map_err(|source| ConfigError::Read {
            path: directory.clone(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|backup| {
            backup.extension().and_then(|value| value.to_str()) == Some("yaml")
                && backup
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| right.cmp(left));
    Ok(paths
        .into_iter()
        .map(|backup| {
            let loaded = load(&backup).ok();
            let created = backup
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(|value| value.strip_prefix(&prefix))
                .unwrap_or("未知时间")
                .to_owned();
            BackupInfo {
                path: backup,
                created,
                version: loaded.as_ref().map(|loaded| loaded.config.version),
                account_count: loaded.as_ref().map(|loaded| loaded.config.accounts.len()),
            }
        })
        .collect())
}

pub fn restore_backup(path: &Path, backup: &Path) -> Result<PathBuf, ConfigError> {
    load(path)?;
    let allowed_directory = backup_directory(path);
    let allowed = fs::canonicalize(&allowed_directory).map_err(|source| ConfigError::Read {
        path: allowed_directory.clone(),
        source,
    })?;
    let backup = fs::canonicalize(backup).map_err(|source| ConfigError::Read {
        path: backup.to_path_buf(),
        source,
    })?;
    if backup.parent() != Some(allowed.as_path()) {
        return Err(ConfigError::Edit(format!(
            "只允许恢复 {} 目录中的受控备份",
            allowed.display()
        )));
    }
    load(&backup).map_err(|error| ConfigError::Edit(format!("备份未通过校验：{error}")))?;
    let source = fs::read_to_string(&backup).map_err(|source| ConfigError::Read {
        path: backup.clone(),
        source,
    })?;
    let current_backup = create_backup(path, 5)?;
    // 恢复也以恢复前的原文作为基线，避免覆盖恢复窗口内其它进程的写入。
    with_path_lock(path, || {
        let original = read_text(path)?;
        replace_locked(path, &source, false, Some(&original))
    })?;
    Ok(current_backup)
}

fn backup_directory(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("backups")
}

pub fn install_new_config(staged: &Path, target: &Path) -> Result<(), ConfigError> {
    if target.exists() {
        return Err(ConfigError::Edit(format!(
            "配置文件已存在，拒绝覆盖：{}；请改用 `config setup`",
            target.display()
        )));
    }
    load(staged)?;
    let source = fs::read_to_string(staged).map_err(|source| ConfigError::Read {
        path: staged.to_path_buf(),
        source,
    })?;
    if let Some(parent) = target.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|source| write_error(parent, source))?;
    }
    let mut file = open_secure_new(target).map_err(|source| write_error(target, source))?;
    if let Err(source) = file
        .write_all(source.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(target);
        return Err(write_error(target, source));
    }
    let _ = fs::remove_file(staged);
    Ok(())
}

fn prune_backups(directory: &Path, stem: &str, keep: usize) -> Result<(), ConfigError> {
    let prefix = format!("{stem}-");
    let mut backups = fs::read_dir(directory)
        .map_err(|source| ConfigError::Read {
            path: directory.to_path_buf(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|value| value.to_str()) == Some("yaml")
                && path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect::<Vec<_>>();
    backups.sort();
    let remove_count = backups.len().saturating_sub(keep);
    for backup in backups.into_iter().take(remove_count) {
        fs::remove_file(&backup).map_err(|source| write_error(&backup, source))?;
    }
    Ok(())
}

fn read_text(path: &Path) -> Result<String, ConfigError> {
    fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn read_raw(path: &Path) -> Result<Value, ConfigError> {
    parse_raw(&read_text(path)?)
}

fn parse_raw(source: &str) -> Result<Value, ConfigError> {
    serde_yaml_ng::from_str(source).map_err(ConfigError::Yaml)
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

fn mask_identifier(value: &str) -> String {
    format!("***{}", uid_suffix(value))
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

// ===== 陈旧临时文件清理（启动兜底） =====
//
// 配置编辑会产生三类含凭据的中间文件，命名均为「配置文件名.PID[.纳秒].后缀」：
//
// - `config.yaml.<pid>.<nanos>.session`：编辑会话的受控副本（EditSession::begin）；
// - `config.yaml.<pid>.editing`：`config edit` 交给外部编辑器的副本（temporary_path）；
// - `config.yaml.<pid>.backup`：替换过程中旧文件的瞬时暂存（replace_validated_inner）。
//
// 正常路径会删除它们，但 release 使用 `panic = "abort"`、Drop 不会执行，
// 因此需要启动兜底。这里只删除**严格匹配**上述命名、且文件名中 PID 已不属于
// 存活进程的普通文件；解析失败、前缀不符、目录与符号链接一律不动。

/// 陈旧中间文件的类型，仅用于日志说明。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum StaleKind {
    Session,
    Editing,
    Backup,
}

impl StaleKind {
    const fn suffix(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Editing => "editing",
            Self::Backup => "backup",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Session => "会话副本",
            Self::Editing => "编辑器副本",
            Self::Backup => "替换暂存",
        }
    }
}

const STALE_KINDS: [StaleKind; 3] = [StaleKind::Session, StaleKind::Editing, StaleKind::Backup];

/// 解析文件名是否严格匹配「前缀.PID[.纳秒].后缀」，匹配时返回 PID。
///
/// 只有 `.session` 允许出现纳秒时间戳中间段；`.editing` 与 `.backup` 必须紧接 PID。
fn parse_stale_candidate(file_name: &str, prefix: &str, expected: StaleKind) -> Option<u32> {
    let rest = file_name.strip_prefix(prefix)?.strip_prefix('.')?;
    let segments = rest.split('.').collect::<Vec<_>>();
    if segments.len() < 2 || segments.len() > 3 {
        return None;
    }
    let pid_part = segments[0];
    if pid_part.is_empty() || !pid_part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let suffix = segments[segments.len() - 1];
    if !suffix.eq_ignore_ascii_case(expected.suffix()) {
        return None;
    }
    if segments.len() == 3 {
        // 中间段只允许会话副本的纳秒时间戳，且必须全为数字。
        if expected != StaleKind::Session {
            return None;
        }
        let stamp = segments[1];
        if stamp.is_empty() || !stamp.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
    }
    pid_part.parse::<u32>().ok()
}

/// 判断 PID 是否仍属于存活进程。
///
/// PID 0 与 1 视为系统占位一律不删；查询失败按「存活」处理，宁可留下文件也不误删。
fn pid_is_alive(pid: u32) -> bool {
    if pid <= 1 {
        return true;
    }
    #[cfg(windows)]
    {
        pid_alive_windows(pid)
    }
    #[cfg(unix)]
    {
        pid_alive_unix(pid)
    }
    #[cfg(not(any(windows, unix)))]
    {
        true
    }
}

#[cfg(unix)]
fn pid_alive_unix(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };
    // SAFETY: kill(pid, 0) 只探测进程是否存在与是否有权限，不解引用任何指针。
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn pid_alive_windows(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, WAIT_OBJECT_0},
        System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            WaitForSingleObject,
        },
    };

    // SAFETY: OpenProcess 只读取 PID，不写入调用方指针；返回句柄由下方 CloseHandle 释放。
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        // ERROR_INVALID_PARAMETER 表示该 PID 不存在；其它错误（如权限不足）保守视为存活。
        // SAFETY: GetLastError 只读取当前线程的错误码。
        let error = unsafe { GetLastError() };
        return error != ERROR_INVALID_PARAMETER;
    }
    // SAFETY: handle 来自 OpenProcess 且非空；0 毫秒表示立即返回。
    let wait = unsafe { WaitForSingleObject(handle, 0) };
    // SAFETY: handle 只在本函数内使用，关闭一次即可。
    unsafe {
        CloseHandle(handle);
    }
    // WAIT_OBJECT_0 表示进程已退出；WAIT_TIMEOUT 或 WAIT_FAILED 都按存活处理。
    wait != WAIT_OBJECT_0
}

/// 扫描配置文件的父目录，返回符合本程序命名规则且 `is_alive` 判定为已退出的文件。
///
/// 该函数不修改文件系统；是否删除由调用方决定。
fn stale_temporary_files(
    path: &Path,
    is_alive: impl Fn(u32) -> bool,
) -> Vec<(PathBuf, u32, StaleKind)> {
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    // 中间文件由 `Path::with_extension` 生成，文件名恒为 `<stem>.yaml.<pid>...`。
    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
        return Vec::new();
    };
    if stem.is_empty() {
        return Vec::new();
    }
    let prefix = format!("{stem}.yaml");
    let directory = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let candidate = entry.path();
        // 只处理普通文件：目录与符号链接一律跳过，避免越界删除。
        let Ok(metadata) = fs::symlink_metadata(&candidate) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        if candidate.as_path() == path {
            continue;
        }
        let Some(file_name) = candidate.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some((pid, kind)) = STALE_KINDS.iter().find_map(|kind| {
            parse_stale_candidate(file_name, &prefix, *kind).map(|pid| (pid, *kind))
        }) else {
            continue;
        };
        if is_alive(pid) {
            continue;
        }
        found.push((candidate, pid, kind));
    }
    found.sort();
    found
}

/// 启动兜底：清理同目录下 PID 已退出的陈旧中间文件，返回删除数量。
///
/// 任何失败只记录 warn，不影响主流程；日志只含路径、PID 与错误文本，不含文件内容。
fn cleanup_stale_files(path: &Path) -> usize {
    cleanup_stale_files_with(path, pid_is_alive)
}

/// [`cleanup_stale_files`] 的可注入版本，`is_alive` 决定某个 PID 是否仍存活。
fn cleanup_stale_files_with(path: &Path, is_alive: impl Fn(u32) -> bool) -> usize {
    let mut removed = 0;
    for (candidate, pid, kind) in stale_temporary_files(path, is_alive) {
        match fs::remove_file(&candidate) {
            Ok(()) => {
                removed += 1;
                tracing::warn!(
                    path = %candidate.display(),
                    pid,
                    "已清理上次异常退出遗留的{}临时文件",
                    kind.label()
                );
            }
            Err(error) => {
                tracing::warn!(
                    path = %candidate.display(),
                    error = %error,
                    "无法清理遗留的{}临时文件，请手动确认该文件是否含凭据",
                    kind.label()
                );
            }
        }
    }
    removed
}

/// 进程内只执行一次启动清理，供配置加载入口调用。
///
/// 用 `OnceLock` 而不是每轮都扫描：清理属于启动兜底，重复扫描只增加目录 IO。
pub(super) fn cleanup_stale_files_once(path: &Path) {
    use std::{
        collections::HashSet,
        sync::{Mutex, OnceLock},
    };

    static CLEANED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let cleaned = CLEANED.get_or_init(|| Mutex::new(HashSet::new()));
    {
        // 只在登记阶段持有锁：目录扫描与删除放在锁外，避免拖慢其它线程。
        let mut cleaned = match cleaned.lock() {
            Ok(cleaned) => cleaned,
            // 锁中毒说明其它线程 panic；此处不影响主流程，直接放弃清理。
            Err(_) => return,
        };
        if !cleaned.insert(path.to_path_buf()) {
            return;
        }
    }
    cleanup_stale_files(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 取得一个**确定已退出**的 PID：启动一个立即结束的子进程并回收它。
    ///
    /// 比扫描「看起来空闲」的 PID 更确定，也不依赖系统的 `pid_max` 配置。
    fn exited_pid() -> Option<u32> {
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", "exit"]);
            command
        };
        #[cfg(unix)]
        let mut command = std::process::Command::new("true");
        #[cfg(not(any(windows, unix)))]
        let mut command = {
            let mut command = std::process::Command::new("echo");
            command.arg("");
            command
        };
        let mut child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let pid = child.id();
        child.wait().ok()?;
        Some(pid)
    }

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
            "task_max_attempts:",
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
        set_schedule(&path, true, 60, false, Some(5)).unwrap();
        let schedule = load(&path).unwrap().config.runtime.schedule;
        assert!(schedule.enabled);
        assert_eq!(schedule.interval_minutes, 60);
        assert!(!schedule.run_on_start);
        assert_eq!(schedule.poll_seconds, Some(5));
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

    #[test]
    fn stale_candidate_parser_accepts_only_program_names() {
        let cases = [
            (
                "config.yaml.4242.1700000000000000000.session",
                StaleKind::Session,
                Some(4242),
            ),
            ("config.yaml.4242.session", StaleKind::Session, Some(4242)),
            ("config.yaml.4242.editing", StaleKind::Editing, Some(4242)),
            ("config.yaml.4242.backup", StaleKind::Backup, Some(4242)),
            // 后缀与类型必须对应。
            ("config.yaml.4242.editing", StaleKind::Session, None),
            ("config.yaml.4242.backup", StaleKind::Editing, None),
            // 非数字 PID、缺失 PID、多余段、其它前缀都不匹配。
            ("config.yaml.pid.backup", StaleKind::Backup, None),
            ("config.yaml.backup", StaleKind::Backup, None),
            ("config.yaml.4242.1.backup", StaleKind::Backup, None),
            ("other.yaml.4242.backup", StaleKind::Backup, None),
            ("config.yaml.4242.backup.1", StaleKind::Backup, None),
            ("config.yaml.4242.conflict.yaml", StaleKind::Backup, None),
            // 中间段只允许出现在会话副本里，而且必须是数字。
            ("config.yaml.4242.abc.session", StaleKind::Session, None),
            (
                "config.yaml.4242.1700000000000000000.backup",
                StaleKind::Backup,
                None,
            ),
        ];
        for (file_name, kind, expected) in cases {
            assert_eq!(
                parse_stale_candidate(file_name, "config.yaml", kind),
                expected,
                "文件名 {file_name} 类型 {kind:?}"
            );
        }
        // PID 超出 u32 范围时不匹配。
        assert_eq!(
            parse_stale_candidate(
                "config.yaml.4294967296.backup",
                "config.yaml",
                StaleKind::Backup
            ),
            None
        );
    }

    #[test]
    fn stale_scan_keeps_files_owned_by_live_pid() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        fs::write(&path, "version: 1\n").unwrap();
        let live = temp.path().join("config.yaml.42.editing");
        let dead = temp.path().join("config.yaml.43.editing");
        fs::write(&live, "live").unwrap();
        fs::write(&dead, "dead").unwrap();

        let found = stale_temporary_files(&path, |pid| pid == 42);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, dead);
        assert_eq!(found[0].1, 43);
        assert_eq!(found[0].2, StaleKind::Editing);
    }

    #[test]
    fn stale_scan_skips_other_prefixes_and_directories() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        fs::write(&path, "version: 1\n").unwrap();
        let decoys = [
            "other.yaml.99.backup",
            "config.yaml.backup",
            "config.yaml.99.yaml",
            "config.yaml.99.conflict.yaml",
            "config.yaml.99.session.tmp",
        ];
        for name in decoys {
            fs::write(temp.path().join(name), "keep").unwrap();
        }
        fs::create_dir(temp.path().join("config.yaml.99.editing")).unwrap();

        let found = stale_temporary_files(&path, |_| false);
        assert!(
            found.is_empty(),
            "不应把非本程序命名的文件当作陈旧文件：{found:?}"
        );
        for name in decoys {
            assert!(temp.path().join(name).exists(), "{name} 不应被清理");
        }
        assert!(temp.path().join("config.yaml.99.editing").is_dir());
    }

    #[test]
    fn cleanup_stale_files_removes_only_dead_pid_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        fs::write(&path, "version: 1\n").unwrap();
        let stale = [
            "config.yaml.7.1700000000000000000.session",
            "config.yaml.7.backup",
            "config.yaml.7.editing",
        ];
        for name in &stale {
            fs::write(temp.path().join(name), "secret-cookie").unwrap();
        }
        let live = temp
            .path()
            .join(format!("config.yaml.{}.editing", std::process::id()));
        fs::write(&live, "secret-cookie").unwrap();

        // 注入「只有当前进程存活」的判定，避免依赖测试机上的 PID 分配情况。
        let removed = cleanup_stale_files_with(&path, |pid| pid == std::process::id());
        assert_eq!(removed, stale.len());
        for name in &stale {
            assert!(!temp.path().join(name).exists(), "{name} 应被清理");
        }
        assert!(live.exists(), "存活进程的临时文件不能被删除");
        assert!(path.exists(), "目标配置不能被删除");
    }

    #[test]
    fn pid_liveness_detects_current_process_as_alive_and_exited_child_as_dead() {
        // 当前进程与系统占位 PID 必须视为存活。
        assert!(pid_is_alive(std::process::id()));
        assert!(pid_is_alive(0));
        assert!(pid_is_alive(1));

        // 已启动并回收的子进程视为已退出。
        let Some(pid) = exited_pid() else {
            // 无法启动子进程的环境（如受限沙箱）跳过该断言，不影响其它检查。
            return;
        };
        assert!(
            !pid_is_alive(pid),
            "已回收的子进程 PID {pid} 应判定为已退出"
        );
    }

    #[test]
    fn cleanup_stale_files_ignores_missing_directory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing/config.yaml");
        assert_eq!(cleanup_stale_files(&path), 0);
    }

    #[test]
    fn stale_cleanup_runs_once_per_config_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        fs::write(&path, "version: 1\n").unwrap();
        let Some(dead_pid) = exited_pid() else {
            return;
        };
        let stale = temp.path().join(format!("config.yaml.{dead_pid}.backup"));
        fs::write(&stale, "secret-cookie").unwrap();

        cleanup_stale_files_once(&path);
        assert!(!stale.exists(), "首次调用应清理陈旧文件");
        // 同一路径同一进程只清理一次：再次调用不应重复扫描。
        fs::write(&stale, "secret-cookie").unwrap();
        cleanup_stale_files_once(&path);
        assert!(stale.exists(), "同一路径同一进程只清理一次");
        fs::remove_file(&stale).unwrap();
    }

    #[test]
    fn commit_reports_conflict_and_keeps_other_process_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        let name = "会话账号";
        fs::write(
            &path,
            format!(
                "version: 1\naccounts:\n  - name: {name}\n    credentials:\n      cookie: ''\n      stoken: ''\n    tasks:\n      bbs: false\n"
            ),
        )
        .unwrap();

        let session = EditSession::begin(&path).unwrap();
        // 模拟另一个进程在会话期间修改了真实配置。
        let other = format!(
            "version: 1\naccounts:\n  - name: {name}\n    credentials:\n      cookie: ''\n      stoken: ''\n    remark: 其它进程写入\n    tasks:\n      bbs: false\n"
        );
        fs::write(&path, &other).unwrap();

        let error = session.commit().unwrap_err();
        let ConfigError::Conflict {
            path: reported,
            preserved,
        } = error
        else {
            panic!("应报告并发冲突，实际为 {error:?}");
        };
        assert_eq!(reported, path);
        // 其它进程的写入不被覆盖。
        assert_eq!(fs::read_to_string(&path).unwrap(), other);
        // 会话内容被保留到冲突副本，且副本可被加载校验。
        let preserved_body = fs::read_to_string(&preserved).unwrap();
        assert!(preserved_body.contains(name));
        assert!(!preserved_body.contains("其它进程写入"));
        assert!(
            preserved
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("conflict")
        );
    }

    #[tokio::test]
    async fn commit_succeeds_when_baseline_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        add_account(
            &path,
            None,
            "account_id=123; account_mid_v2=mid; stoken=v2_secret",
        )
        .await
        .unwrap();
        let session = EditSession::begin(&path).unwrap();
        session.commit().unwrap();
        // 提交成功后不应残留会话副本。
        let leftovers = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().to_string_lossy().contains(".session"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[tokio::test]
    async fn refreshed_cookie_writeback_has_no_backup() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        add_account(
            &path,
            None,
            "account_id=123; account_mid_v2=mid; stoken=v2_secret; cookie_token=old",
        )
        .await
        .unwrap();
        let name = load(&path).unwrap().config.accounts[0].name.clone();

        // 正常写回：刷新后的 Cookie 落盘，且不创建受控时间戳备份。
        assert!(
            persist_refreshed_cookie(
                &path,
                &name,
                "account_id=123; account_mid_v2=mid; stoken=v2_secret; cookie_token=new",
            )
            .unwrap()
        );
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("cookie_token=new"));
        assert!(!temp.path().join("backups").exists());
    }

    #[test]
    fn refresh_writeback_reports_conflict_when_target_changed_after_read() {
        // 直接验证「读取基线 → 目标被改写 → 带基线替换」这条链路：
        // 基线不匹配时必须报告冲突、保留待保存副本，且不覆盖对方写入。
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        let baseline = "version: 1\naccounts:\n  - name: 账号A\n    credentials:\n      cookie: ''\n      stoken: ''\n    tasks:\n      bbs: false\n";
        fs::write(&path, baseline).unwrap();
        let other = baseline.replace("cookie: ''", "cookie: 'other-process'");
        fs::write(&path, &other).unwrap();

        let error = replace_validated_inner(
            &path,
            &baseline.replace("cookie: ''", "cookie: 'refreshed'"),
            false,
            Some(baseline),
        )
        .unwrap_err();

        let ConfigError::Conflict { preserved, .. } = error else {
            panic!("应报告并发冲突");
        };
        assert_eq!(fs::read_to_string(&path).unwrap(), other);
        assert!(
            fs::read_to_string(&preserved)
                .unwrap()
                .contains("cookie: 'refreshed'")
        );
    }

    #[test]
    fn replace_without_baseline_still_replaces_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        fs::write(&path, minimal_config("旧备注")).unwrap();
        replace_validated_inner(&path, &minimal_config("新备注"), false, None).unwrap();
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("remark: 新备注"));
        assert!(!written.contains("旧备注"));
    }

    /// 一份可通过 `load` 完整校验的最小配置，`remark` 用于区分新旧内容。
    fn minimal_config(remark: &str) -> String {
        format!(
            "version: 1\naccounts:\n  - name: 测试账号\n    remark: {remark}\n    credentials:\n      cookie: ''\n      stoken: ''\n    tasks:\n      bbs: false\n"
        )
    }

    #[tokio::test]
    async fn backup_keeps_previous_config_before_atomic_replace() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        add_account(
            &path,
            None,
            "account_id=123; account_mid_v2=mid; stoken=v2_secret",
        )
        .await
        .unwrap();
        let name = load(&path).unwrap().config.accounts[0].name.clone();
        set_schedule(&path, true, 60, false, None).unwrap();
        let backups = list_backups(&path).unwrap();
        assert!(!backups.is_empty(), "受控备份应随编辑产生");
        assert!(
            backups
                .iter()
                .all(|backup| backup.version == Some(super::super::CURRENT_CONFIG_VERSION))
        );
        assert_eq!(load(&path).unwrap().config.accounts[0].name, name);
    }
}
