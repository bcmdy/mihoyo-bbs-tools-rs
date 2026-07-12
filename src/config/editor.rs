use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::Command,
};

use serde_yaml_ng::{Mapping, Value};

use crate::auth::CookieJar;

use super::{ConfigError, load, open_secure_new};

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
    fs::write(path, updated).map_err(|source| write_error(path, source))?;
    let _ = fs::remove_file(temporary);
    Ok(())
}

pub fn add_account_from_stdin(path: &Path, name: Option<&str>) -> Result<String, ConfigError> {
    eprintln!("请输入完整 Cookie（输入内容不会写入日志）：");
    let mut cookie = String::new();
    io::stdin()
        .lock()
        .read_line(&mut cookie)
        .map_err(|_| ConfigError::Edit("无法从标准输入读取 Cookie".to_owned()))?;
    let cookie = cookie.trim();
    let jar =
        CookieJar::parse(cookie).map_err(|_| ConfigError::Edit("Cookie 格式无效".to_owned()))?;
    jar.get("stoken")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ConfigError::Edit("Cookie 中缺少 stoken，请重新获取完整 Cookie".to_owned())
        })?;
    let account_name = name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| jar.uid().map(|uid| format!("账号-{}", uid_suffix(uid))))
        .ok_or_else(|| ConfigError::Edit("未提供备注且 Cookie 中缺少 UID".to_owned()))?;

    mutate_raw(path, |root| {
        let accounts = accounts_mut(root)?;
        if accounts
            .iter()
            .any(|account| account_name_of(account) == Some(account_name.as_str()))
        {
            return Err(ConfigError::Edit(format!(
                "账号名称 {account_name:?} 已存在"
            )));
        }
        let mut credentials = Mapping::new();
        credentials.insert(key("cookie"), Value::String(cookie.to_owned()));
        let mut account = Mapping::new();
        account.insert(key("name"), Value::String(account_name.clone()));
        account.insert(key("enabled"), Value::Bool(true));
        account.insert(key("credentials"), Value::Mapping(credentials));
        account.insert(key("device"), Value::Mapping(Mapping::new()));
        account.insert(key("proxy"), Value::Mapping(Mapping::new()));
        account.insert(key("tasks"), default_tasks());
        account.insert(key("games"), default_games());
        accounts.push(Value::Mapping(account));
        Ok(())
    })?;
    Ok(account_name)
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

fn mutate_raw(
    path: &Path,
    mutate: impl FnOnce(&mut Value) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    let source = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut value: Value = serde_yaml_ng::from_str(&source)?;
    mutate(&mut value)?;
    let updated = serde_yaml_ng::to_string(&value).map_err(ConfigError::Serialize)?;
    let temporary = temporary_path(path);
    secure_write_new(&temporary, &updated)?;
    load(&temporary).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        ConfigError::Edit(format!("修改后配置未通过校验，原配置未修改：{error}"))
    })?;
    fs::write(path, updated).map_err(|source| write_error(path, source))?;
    let _ = fs::remove_file(temporary);
    Ok(())
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
    for field in ["enabled", "sign", "read", "like", "cancel_like", "share"] {
        bbs.insert(key(field), Value::Bool(true));
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
    Value::Sequence(
        [
            "genshin",
            "honkai2",
            "honkai3rd",
            "tears_of_themis",
            "star_rail",
            "zenless_zone_zero",
        ]
        .into_iter()
        .map(|game| Value::String(game.to_owned()))
        .collect(),
    )
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
