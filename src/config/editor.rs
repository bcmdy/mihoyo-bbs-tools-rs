use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::Command,
};

use serde_yaml_ng::{Mapping, Value};

use crate::auth::CookieJar;
#[cfg(not(test))]
use reqwest::header::{COOKIE, USER_AGENT};
#[cfg(not(test))]
use serde::Deserialize;
#[cfg(not(test))]
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

pub fn add_account_from_stdin(path: &Path, name: Option<&str>) -> Result<String, ConfigError> {
    eprintln!("请输入完整 Cookie（输入内容不会写入日志）：");
    let mut cookie = String::new();
    io::stdin()
        .lock()
        .read_line(&mut cookie)
        .map_err(|_| ConfigError::Edit("无法从标准输入读取 Cookie".to_owned()))?;
    add_account(path, name, cookie.trim())
}

pub fn add_account(path: &Path, name: Option<&str>, cookie: &str) -> Result<String, ConfigError> {
    let jar =
        CookieJar::parse(cookie).map_err(|_| ConfigError::Edit("Cookie 格式无效".to_owned()))?;
    jar.get("stoken")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ConfigError::Edit("Cookie 中缺少 stoken，请重新获取完整 Cookie".to_owned())
        })?;
    let mut account_name = fetch_nickname(cookie, jar.uid().unwrap_or_default())?;

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
        let mut account = Mapping::new();
        account.insert(key("name"), Value::String(account_name.clone()));
        if let Some(remark) = name.map(str::trim).filter(|value| !value.is_empty()) {
            account.insert(key("remark"), Value::String(remark.to_owned()));
        }
        account.insert(key("enabled"), Value::Bool(true));
        account.insert(key("credentials"), Value::Mapping(credentials));
        account.insert(key("device"), Value::Mapping(Mapping::new()));
        account.insert(key("proxy"), Value::Mapping(Mapping::new()));
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
fn fetch_nickname(cookie: &str, _uid: &str) -> Result<String, ConfigError> {
    if cookie.is_empty() {
        Err(ConfigError::Edit("Cookie 不能为空".to_owned()))
    } else {
        Ok("测试昵称".to_owned())
    }
}

#[cfg(not(test))]
fn fetch_nickname(cookie: &str, uid: &str) -> Result<String, ConfigError> {
    let mut url = Url::parse("https://bbs-api.miyoushe.com/user/wapi/getUserFullInfo").unwrap();
    url.query_pairs_mut()
        .append_pair("gids", "2")
        .append_pair("uid", uid);
    let response: ProfileEnvelope = reqwest::blocking::Client::new()
        .get(url)
        .header(COOKIE, cookie)
        .header(USER_AGENT, "Mozilla/5.0 miHoYoBBS/2.84.1")
        .send()
        .and_then(|v| v.error_for_status())
        .and_then(|v| v.json())
        .map_err(|_| ConfigError::Edit("无法获取米游社昵称，请检查 Cookie 和网络".to_owned()))?;
    let retcode = response.retcode;
    let nickname = response
        .data
        .map(|v| v.user_info.nickname)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| ConfigError::Edit(format!("米游社昵称查询失败（代码 {retcode}）")))?;
    Ok(nickname)
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
        tasks.insert(key("bbs"), Value::Mapping(detail));
        Ok(())
    })
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

pub fn set_runtime(
    path: &Path,
    timezone: &str,
    timeout: u64,
    retry: u32,
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
        runtime.insert(key("random_delay_seconds"), Value::Number(delay.into()));
        runtime.insert(key("log_level"), Value::String(level.into()));
        Ok(())
    })
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
    root.insert(key("runtime"), Value::Mapping(Mapping::new()));
    root.insert(key("captcha"), Value::Mapping(Mapping::new()));
    root.insert(key("accounts"), Value::Sequence(Vec::new()));
    root.insert(key("notifications"), Value::Mapping(Mapping::new()));
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
    for field in ["read", "like", "cancel_like", "share"] {
        bbs.insert(key(field), Value::Bool(false));
    }
    let mut tasks = Mapping::new();
    tasks.insert(key("china_game_checkin"), Value::Bool(true));
    tasks.insert(key("hoyolab_checkin"), Value::Bool(false));
    tasks.insert(key("bbs"), Value::Mapping(bbs));
    tasks.insert(key("china_cloud_game"), Value::Bool(false));
    tasks.insert(key("overseas_cloud_game"), Value::Bool(false));
    tasks.insert(key("web_activity"), Value::Bool(false));
    Value::Mapping(tasks)
}

fn default_games() -> Value {
    Value::Sequence(vec![Value::String("genshin".to_owned())])
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

    #[test]
    fn add_account_creates_missing_parent_and_valid_config() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested/config.yaml");
        let name = add_account(
            &path,
            Some("测试账号"),
            "account_id=123; account_mid_v2=mid; stoken=v2_secret",
        )
        .unwrap();
        assert_eq!(name, "测试昵称");
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
        assert!(!fs::read_to_string(path).unwrap().contains("MIHOYO_COOKIE"));
    }

    #[test]
    fn invalid_cookie_does_not_create_parent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing/config.yaml");
        assert!(add_account(&path, None, "invalid").is_err());
        assert!(!path.parent().unwrap().exists());
    }
}
