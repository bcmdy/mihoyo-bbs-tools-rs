use std::{
    collections::HashSet,
    env, fs,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};
use thiserror::Error;
use url::Url;

use crate::auth::SecretString;

mod legacy;

pub const CURRENT_CONFIG_VERSION: u64 = 1;
pub const EXAMPLE_CONFIG: &str = include_str!("../../config/config.example.yaml");

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub warnings: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("无法读取配置文件 {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("YAML 配置无效: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    #[error("无法序列化配置: {0}")]
    Serialize(serde_yaml_ng::Error),
    #[error("环境变量 {0} 未设置")]
    MissingEnvironmentVariable(String),
    #[error("环境变量占位符无效: {0}")]
    InvalidEnvironmentPlaceholder(String),
    #[error("不支持配置版本 {0}，当前仅支持 version {CURRENT_CONFIG_VERSION}")]
    UnsupportedVersion(u64),
    #[error("迁移输出路径不能与输入配置相同: {0}")]
    OutputMatchesInput(PathBuf),
    #[error("迁移输出文件已存在，拒绝覆盖: {0}")]
    OutputAlreadyExists(PathBuf),
    #[error("迁移输出路径无效: {0}")]
    InvalidOutputPath(PathBuf),
    #[error("无法写入迁移配置 {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("配置校验失败:\n- {}", .0.join("\n- "))]
    Validation(Vec<String>),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub version: u64,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub captcha: CaptchaConfig,
    pub accounts: Vec<AccountConfig>,
    #[serde(default)]
    pub notifications: NotificationsConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default = "default_timeout")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_retry_count")]
    pub retry_count: u32,
    #[serde(default = "default_random_delay")]
    pub random_delay_seconds: u64,
    #[serde(default)]
    pub log_level: LogLevel,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            timezone: default_timezone(),
            request_timeout_seconds: default_timeout(),
            retry_count: default_retry_count(),
            random_delay_seconds: default_random_delay(),
            log_level: LogLevel::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CaptchaConfig {
    #[serde(default)]
    pub endpoint: Option<Url>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AccountConfig {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub credentials: CredentialConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub tasks: TaskConfig,
    #[serde(default)]
    pub games: Vec<Game>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CredentialConfig {
    #[serde(
        deserialize_with = "deserialize_secret",
        serialize_with = "serialize_secret"
    )]
    pub cookie: SecretString,
    #[serde(
        deserialize_with = "deserialize_secret",
        serialize_with = "serialize_secret"
    )]
    pub stoken: SecretString,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ProxyConfig {
    #[serde(
        default,
        deserialize_with = "deserialize_optional_secret",
        serialize_with = "serialize_optional_secret"
    )]
    pub url: Option<SecretString>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TaskConfig {
    #[serde(default)]
    pub china_game_checkin: bool,
    #[serde(default)]
    pub hoyolab_checkin: bool,
    #[serde(default)]
    pub bbs: bool,
    #[serde(default)]
    pub china_cloud_game: bool,
    #[serde(default)]
    pub overseas_cloud_game: bool,
    #[serde(default)]
    pub web_activity: bool,
}

impl TaskConfig {
    fn any_enabled(&self) -> bool {
        self.china_game_checkin
            || self.hoyolab_checkin
            || self.bbs
            || self.china_cloud_game
            || self.overseas_cloud_game
            || self.web_activity
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Game {
    Genshin,
    Honkai2,
    Honkai3rd,
    TearsOfThemis,
    StarRail,
    ZenlessZoneZero,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct NotificationsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub providers: Vec<NotificationProvider>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotificationProvider {
    Telegram {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        bot_token: SecretString,
        chat_id: String,
        #[serde(default)]
        api_url: Option<Url>,
    },
    Webhook {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        url: SecretString,
    },
    Pushplus {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        token: SecretString,
        #[serde(default)]
        topic: Option<String>,
    },
}

pub fn load(path: &Path) -> Result<LoadedConfig, ConfigError> {
    load_with_mode(path, false)
}

pub fn migrate_config(path: &Path) -> Result<LoadedConfig, ConfigError> {
    load_with_mode(path, true)
}

/// 将配置序列化为 YAML。返回内容可能包含明文凭据，调用方不得写入日志。
pub fn to_yaml(config: &Config) -> Result<String, ConfigError> {
    serde_yaml_ng::to_string(config).map_err(ConfigError::Serialize)
}

/// 迁移配置并安全地新建输出文件。此函数拒绝覆盖输入文件或任何已有文件。
pub fn write_migrated_config(
    input: &Path,
    output: &Path,
) -> Result<LoadedConfig, ConfigError> {
    ensure_distinct_new_output(input, output)?;
    let loaded = migrate_config(input)?;
    let yaml = to_yaml(&loaded.config)?;
    let mut file = open_secure_new(output).map_err(|source| {
        if source.kind() == std::io::ErrorKind::AlreadyExists {
            ConfigError::OutputAlreadyExists(output.to_path_buf())
        } else {
            ConfigError::Write {
                path: output.to_path_buf(),
                source,
            }
        }
    })?;
    if let Err(source) = file.write_all(yaml.as_bytes()).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(output);
        return Err(ConfigError::Write {
            path: output.to_path_buf(),
            source,
        });
    }
    Ok(loaded)
}

fn ensure_distinct_new_output(input: &Path, output: &Path) -> Result<(), ConfigError> {
    let input = fs::canonicalize(input).map_err(|source| ConfigError::Read {
        path: input.to_path_buf(),
        source,
    })?;
    if output.exists() {
        let existing = fs::canonicalize(output).map_err(|source| ConfigError::Write {
            path: output.to_path_buf(),
            source,
        })?;
        if existing == input {
            return Err(ConfigError::OutputMatchesInput(output.to_path_buf()));
        }
        return Err(ConfigError::OutputAlreadyExists(output.to_path_buf()));
    }
    let file_name = output
        .file_name()
        .ok_or_else(|| ConfigError::InvalidOutputPath(output.to_path_buf()))?;
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent).map_err(|source| ConfigError::Write {
        path: output.to_path_buf(),
        source,
    })?;
    if parent.join(file_name) == input {
        return Err(ConfigError::OutputMatchesInput(output.to_path_buf()));
    }
    Ok(())
}

fn open_secure_new(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn load_with_mode(path: &Path, migration_requested: bool) -> Result<LoadedConfig, ConfigError> {
    let source = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut value: Value = serde_yaml_ng::from_str(&source)?;
    expand_environment(&mut value, &|name| env::var(name).ok())?;
    if let Some(11..=15) = config_version(&value) {
        let account_name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("legacy");
        let loaded = legacy::migrate_value(&value, account_name)?;
        validate(&loaded.config)?;
        return Ok(loaded);
    }
    reject_unsupported_version(&value)?;
    let warnings = collect_unknown_field_warnings(&value);
    let config: Config = serde_yaml_ng::from_value(value)?;
    validate(&config)?;
    let mut loaded = LoadedConfig { config, warnings };
    if migration_requested {
        loaded
            .warnings
            .push("配置已经是当前 version 1，无需迁移".to_owned());
    }
    Ok(loaded)
}

pub fn validate(config: &Config) -> Result<(), ConfigError> {
    let mut errors = Vec::new();
    if config.version != CURRENT_CONFIG_VERSION {
        return Err(ConfigError::UnsupportedVersion(config.version));
    }
    if !(1..=300).contains(&config.runtime.request_timeout_seconds) {
        errors.push("runtime.request_timeout_seconds 必须在 1..=300 之间".to_owned());
    }
    if config.runtime.retry_count > 10 {
        errors.push("runtime.retry_count 必须在 0..=10 之间".to_owned());
    }
    if config.runtime.random_delay_seconds > 3600 {
        errors.push("runtime.random_delay_seconds 必须在 0..=3600 之间".to_owned());
    }
    if !valid_timezone(&config.runtime.timezone) {
        errors.push("runtime.timezone 不是有效的时区名称".to_owned());
    }
    if let Some(endpoint) = &config.captcha.endpoint {
        if !matches!(endpoint.scheme(), "http" | "https") {
            errors.push("captcha.endpoint 仅支持 http 或 https".to_owned());
        }
    }
    if config.accounts.is_empty() {
        errors.push("accounts 至少需要一个账号".to_owned());
    }
    let mut names = HashSet::new();
    for (index, account) in config.accounts.iter().enumerate() {
        let path = format!("accounts[{index}]");
        let trimmed = account.name.trim();
        if trimmed.is_empty() {
            errors.push(format!("{path}.name 不能为空"));
        } else if trimmed != account.name {
            errors.push(format!("{path}.name 不能包含首尾空白"));
        } else if !names.insert(trimmed) {
            errors.push(format!("账号名称 {trimmed:?} 重复"));
        }
        if let Some(proxy) = &account.proxy.url {
            validate_proxy(
                proxy.expose_secret(),
                &format!("{path}.proxy.url"),
                &mut errors,
            );
        }
        if account.enabled && account.tasks.any_enabled() && account.credentials.cookie.is_empty() {
            errors.push(format!("{path} 启用了任务但 credentials.cookie 为空"));
        }
        if account.enabled && account.tasks.bbs && account.credentials.stoken.is_empty() {
            errors.push(format!("{path} 启用了 BBS 任务但 credentials.stoken 为空"));
        }
    }
    if config.notifications.enabled && config.notifications.providers.is_empty() {
        errors.push("notifications.enabled=true 时必须配置至少一个 provider".to_owned());
    }
    for (index, provider) in config.notifications.providers.iter().enumerate() {
        validate_provider(provider, index, &mut errors);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Validation(errors))
    }
}

fn validate_proxy(raw: &str, path: &str, errors: &mut Vec<String>) {
    if raw.trim().is_empty() {
        errors.push(format!("{path} 不能为空字符串；不使用代理时请设为 null"));
        return;
    }
    let normalized = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("http://{raw}")
    };
    match Url::parse(&normalized) {
        Ok(url) if matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") => {}
        Ok(_) => errors.push(format!("{path} 仅支持 http、https、socks5 或 socks5h")),
        Err(_) => errors.push(format!("{path} 不是有效 URL")),
    }
}

fn validate_provider(provider: &NotificationProvider, index: usize, errors: &mut Vec<String>) {
    let path = format!("notifications.providers[{index}]");
    match provider {
        NotificationProvider::Telegram {
            bot_token,
            chat_id,
            api_url,
        } => {
            if bot_token.is_empty() {
                errors.push(format!("{path}.bot_token 不能为空"));
            }
            if chat_id.trim().is_empty() {
                errors.push(format!("{path}.chat_id 不能为空"));
            }
            if let Some(url) = api_url {
                if !matches!(url.scheme(), "http" | "https") {
                    errors.push(format!("{path}.api_url 仅支持 http 或 https"));
                }
            }
        }
        NotificationProvider::Webhook { url } => match Url::parse(url.expose_secret()) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => {}
            _ => errors.push(format!("{path}.url 必须是 http 或 https URL")),
        },
        NotificationProvider::Pushplus { token, .. } => {
            if token.is_empty() {
                errors.push(format!("{path}.token 不能为空"));
            }
        }
    }
}

fn reject_unsupported_version(value: &Value) -> Result<(), ConfigError> {
    match config_version(value) {
        Some(CURRENT_CONFIG_VERSION) => Ok(()),
        Some(other) => Err(ConfigError::UnsupportedVersion(other)),
        None => Ok(()),
    }
}

fn config_version(value: &Value) -> Option<u64> {
    value
        .as_mapping()
        .and_then(|map| map.get(Value::String("version".to_owned())))
        .and_then(Value::as_u64)
}

fn expand_environment(
    value: &mut Value,
    resolver: &impl Fn(&str) -> Option<String>,
) -> Result<(), ConfigError> {
    match value {
        Value::String(text) => *text = expand_string(text, resolver)?,
        Value::Sequence(values) => {
            for value in values {
                expand_environment(value, resolver)?;
            }
        }
        Value::Mapping(map) => {
            for value in map.values_mut() {
                expand_environment(value, resolver)?;
            }
        }
        Value::Tagged(tagged) => expand_environment(&mut tagged.value, resolver)?,
        _ => {}
    }
    Ok(())
}

fn expand_string(
    text: &str,
    resolver: &impl Fn(&str) -> Option<String>,
) -> Result<String, ConfigError> {
    let mut output = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(start) = remaining.find("${") {
        output.push_str(&remaining[..start]);
        let after = &remaining[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(ConfigError::InvalidEnvironmentPlaceholder(
                remaining[start..].to_owned(),
            ));
        };
        let name = &after[..end];
        let valid_first = name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_');
        if !valid_first || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(ConfigError::InvalidEnvironmentPlaceholder(format!(
                "${{{name}}}"
            )));
        }
        output.push_str(
            &resolver(name)
                .ok_or_else(|| ConfigError::MissingEnvironmentVariable(name.to_owned()))?,
        );
        remaining = &after[end + 1..];
    }
    output.push_str(remaining);
    Ok(output)
}

fn collect_unknown_field_warnings(value: &Value) -> Vec<String> {
    let mut warnings = Vec::new();
    inspect_mapping(
        value,
        "",
        &["version", "runtime", "captcha", "accounts", "notifications"],
        &mut warnings,
    );
    if let Some(map) = value.as_mapping() {
        inspect_child(
            map,
            "runtime",
            &[
                "timezone",
                "request_timeout_seconds",
                "retry_count",
                "random_delay_seconds",
                "log_level",
            ],
            &mut warnings,
        );
        inspect_child(map, "captcha", &["endpoint"], &mut warnings);
        if let Some(Value::Sequence(accounts)) = get(map, "accounts") {
            for (index, account) in accounts.iter().enumerate() {
                let base = format!("accounts[{index}]");
                inspect_mapping(
                    account,
                    &base,
                    &["name", "enabled", "credentials", "proxy", "tasks", "games"],
                    &mut warnings,
                );
                if let Some(account_map) = account.as_mapping() {
                    inspect_named_child(
                        account_map,
                        "credentials",
                        &format!("{base}.credentials"),
                        &["cookie", "stoken"],
                        &mut warnings,
                    );
                    inspect_named_child(
                        account_map,
                        "proxy",
                        &format!("{base}.proxy"),
                        &["url"],
                        &mut warnings,
                    );
                    inspect_named_child(
                        account_map,
                        "tasks",
                        &format!("{base}.tasks"),
                        &[
                            "china_game_checkin",
                            "hoyolab_checkin",
                            "bbs",
                            "china_cloud_game",
                            "overseas_cloud_game",
                            "web_activity",
                        ],
                        &mut warnings,
                    );
                }
            }
        }
        if let Some(Value::Mapping(notifications)) = get(map, "notifications") {
            inspect_mapping(
                &Value::Mapping(notifications.clone()),
                "notifications",
                &["enabled", "providers"],
                &mut warnings,
            );
            if let Some(Value::Sequence(providers)) = get(notifications, "providers") {
                for (index, provider) in providers.iter().enumerate() {
                    let allowed = match provider
                        .as_mapping()
                        .and_then(|m| get(m, "type"))
                        .and_then(Value::as_str)
                    {
                        Some("telegram") => &["type", "bot_token", "chat_id", "api_url"][..],
                        Some("webhook") => &["type", "url"][..],
                        Some("pushplus") => &["type", "token", "topic"][..],
                        _ => &["type"][..],
                    };
                    inspect_mapping(
                        provider,
                        &format!("notifications.providers[{index}]"),
                        allowed,
                        &mut warnings,
                    );
                }
            }
        }
    }
    warnings
}

fn inspect_child(map: &Mapping, key: &str, allowed: &[&str], warnings: &mut Vec<String>) {
    inspect_named_child(map, key, key, allowed, warnings);
}

fn inspect_named_child(
    map: &Mapping,
    key: &str,
    path: &str,
    allowed: &[&str],
    warnings: &mut Vec<String>,
) {
    if let Some(value) = get(map, key) {
        inspect_mapping(value, path, allowed, warnings);
    }
}

fn inspect_mapping(value: &Value, path: &str, allowed: &[&str], warnings: &mut Vec<String>) {
    let Some(map) = value.as_mapping() else {
        return;
    };
    for key in map.keys().filter_map(Value::as_str) {
        if !allowed.contains(&key) {
            let full = if path.is_empty() {
                key.to_owned()
            } else {
                format!("{path}.{key}")
            };
            warnings.push(format!("未知配置字段 {full}"));
        }
    }
}

fn get<'a>(map: &'a Mapping, key: &str) -> Option<&'a Value> {
    map.get(Value::String(key.to_owned()))
}

fn deserialize_secret<'de, D>(deserializer: D) -> Result<SecretString, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(SecretString::new)
}

fn deserialize_optional_secret<'de, D>(deserializer: D) -> Result<Option<SecretString>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(|value| value.map(SecretString::new))
}

fn serialize_secret<S>(secret: &SecretString, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(secret.expose_secret())
}

fn serialize_optional_secret<S>(
    secret: &Option<SecretString>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match secret {
        Some(secret) => serializer.serialize_some(secret.expose_secret()),
        None => serializer.serialize_none(),
    }
}

fn valid_timezone(value: &str) -> bool {
    value == "UTC" || value == "Etc/UTC" || {
        let mut parts = value.split('/');
        let first = parts.next().unwrap_or_default();
        let second = parts.next().unwrap_or_default();
        !first.is_empty()
            && !second.is_empty()
            && parts.next().is_none()
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'))
    }
}

fn default_timezone() -> String {
    "Asia/Shanghai".to_owned()
}
const fn default_timeout() -> u64 {
    30
}
const fn default_retry_count() -> u32 {
    3
}
const fn default_random_delay() -> u64 {
    10
}
const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Result<LoadedConfig, ConfigError> {
        let mut value: Value = serde_yaml_ng::from_str(source)?;
        reject_unsupported_version(&value)?;
        expand_environment(&mut value, &|name| match name {
            "COOKIE" => Some("account_id=123; cookie_token=secret".to_owned()),
            "STOKEN" => Some("v2_secret".to_owned()),
            _ => None,
        })?;
        let warnings = collect_unknown_field_warnings(&value);
        let config: Config = serde_yaml_ng::from_value(value)?;
        validate(&config)?;
        Ok(LoadedConfig { config, warnings })
    }

    const MINIMAL: &str = r#"
version: 1
accounts:
  - name: first
    credentials:
      cookie: "${COOKIE}"
      stoken: "${STOKEN}"
    tasks:
      bbs: true
"#;

    #[test]
    fn applies_defaults_and_expands_environment() {
        let loaded = parse(MINIMAL).unwrap();
        assert_eq!(loaded.config.runtime.request_timeout_seconds, 30);
        assert_eq!(loaded.config.runtime.timezone, "Asia/Shanghai");
        assert_eq!(
            loaded.config.accounts[0].credentials.cookie.expose_secret(),
            "account_id=123; cookie_token=secret"
        );
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn missing_environment_variable_is_explicit_and_redacted() {
        let error = parse(&MINIMAL.replace("${COOKIE}", "${MISSING_SECRET}")).unwrap_err();
        assert!(
            matches!(error, ConfigError::MissingEnvironmentVariable(ref name) if name == "MISSING_SECRET")
        );
        assert!(!error.to_string().contains("cookie_token"));
    }

    #[test]
    fn warns_about_unknown_fields_at_nested_paths() {
        let source = MINIMAL
            .replace("    tasks:", "    unexpected: true\n    tasks:")
            .replace("      bbs: true", "      bbs: true\n      typo: true");
        let loaded = parse(&source).unwrap();
        assert_eq!(
            loaded.warnings,
            vec![
                "未知配置字段 accounts[0].unexpected",
                "未知配置字段 accounts[0].tasks.typo"
            ]
        );
    }

    #[test]
    fn rejects_duplicate_account_names() {
        let source = format!(
            "{MINIMAL}\n  - name: first\n    credentials:\n      cookie: x\n      stoken: y\n"
        );
        assert!(
            matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.iter().any(|e| e.contains("重复")))
        );
    }

    #[test]
    fn rejects_unsafe_protocols_and_timeout() {
        let source = MINIMAL.replace("accounts:", "runtime:\n  request_timeout_seconds: 0\ncaptcha:\n  endpoint: file:///tmp/captcha\naccounts:")
            .replace("    credentials:", "    proxy:\n      url: ftp://example.com\n    credentials:");
        assert!(
            matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.len() == 3)
        );
    }

    #[test]
    fn unknown_versions_are_not_silently_accepted() {
        let source = MINIMAL.replace("version: 1", "version: 99");
        assert!(matches!(
            parse(&source),
            Err(ConfigError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn unknown_notification_provider_is_an_error() {
        let source = format!(
            "{MINIMAL}\nnotifications:\n  enabled: true\n  providers:\n    - type: unknown\n"
        );
        assert!(matches!(parse(&source), Err(ConfigError::Yaml(_))));
    }

    #[test]
    fn config_debug_does_not_expose_secrets() {
        let loaded = parse(MINIMAL).unwrap();
        let debug = format!("{:?}", loaded.config);
        assert!(!debug.contains("cookie_token=secret"));
        assert!(!debug.contains("v2_secret"));
    }

    #[test]
    fn public_migrate_api_uses_file_name_for_legacy_account() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture-account.yaml");
        std::fs::write(&path, include_str!("fixtures/legacy_v15.yaml")).unwrap();
        let migrated = migrate_config(&path).unwrap();
        assert_eq!(migrated.config.accounts[0].name, "fixture-account");
        assert_eq!(migrated.config.version, CURRENT_CONFIG_VERSION);
    }

    #[test]
    fn yaml_serialization_preserves_secrets_without_exposing_them_in_debug() {
        let loaded = parse(MINIMAL).unwrap();
        let yaml = to_yaml(&loaded.config).unwrap();
        assert!(yaml.contains("cookie_token=secret"));
        assert!(yaml.contains("v2_secret"));
        let debug = format!("{:?}", loaded.config);
        assert!(!debug.contains("cookie_token=secret"));
        assert!(!debug.contains("v2_secret"));
    }

    #[test]
    fn writes_migrated_config_without_overwriting_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("legacy.yaml");
        let output = directory.path().join("migrated.yaml");
        std::fs::write(&input, include_str!("fixtures/legacy_v15.yaml")).unwrap();

        let migrated = write_migrated_config(&input, &output).unwrap();
        assert_eq!(migrated.config.version, CURRENT_CONFIG_VERSION);
        let text = std::fs::read_to_string(&output).unwrap();
        assert!(text.contains("fixture-cookie-token"));
        let reloaded = load(&output).unwrap();
        assert_eq!(reloaded.config.accounts[0].name, "legacy");
        assert!(matches!(
            write_migrated_config(&input, &output),
            Err(ConfigError::OutputAlreadyExists(_))
        ));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&output).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn refuses_to_overwrite_input_config() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("legacy.yaml");
        std::fs::write(&input, include_str!("fixtures/legacy_v15.yaml")).unwrap();
        assert!(matches!(
            write_migrated_config(&input, &input),
            Err(ConfigError::OutputMatchesInput(_))
        ));
    }
}
