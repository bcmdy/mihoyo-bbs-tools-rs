use std::{
    collections::HashSet,
    env, fs,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use lettre::message::Mailbox;
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};
use thiserror::Error;
use url::Url;

use crate::auth::{CookieJar, SecretString};

mod dacapo;
mod editor;
mod interactive;
mod legacy;
pub use dacapo::{DacapoError, load_dacapo};
pub use editor::{
    add_account_from_stdin, edit_file, persist_refreshed_cookie, remove_account,
    remove_notification_provider, replace_account_cookie, set_account_china_checkin,
    set_account_cloud_games, set_account_device, set_account_games, set_account_general,
    set_account_hoyolab, set_account_proxy, set_account_tasks, set_captcha_endpoint, set_logging,
    set_notification_options, set_notification_provider, set_runtime, set_schedule,
};
pub use interactive::setup as interactive_setup;

pub const CURRENT_CONFIG_VERSION: u64 = 1;
pub const EXAMPLE_CONFIG: &str = include_str!("../../config/config.example.yaml");

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub warnings: Vec<String>,
    pub source: ConfigSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigSource {
    Current,
    PythonLegacy(u64),
    Dacapo,
    StandardInput,
}

impl ConfigSource {
    pub const fn supports_persistent_refresh(self) -> bool {
        matches!(self, Self::Current)
    }
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
    #[error("配置编辑失败：{0}")]
    Edit(String),
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
    #[serde(default = "default_game_checkin_max_attempts")]
    pub game_checkin_max_attempts: u32,
    #[serde(default = "default_random_delay")]
    pub random_delay_seconds: u64,
    #[serde(default)]
    pub log_level: LogLevel,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub schedule: ScheduleConfig,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            timezone: default_timezone(),
            request_timeout_seconds: default_timeout(),
            retry_count: default_retry_count(),
            game_checkin_max_attempts: default_game_checkin_max_attempts(),
            random_delay_seconds: default_random_delay(),
            log_level: LogLevel::default(),
            logging: LoggingConfig::default(),
            schedule: ScheduleConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduleConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_schedule_interval")]
    pub interval_minutes: u64,
    #[serde(default = "default_true")]
    pub run_on_start: bool,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: default_schedule_interval(),
            run_on_start: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LoggingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_log_directory")]
    pub directory: PathBuf,
    #[serde(default = "default_log_prefix")]
    pub file_prefix: String,
}
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: PathBuf::from("logs"),
            file_prefix: default_log_prefix(),
        }
    }
}
fn default_log_directory() -> PathBuf {
    PathBuf::from("logs")
}
fn default_log_prefix() -> String {
    "mihoyo-bbs-tools".to_owned()
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
    #[serde(default)]
    pub remark: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub credentials: CredentialConfig,
    #[serde(default)]
    pub device: DeviceConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub china_checkin: ChinaCheckinConfig,
    #[serde(default)]
    pub hoyolab: Option<HoyolabConfig>,
    #[serde(default)]
    pub cloud_games: CloudGamesConfig,
    #[serde(default)]
    pub tasks: TaskConfig,
    #[serde(default)]
    pub games: Vec<Game>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeviceConfig {
    #[serde(default = "default_device_name")]
    pub name: String,
    #[serde(default = "default_device_model")]
    pub model: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub fp: String,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            name: default_device_name(),
            model: default_device_model(),
            id: String::new(),
            fp: String::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CredentialConfig {
    #[serde(
        deserialize_with = "deserialize_secret",
        serialize_with = "serialize_secret"
    )]
    pub cookie: SecretString,
    #[serde(
        default,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChinaCheckinConfig {
    #[serde(default = "default_china_user_agent")]
    pub user_agent: String,
    #[serde(default)]
    pub role_blacklist: RoleBlacklistConfig,
}

impl Default for ChinaCheckinConfig {
    fn default() -> Self {
        Self {
            user_agent: default_china_user_agent(),
            role_blacklist: RoleBlacklistConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RoleBlacklistConfig {
    #[serde(default)]
    pub genshin: Vec<String>,
    #[serde(default)]
    pub honkai2: Vec<String>,
    #[serde(default)]
    pub honkai3rd: Vec<String>,
    #[serde(default)]
    pub tears_of_themis: Vec<String>,
    #[serde(default)]
    pub star_rail: Vec<String>,
    #[serde(default)]
    pub zenless_zone_zero: Vec<String>,
}

impl RoleBlacklistConfig {
    pub fn for_game(&self, game: Game) -> &[String] {
        match game {
            Game::Genshin => &self.genshin,
            Game::Honkai2 => &self.honkai2,
            Game::Honkai3rd => &self.honkai3rd,
            Game::TearsOfThemis => &self.tears_of_themis,
            Game::StarRail => &self.star_rail,
            Game::ZenlessZoneZero => &self.zenless_zone_zero,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HoyolabConfig {
    #[serde(
        default,
        deserialize_with = "deserialize_secret",
        serialize_with = "serialize_secret"
    )]
    pub cookie: SecretString,
    #[serde(default = "default_hoyolab_language")]
    pub language: String,
    #[serde(default = "default_hoyolab_user_agent")]
    pub user_agent: String,
    #[serde(default)]
    pub games: Vec<Game>,
}

impl Default for HoyolabConfig {
    fn default() -> Self {
        Self {
            cookie: SecretString::default(),
            language: default_hoyolab_language(),
            user_agent: default_hoyolab_user_agent(),
            games: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CloudGamesConfig {
    #[serde(default)]
    pub china: ChinaCloudGamesConfig,
    #[serde(default)]
    pub overseas: OverseasCloudGamesConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ChinaCloudGamesConfig {
    #[serde(default)]
    pub genshin: CloudGameEntryConfig,
    #[serde(default)]
    pub zenless_zone_zero: CloudGameEntryConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CloudGameEntryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_secret",
        serialize_with = "serialize_optional_secret"
    )]
    pub token: Option<SecretString>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OverseasCloudGamesConfig {
    #[serde(default = "default_hoyolab_language")]
    pub language: String,
    #[serde(default)]
    pub genshin: CloudGameEntryConfig,
}

impl Default for OverseasCloudGamesConfig {
    fn default() -> Self {
        Self {
            language: default_hoyolab_language(),
            genshin: CloudGameEntryConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TaskConfig {
    #[serde(default)]
    pub china_game_checkin: bool,
    #[serde(default)]
    pub hoyolab_checkin: bool,
    #[serde(default)]
    pub bbs: BbsTaskConfig,
    #[serde(default)]
    pub china_cloud_game: bool,
    #[serde(default)]
    pub overseas_cloud_game: bool,
    #[serde(default)]
    pub web_activity: WebActivityTaskConfig,
}

impl TaskConfig {
    fn requires_primary_cookie(&self) -> bool {
        self.china_game_checkin || self.bbs.is_enabled() || self.web_activity.enabled
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WebActivityTaskConfig {
    pub enabled: bool,
    pub activities: Vec<WebActivity>,
}

impl Default for WebActivityTaskConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            activities: vec![WebActivity::GenshinMizone],
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebActivity {
    GenshinMizone,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WebActivityTaskConfigInput {
    Bool(bool),
    Detail(WebActivityTaskConfigDetail),
}

#[derive(Deserialize)]
struct WebActivityTaskConfigDetail {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default = "default_web_activities")]
    activities: Vec<WebActivity>,
}

impl<'de> Deserialize<'de> for WebActivityTaskConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(
            match WebActivityTaskConfigInput::deserialize(deserializer)? {
                WebActivityTaskConfigInput::Bool(enabled) => Self {
                    enabled,
                    activities: default_web_activities(),
                },
                WebActivityTaskConfigInput::Detail(detail) => Self {
                    enabled: detail.enabled,
                    activities: detail.activities,
                },
            },
        )
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
    pub error_only: bool,
    #[serde(default)]
    pub block_keywords: Vec<String>,
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
        #[serde(
            default = "default_telegram_api_url",
            deserialize_with = "deserialize_telegram_api_url"
        )]
        api_url: Url,
        #[serde(
            default,
            deserialize_with = "deserialize_optional_secret",
            serialize_with = "serialize_optional_secret"
        )]
        proxy: Option<SecretString>,
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
    Ftqq {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        sendkey: SecretString,
        #[serde(default)]
        api_url: Option<Url>,
    },
    Pushme {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        token: SecretString,
        #[serde(default)]
        api_url: Option<Url>,
    },
    Cqhttp {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        url: SecretString,
        #[serde(
            default,
            deserialize_with = "deserialize_optional_secret",
            serialize_with = "serialize_optional_secret"
        )]
        qq: Option<SecretString>,
        #[serde(
            default,
            deserialize_with = "deserialize_optional_secret",
            serialize_with = "serialize_optional_secret"
        )]
        group: Option<SecretString>,
    },
    Wecom {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        corp_id: SecretString,
        agent_id: String,
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        secret: SecretString,
        #[serde(default = "default_wecom_to_user")]
        to_user: String,
        #[serde(default)]
        api_url: Option<Url>,
    },
    Wecomrobot {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        url: SecretString,
        #[serde(
            default,
            deserialize_with = "deserialize_optional_secret",
            serialize_with = "serialize_optional_secret"
        )]
        mobile: Option<SecretString>,
    },
    Pushdeer {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        token: SecretString,
        #[serde(default)]
        api_url: Option<Url>,
    },
    Dingrobot {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        webhook: SecretString,
        #[serde(
            default,
            deserialize_with = "deserialize_optional_secret",
            serialize_with = "serialize_optional_secret"
        )]
        secret: Option<SecretString>,
    },
    Feishubot {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        webhook: SecretString,
    },
    Bark {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        token: SecretString,
        #[serde(default)]
        api_url: Option<Url>,
        #[serde(default)]
        icon: Option<String>,
    },
    Gotify {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        token: SecretString,
        api_url: Url,
        #[serde(default)]
        priority: i64,
    },
    Ifttt {
        event: String,
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        key: SecretString,
        #[serde(default)]
        api_url: Option<Url>,
    },
    Qmsg {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        key: SecretString,
        #[serde(default)]
        api_url: Option<Url>,
    },
    Discord {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        webhook: SecretString,
    },
    Wxpusher {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        app_token: SecretString,
        #[serde(default)]
        uids: Vec<String>,
        #[serde(default)]
        topic_ids: Vec<i64>,
        #[serde(default)]
        api_url: Option<Url>,
    },
    Serverchan3 {
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        sendkey: SecretString,
        #[serde(default)]
        tags: Option<String>,
    },
    Smtp {
        host: String,
        port: u16,
        from: String,
        to: String,
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        username: SecretString,
        #[serde(
            deserialize_with = "deserialize_secret",
            serialize_with = "serialize_secret"
        )]
        password: SecretString,
        subject: String,
        #[serde(default = "default_smtp_tls")]
        tls: SmtpTlsMode,
        #[serde(default)]
        timeout_seconds: Option<u64>,
    },
    WindowsToast {
        #[serde(default = "default_windows_toast_title_prefix")]
        title_prefix: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpTlsMode {
    None,
    Starttls,
    Implicit,
}

fn default_smtp_tls() -> SmtpTlsMode {
    SmtpTlsMode::Implicit
}

fn default_windows_toast_title_prefix() -> String {
    "MihoyoBBSTools RS".to_owned()
}

fn default_wecom_to_user() -> String {
    "@all".to_owned()
}

pub fn load(path: &Path) -> Result<LoadedConfig, ConfigError> {
    load_with_mode(path, false)
}

pub fn load_from_str(source: &str, account_name: &str) -> Result<LoadedConfig, ConfigError> {
    parse_source(
        source,
        if account_name.trim().is_empty() {
            "stdin"
        } else {
            account_name.trim()
        },
        false,
        ConfigSource::StandardInput,
    )
}

pub fn migrate_config(path: &Path) -> Result<LoadedConfig, ConfigError> {
    load_with_mode(path, true)
}

/// 将配置序列化为 YAML。返回内容可能包含明文凭据，调用方不得写入日志。
pub fn to_yaml(config: &Config) -> Result<String, ConfigError> {
    serde_yaml_ng::to_string(config).map_err(ConfigError::Serialize)
}

/// 迁移配置并安全地新建输出文件。此函数拒绝覆盖输入文件或任何已有文件。
pub fn write_migrated_config(input: &Path, output: &Path) -> Result<LoadedConfig, ConfigError> {
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
    if let Err(source) = file
        .write_all(yaml.as_bytes())
        .and_then(|()| file.sync_all())
    {
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
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
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
    let account_name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("legacy");
    parse_source(
        &source,
        account_name,
        migration_requested,
        ConfigSource::Current,
    )
}

fn parse_source(
    source: &str,
    account_name: &str,
    migration_requested: bool,
    current_source: ConfigSource,
) -> Result<LoadedConfig, ConfigError> {
    let mut value: Value = serde_yaml_ng::from_str(&source)?;
    expand_environment(&mut value, &|name| env::var(name).ok())?;
    if let Some(11..=15) = config_version(&value) {
        let loaded = legacy::migrate_value(&value, account_name)?;
        validate(&loaded.config)?;
        return Ok(loaded);
    }
    reject_unsupported_version(&value)?;
    let warnings = collect_unknown_field_warnings(&value);
    let mut config: Config = serde_yaml_ng::from_value(value)?;
    hydrate_stokens_from_cookies(&mut config);
    validate(&config)?;
    let mut loaded = LoadedConfig {
        config,
        warnings,
        source: current_source,
    };
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
    if !(1..=10).contains(&config.runtime.game_checkin_max_attempts) {
        errors.push("runtime.game_checkin_max_attempts 必须在 1..=10 之间".to_owned());
    }
    if config.runtime.random_delay_seconds > 3600 {
        errors.push("runtime.random_delay_seconds 必须在 0..=3600 之间".to_owned());
    }
    if !(1..=10_080).contains(&config.runtime.schedule.interval_minutes) {
        errors.push("runtime.schedule.interval_minutes 必须在 1..=10080 之间".to_owned());
    }
    if !valid_timezone(&config.runtime.timezone) {
        errors.push("runtime.timezone 不是有效的时区名称".to_owned());
    }
    if config.runtime.logging.file_prefix.trim().is_empty() {
        errors.push("runtime.logging.file_prefix 不能为空".to_owned());
    }
    if config.runtime.logging.directory.as_os_str().is_empty() {
        errors.push("runtime.logging.directory 不能为空".to_owned());
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
        validate_cloud_games(account, &path, &mut errors);
        validate_checkin_regions(account, &path, &mut errors);
        if account.enabled
            && account.tasks.requires_primary_cookie()
            && account.credentials.cookie.is_empty()
        {
            errors.push(format!("{path} 启用了任务但 credentials.cookie 为空"));
        }
        if account.enabled
            && account.tasks.bbs.is_enabled()
            && account.credentials.stoken.is_empty()
        {
            errors.push(format!("{path} 启用了 BBS 任务但 credentials.stoken 为空"));
        }
        if account.tasks.bbs.is_enabled() && account.tasks.bbs.forums.is_empty() {
            errors.push(format!(
                "{path}.tasks.bbs.forums 不能为空；社区签到和帖子任务至少需要一个板块"
            ));
        }
        let mut forums = HashSet::new();
        for forum in &account.tasks.bbs.forums {
            if crate::bbs::forum_by_id(*forum).is_none() {
                errors.push(format!(
                    "{path}.tasks.bbs.forums 包含不支持的板块 ID {forum}"
                ));
            } else if !forums.insert(*forum) {
                errors.push(format!("{path}.tasks.bbs.forums 包含重复的板块 ID {forum}"));
            }
        }
        let mut activities = HashSet::new();
        for activity in &account.tasks.web_activity.activities {
            if !activities.insert(*activity) {
                errors.push(format!(
                    "{path}.tasks.web_activity.activities 包含重复活动 {activity:?}"
                ));
            }
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

fn validate_cloud_games(account: &AccountConfig, path: &str, errors: &mut Vec<String>) {
    let china = &account.cloud_games.china;
    let overseas = &account.cloud_games.overseas;
    for (field, entry) in [
        ("china.genshin", &china.genshin),
        ("china.zenless_zone_zero", &china.zenless_zone_zero),
        ("overseas.genshin", &overseas.genshin),
    ] {
        if entry
            .token
            .as_ref()
            .is_some_and(|token| token.expose_secret().trim().is_empty())
        {
            errors.push(format!("{path}.cloud_games.{field}.token 不能为空字符串"));
        }
        if entry.enabled && entry.token.is_none() {
            errors.push(format!(
                "{path}.cloud_games.{field}.enabled=true 时必须配置 token"
            ));
        }
    }
    if !matches!(
        overseas.language.as_str(),
        "zh-cn" | "en-us" | "ja-jp" | "ko-kr"
    ) {
        errors.push(format!(
            "{path}.cloud_games.overseas.language 必须是 zh-cn、en-us、ja-jp 或 ko-kr"
        ));
    }
}

fn validate_checkin_regions(account: &AccountConfig, path: &str, errors: &mut Vec<String>) {
    if account.china_checkin.user_agent.trim().is_empty() {
        errors.push(format!("{path}.china_checkin.user_agent 不能为空"));
    }
    validate_game_list(&account.games, &format!("{path}.games"), true, errors);
    for (game, values) in [
        ("genshin", &account.china_checkin.role_blacklist.genshin),
        ("honkai2", &account.china_checkin.role_blacklist.honkai2),
        ("honkai3rd", &account.china_checkin.role_blacklist.honkai3rd),
        (
            "tears_of_themis",
            &account.china_checkin.role_blacklist.tears_of_themis,
        ),
        ("star_rail", &account.china_checkin.role_blacklist.star_rail),
        (
            "zenless_zone_zero",
            &account.china_checkin.role_blacklist.zenless_zone_zero,
        ),
    ] {
        let mut seen = HashSet::new();
        for uid in values {
            if uid.trim().is_empty() {
                errors.push(format!(
                    "{path}.china_checkin.role_blacklist.{game} 不能包含空 UID"
                ));
            } else if uid.trim() != uid {
                errors.push(format!(
                    "{path}.china_checkin.role_blacklist.{game} 的 UID 不能包含首尾空白"
                ));
            } else if !uid.chars().all(|character| character.is_ascii_digit()) {
                errors.push(format!(
                    "{path}.china_checkin.role_blacklist.{game} 的 UID 必须只包含数字"
                ));
            } else if !seen.insert(uid) {
                errors.push(format!(
                    "{path}.china_checkin.role_blacklist.{game} 包含重复 UID {uid}"
                ));
            }
        }
    }

    match &account.hoyolab {
        Some(hoyolab) => {
            if hoyolab.user_agent.trim().is_empty() {
                errors.push(format!("{path}.hoyolab.user_agent 不能为空"));
            }
            if !matches!(
                hoyolab.language.as_str(),
                "zh-cn" | "en-us" | "ja-jp" | "ko-kr"
            ) {
                errors.push(format!(
                    "{path}.hoyolab.language 必须是 zh-cn、en-us、ja-jp 或 ko-kr"
                ));
            }
            validate_game_list(
                &hoyolab.games,
                &format!("{path}.hoyolab.games"),
                false,
                errors,
            );
            if !hoyolab.cookie.is_empty() && hoyolab.cookie.expose_secret().trim().is_empty() {
                errors.push(format!("{path}.hoyolab.cookie 不能只包含空白"));
            }
            if account.enabled
                && account.tasks.hoyolab_checkin
                && hoyolab.cookie.expose_secret().trim().is_empty()
            {
                errors.push(format!(
                    "{path}.tasks.hoyolab_checkin 已启用，但 hoyolab.cookie 为空"
                ));
            }
            if account.enabled && account.tasks.hoyolab_checkin && hoyolab.games.is_empty() {
                errors.push(format!(
                    "{path}.tasks.hoyolab_checkin 已启用，但 hoyolab.games 为空"
                ));
            }
        }
        None if account.enabled && account.tasks.hoyolab_checkin => {
            if account.credentials.cookie.is_empty() {
                errors.push(format!(
                    "{path}.tasks.hoyolab_checkin 使用兼容配置，但 credentials.cookie 为空"
                ));
            }
            if account.games.is_empty() {
                errors.push(format!(
                    "{path}.tasks.hoyolab_checkin 使用兼容配置，但 games 为空"
                ));
            }
        }
        None => {}
    }
}

fn validate_game_list(games: &[Game], path: &str, allow_honkai2: bool, errors: &mut Vec<String>) {
    let mut seen = HashSet::new();
    for game in games {
        if !allow_honkai2 && *game == Game::Honkai2 {
            errors.push(format!("{path} 不支持 honkai2"));
        } else if !seen.insert(*game) {
            errors.push(format!("{path} 包含重复游戏 {game:?}"));
        }
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
            proxy,
        } => {
            if bot_token.is_empty() {
                errors.push(format!("{path}.bot_token 不能为空"));
            }
            if chat_id.trim().is_empty() {
                errors.push(format!("{path}.chat_id 不能为空"));
            }
            if !matches!(api_url.scheme(), "http" | "https") {
                errors.push(format!("{path}.api_url 仅支持 http 或 https"));
            }
            if let Some(proxy) = proxy {
                validate_proxy(proxy.expose_secret(), &format!("{path}.proxy"), errors);
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
        NotificationProvider::Ftqq { sendkey, api_url } => {
            validate_required_secret(sendkey, &format!("{path}.sendkey"), errors);
            validate_optional_http_url(api_url.as_ref(), &format!("{path}.api_url"), errors);
        }
        NotificationProvider::Pushme { token, api_url }
        | NotificationProvider::Pushdeer { token, api_url }
        | NotificationProvider::Bark { token, api_url, .. } => {
            validate_required_secret(token, &format!("{path}.token"), errors);
            validate_optional_http_url(api_url.as_ref(), &format!("{path}.api_url"), errors);
        }
        NotificationProvider::Cqhttp { url, qq, group } => {
            validate_secret_http_url(url, &format!("{path}.url"), errors);
            match (qq, group) {
                (Some(_), Some(_)) => errors.push(format!("{path}.qq 与 group 只能配置一个")),
                (None, None) => errors.push(format!("{path}.qq 与 group 必须配置一个")),
                (Some(value), None) => {
                    validate_required_secret(value, &format!("{path}.qq"), errors)
                }
                (None, Some(value)) => {
                    validate_required_secret(value, &format!("{path}.group"), errors)
                }
            }
        }
        NotificationProvider::Wecom {
            corp_id,
            agent_id,
            secret,
            to_user,
            api_url,
        } => {
            validate_required_secret(corp_id, &format!("{path}.corp_id"), errors);
            if agent_id.trim().is_empty() {
                errors.push(format!("{path}.agent_id 不能为空"));
            }
            validate_required_secret(secret, &format!("{path}.secret"), errors);
            if to_user.trim().is_empty() {
                errors.push(format!("{path}.to_user 不能为空"));
            }
            validate_optional_http_url(api_url.as_ref(), &format!("{path}.api_url"), errors);
        }
        NotificationProvider::Wecomrobot { url, mobile } => {
            validate_secret_http_url(url, &format!("{path}.url"), errors);
            if let Some(mobile) = mobile {
                validate_required_secret(mobile, &format!("{path}.mobile"), errors);
            }
        }
        NotificationProvider::Dingrobot { webhook, secret } => {
            validate_secret_http_url(webhook, &format!("{path}.webhook"), errors);
            if let Some(secret) = secret {
                validate_required_secret(secret, &format!("{path}.secret"), errors);
            }
        }
        NotificationProvider::Feishubot { webhook } | NotificationProvider::Discord { webhook } => {
            validate_secret_http_url(webhook, &format!("{path}.webhook"), errors);
        }
        NotificationProvider::Gotify { token, api_url, .. } => {
            validate_required_secret(token, &format!("{path}.token"), errors);
            validate_http_url(api_url, &format!("{path}.api_url"), errors);
        }
        NotificationProvider::Ifttt {
            event,
            key,
            api_url,
        } => {
            if event.trim().is_empty() {
                errors.push(format!("{path}.event 不能为空"));
            }
            validate_required_secret(key, &format!("{path}.key"), errors);
            validate_optional_http_url(api_url.as_ref(), &format!("{path}.api_url"), errors);
        }
        NotificationProvider::Qmsg { key, api_url } => {
            validate_required_secret(key, &format!("{path}.key"), errors);
            validate_optional_http_url(api_url.as_ref(), &format!("{path}.api_url"), errors);
        }
        NotificationProvider::Wxpusher {
            app_token,
            uids,
            topic_ids,
            api_url,
        } => {
            validate_required_secret(app_token, &format!("{path}.app_token"), errors);
            if uids.is_empty() && topic_ids.is_empty() {
                errors.push(format!("{path}.uids 与 topic_ids 至少配置一个接收目标"));
            }
            if uids.iter().any(|uid| uid.trim().is_empty()) {
                errors.push(format!("{path}.uids 不能包含空值"));
            }
            validate_optional_http_url(api_url.as_ref(), &format!("{path}.api_url"), errors);
        }
        NotificationProvider::Serverchan3 { sendkey, .. } => {
            validate_required_secret(sendkey, &format!("{path}.sendkey"), errors);
        }
        NotificationProvider::Smtp {
            host,
            port,
            from,
            to,
            username,
            password,
            subject,
            timeout_seconds,
            ..
        } => {
            if host.trim().is_empty()
                || host.trim() != host
                || host.contains("://")
                || host.contains('/')
                || host.contains('\\')
                || host.chars().any(char::is_whitespace)
            {
                errors.push(format!("{path}.host 必须是无协议和路径的 SMTP 主机名"));
            }
            if *port == 0 {
                errors.push(format!("{path}.port 必须大于 0"));
            }
            if from.parse::<Mailbox>().is_err() {
                errors.push(format!("{path}.from 不是有效邮箱地址"));
            }
            if to.parse::<Mailbox>().is_err() {
                errors.push(format!("{path}.to 不是有效邮箱地址"));
            }
            validate_required_secret(username, &format!("{path}.username"), errors);
            validate_required_secret(password, &format!("{path}.password"), errors);
            if subject.trim().is_empty() {
                errors.push(format!("{path}.subject 不能为空"));
            }
            if timeout_seconds.is_some_and(|timeout| !(1..=300).contains(&timeout)) {
                errors.push(format!("{path}.timeout_seconds 必须在 1 到 300 之间"));
            }
        }
        NotificationProvider::WindowsToast { title_prefix } => {
            if title_prefix.chars().any(|character| character.is_control()) {
                errors.push(format!("{path}.title_prefix 不能包含控制字符"));
            }
        }
    }
}

fn validate_required_secret(secret: &SecretString, path: &str, errors: &mut Vec<String>) {
    if secret.expose_secret().trim().is_empty() {
        errors.push(format!("{path} 不能为空"));
    }
}

fn validate_secret_http_url(secret: &SecretString, path: &str, errors: &mut Vec<String>) {
    match Url::parse(secret.expose_secret()) {
        Ok(url) => validate_http_url(&url, path, errors),
        Err(_) => errors.push(format!("{path} 必须是 http 或 https URL")),
    }
}

fn validate_optional_http_url(url: Option<&Url>, path: &str, errors: &mut Vec<String>) {
    if let Some(url) = url {
        validate_http_url(url, path, errors);
    }
}

fn validate_http_url(url: &Url, path: &str, errors: &mut Vec<String>) {
    if !matches!(url.scheme(), "http" | "https") {
        errors.push(format!("{path} 仅支持 http 或 https"));
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
                "game_checkin_max_attempts",
                "random_delay_seconds",
                "log_level",
                "logging",
                "schedule",
            ],
            &mut warnings,
        );
        if let Some(Value::Mapping(runtime)) = get(map, "runtime") {
            inspect_named_child(
                runtime,
                "logging",
                "runtime.logging",
                &["enabled", "directory", "file_prefix"],
                &mut warnings,
            );
            inspect_named_child(
                runtime,
                "schedule",
                "runtime.schedule",
                &["enabled", "interval_minutes", "run_on_start"],
                &mut warnings,
            );
        }
        inspect_child(map, "captcha", &["endpoint"], &mut warnings);
        if let Some(Value::Sequence(accounts)) = get(map, "accounts") {
            for (index, account) in accounts.iter().enumerate() {
                let base = format!("accounts[{index}]");
                inspect_mapping(
                    account,
                    &base,
                    &[
                        "name",
                        "remark",
                        "enabled",
                        "credentials",
                        "device",
                        "proxy",
                        "china_checkin",
                        "hoyolab",
                        "cloud_games",
                        "tasks",
                        "games",
                    ],
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
                    if let Some(Value::Mapping(cloud_games)) = get(account_map, "cloud_games") {
                        inspect_mapping(
                            &Value::Mapping(cloud_games.clone()),
                            &format!("{base}.cloud_games"),
                            &["china", "overseas"],
                            &mut warnings,
                        );
                        inspect_named_child(
                            cloud_games,
                            "china",
                            &format!("{base}.cloud_games.china"),
                            &["genshin", "zenless_zone_zero"],
                            &mut warnings,
                        );
                        inspect_named_child(
                            cloud_games,
                            "overseas",
                            &format!("{base}.cloud_games.overseas"),
                            &["language", "genshin"],
                            &mut warnings,
                        );
                        if let Some(Value::Mapping(china)) = get(cloud_games, "china") {
                            inspect_named_child(
                                china,
                                "genshin",
                                &format!("{base}.cloud_games.china.genshin"),
                                &["enabled", "token"],
                                &mut warnings,
                            );
                            inspect_named_child(
                                china,
                                "zenless_zone_zero",
                                &format!("{base}.cloud_games.china.zenless_zone_zero"),
                                &["enabled", "token"],
                                &mut warnings,
                            );
                        }
                        if let Some(Value::Mapping(overseas)) = get(cloud_games, "overseas") {
                            inspect_named_child(
                                overseas,
                                "genshin",
                                &format!("{base}.cloud_games.overseas.genshin"),
                                &["enabled", "token"],
                                &mut warnings,
                            );
                        }
                    }
                    inspect_named_child(
                        account_map,
                        "device",
                        &format!("{base}.device"),
                        &["id", "name", "model", "fp"],
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
                        "china_checkin",
                        &format!("{base}.china_checkin"),
                        &["user_agent", "role_blacklist"],
                        &mut warnings,
                    );
                    if let Some(Value::Mapping(china_checkin)) = get(account_map, "china_checkin") {
                        inspect_named_child(
                            china_checkin,
                            "role_blacklist",
                            &format!("{base}.china_checkin.role_blacklist"),
                            &[
                                "genshin",
                                "honkai2",
                                "honkai3rd",
                                "tears_of_themis",
                                "star_rail",
                                "zenless_zone_zero",
                            ],
                            &mut warnings,
                        );
                    }
                    inspect_named_child(
                        account_map,
                        "hoyolab",
                        &format!("{base}.hoyolab"),
                        &["cookie", "language", "user_agent", "games"],
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
                    if let Some(Value::Mapping(tasks)) = get(account_map, "tasks") {
                        inspect_named_child(
                            tasks,
                            "bbs",
                            &format!("{base}.tasks.bbs"),
                            &[
                                "enabled",
                                "sign",
                                "forums",
                                "read",
                                "like",
                                "cancel_like",
                                "share",
                            ],
                            &mut warnings,
                        );
                        inspect_named_child(
                            tasks,
                            "web_activity",
                            &format!("{base}.tasks.web_activity"),
                            &["enabled", "activities"],
                            &mut warnings,
                        );
                        if get(tasks, "hoyolab_checkin").and_then(Value::as_bool) == Some(true)
                            && get(account_map, "hoyolab").is_none()
                        {
                            warnings.push(format!(
                                "{base}.hoyolab 未配置，HoYoLAB 暂按兼容模式复用 credentials.cookie 和 games；建议补充独立配置"
                            ));
                        }
                    }
                }
            }
        }
        if let Some(Value::Mapping(notifications)) = get(map, "notifications") {
            inspect_mapping(
                &Value::Mapping(notifications.clone()),
                "notifications",
                &["enabled", "error_only", "block_keywords", "providers"],
                &mut warnings,
            );
            if let Some(Value::Sequence(providers)) = get(notifications, "providers") {
                for (index, provider) in providers.iter().enumerate() {
                    let allowed = match provider
                        .as_mapping()
                        .and_then(|m| get(m, "type"))
                        .and_then(Value::as_str)
                    {
                        Some("telegram") => {
                            &["type", "bot_token", "chat_id", "api_url", "proxy"][..]
                        }
                        Some("webhook") => &["type", "url"][..],
                        Some("pushplus") => &["type", "token", "topic"][..],
                        Some("ftqq") => &["type", "sendkey", "api_url"][..],
                        Some("pushme") => &["type", "token", "api_url"][..],
                        Some("cqhttp") => &["type", "url", "qq", "group"][..],
                        Some("wecom") => &[
                            "type", "corp_id", "agent_id", "secret", "to_user", "api_url",
                        ][..],
                        Some("wecomrobot") => &["type", "url", "mobile"][..],
                        Some("pushdeer") => &["type", "token", "api_url"][..],
                        Some("dingrobot") => &["type", "webhook", "secret"][..],
                        Some("feishubot") => &["type", "webhook"][..],
                        Some("bark") => &["type", "token", "api_url", "icon"][..],
                        Some("gotify") => &["type", "token", "api_url", "priority"][..],
                        Some("ifttt") => &["type", "event", "key", "api_url"][..],
                        Some("qmsg") => &["type", "key", "api_url"][..],
                        Some("discord") => &["type", "webhook"][..],
                        Some("wxpusher") => {
                            &["type", "app_token", "uids", "topic_ids", "api_url"][..]
                        }
                        Some("serverchan3") => &["type", "sendkey", "tags"][..],
                        Some("smtp") => &[
                            "type",
                            "host",
                            "port",
                            "from",
                            "to",
                            "username",
                            "password",
                            "subject",
                            "tls",
                            "timeout_seconds",
                        ][..],
                        Some("windows_toast") => &["type", "title_prefix"][..],
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
fn default_hoyolab_language() -> String {
    "zh-cn".to_owned()
}
const fn default_timeout() -> u64 {
    30
}
const fn default_retry_count() -> u32 {
    3
}
const fn default_game_checkin_max_attempts() -> u32 {
    3
}
const fn default_random_delay() -> u64 {
    10
}
const fn default_schedule_interval() -> u64 {
    720
}

fn hydrate_stokens_from_cookies(config: &mut Config) {
    for account in &mut config.accounts {
        if !account.credentials.stoken.is_empty() {
            continue;
        }
        if let Ok(jar) = CookieJar::parse(account.credentials.cookie.expose_secret()) {
            if let Some(stoken) = jar.get("stoken").filter(|value| !value.is_empty()) {
                account.credentials.stoken = SecretString::new(stoken);
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BbsTaskConfig {
    pub enabled: bool,
    pub sign: bool,
    pub forums: Vec<u8>,
    pub read: bool,
    pub like: bool,
    pub cancel_like: bool,
    pub share: bool,
}

impl Default for BbsTaskConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sign: true,
            forums: default_bbs_forums(),
            read: true,
            like: true,
            cancel_like: true,
            share: true,
        }
    }
}

impl BbsTaskConfig {
    pub fn is_enabled(&self) -> bool {
        self.enabled && self.any_action_enabled()
    }

    pub fn any_action_enabled(&self) -> bool {
        self.sign || self.read || self.like || self.share
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BbsTaskConfigInput {
    Bool(bool),
    Detail(BbsTaskConfigDetail),
}

#[derive(Deserialize)]
struct BbsTaskConfigDetail {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default = "default_true")]
    sign: bool,
    #[serde(default = "default_bbs_forums")]
    forums: Vec<u8>,
    #[serde(default = "default_true")]
    read: bool,
    #[serde(default = "default_true")]
    like: bool,
    #[serde(default = "default_true")]
    cancel_like: bool,
    #[serde(default = "default_true")]
    share: bool,
}

impl<'de> Deserialize<'de> for BbsTaskConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match BbsTaskConfigInput::deserialize(deserializer)? {
            BbsTaskConfigInput::Bool(enabled) => Self {
                enabled,
                ..Self::default()
            },
            BbsTaskConfigInput::Detail(detail) => Self {
                enabled: detail.enabled,
                sign: detail.sign,
                forums: detail.forums,
                read: detail.read,
                like: detail.like,
                cancel_like: detail.cancel_like,
                share: detail.share,
            },
        })
    }
}
fn default_bbs_forums() -> Vec<u8> {
    vec![5, 2]
}
fn default_web_activities() -> Vec<WebActivity> {
    vec![WebActivity::GenshinMizone]
}
fn default_device_name() -> String {
    "Xiaomi MI 6".to_owned()
}
fn default_device_model() -> String {
    "Mi 6".to_owned()
}
fn default_china_user_agent() -> String {
    "Mozilla/5.0 (Linux; Android 12) AppleWebKit/537.36 Mobile Safari/537.36 miHoYoBBS/2.109.0"
        .to_owned()
}
fn default_hoyolab_user_agent() -> String {
    "Mozilla/5.0 (Linux; Android 12) AppleWebKit/537.36 Mobile Safari/537.36".to_owned()
}
fn default_telegram_api_url() -> Url {
    Url::parse("https://api.telegram.org").expect("valid Telegram API URL")
}

fn deserialize_telegram_api_url<'de, D>(deserializer: D) -> Result<Url, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Url>::deserialize(deserializer)?.unwrap_or_else(default_telegram_api_url))
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
        Ok(LoadedConfig {
            config,
            warnings,
            source: ConfigSource::Current,
        })
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
        assert_eq!(loaded.config.runtime.game_checkin_max_attempts, 3);
        assert_eq!(loaded.config.runtime.schedule, ScheduleConfig::default());
        assert_eq!(
            loaded.config.accounts[0].credentials.cookie.expose_secret(),
            "account_id=123; cookie_token=secret"
        );
        assert_eq!(loaded.config.accounts[0].device, DeviceConfig::default());
        assert_eq!(
            loaded.config.accounts[0].china_checkin,
            ChinaCheckinConfig::default()
        );
        assert!(loaded.config.accounts[0].hoyolab.is_none());
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn explicit_hoyolab_config_uses_independent_cookie_games_and_headers() {
        let source = MINIMAL
            .replace(
                "    tasks:",
                "    hoyolab:\n      cookie: overseas-secret\n      language: ja-jp\n      user_agent: custom-hoyolab-agent\n      games: [genshin, star_rail]\n    tasks:",
            )
            .replace("      bbs: true", "      bbs: true\n      hoyolab_checkin: true");
        let loaded = parse(&source).unwrap();
        let hoyolab = loaded.config.accounts[0].hoyolab.as_ref().unwrap();
        assert_eq!(hoyolab.cookie.expose_secret(), "overseas-secret");
        assert_eq!(hoyolab.language, "ja-jp");
        assert_eq!(hoyolab.user_agent, "custom-hoyolab-agent");
        assert_eq!(hoyolab.games, vec![Game::Genshin, Game::StarRail]);
        assert!(!format!("{:?}", loaded.config).contains("overseas-secret"));
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn missing_hoyolab_node_uses_compatible_cookie_and_games_with_warning() {
        let source = MINIMAL
            .replace("    tasks:", "    games: [genshin]\n    tasks:")
            .replace(
                "      bbs: true",
                "      bbs: true\n      hoyolab_checkin: true",
            );
        let loaded = parse(&source).unwrap();
        assert!(loaded.config.accounts[0].hoyolab.is_none());
        assert!(
            loaded
                .warnings
                .iter()
                .any(|warning| warning.contains("兼容模式"))
        );
    }

    #[test]
    fn hoyolab_rejects_missing_cookie_empty_games_and_unsupported_game() {
        let source = MINIMAL
            .replace(
                "    tasks:",
                "    hoyolab:\n      cookie: ''\n      language: zh-cn\n      user_agent: agent\n      games: [honkai2]\n    tasks:",
            )
            .replace("      bbs: true", "      bbs: true\n      hoyolab_checkin: true");
        assert!(
            matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.iter().any(|error| error.contains("hoyolab.cookie")) && errors.iter().any(|error| error.contains("不支持 honkai2")))
        );
    }

    #[test]
    fn china_role_blacklist_rejects_blank_and_duplicate_uids() {
        let source = MINIMAL.replace(
            "    tasks:",
            "    china_checkin:\n      role_blacklist:\n        genshin: ['10001', '10001', '   ']\n    tasks:",
        );
        assert!(
            matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.iter().any(|error| error.contains("重复 UID")) && errors.iter().any(|error| error.contains("空 UID")))
        );
    }

    #[test]
    fn default_example_config_is_valid_with_one_cookie() {
        let source = EXAMPLE_CONFIG.replace(
            "${MIHOYO_COOKIE_1}",
            "account_id=123; account_mid_v2=mid; stoken=v2_example",
        );
        let mut config: Config = serde_yaml_ng::from_str(&source).unwrap();
        hydrate_stokens_from_cookies(&mut config);
        validate(&config).unwrap();
    }

    #[test]
    fn cloud_game_config_round_trips_without_exposing_tokens() {
        let source = MINIMAL.replace(
            "    tasks:",
            "    cloud_games:\n      china:\n        genshin:\n          enabled: true\n          token: cn-cloud-secret\n        zenless_zone_zero:\n          enabled: false\n          token: saved-zzz-secret\n      overseas:\n        language: en-us\n        genshin:\n          enabled: true\n          token: os-cloud-secret\n    tasks:",
        );
        let loaded = parse(&source).unwrap();
        let cloud = &loaded.config.accounts[0].cloud_games;
        assert!(cloud.china.genshin.enabled);
        assert!(!cloud.china.zenless_zone_zero.enabled);
        assert_eq!(cloud.overseas.language, "en-us");
        assert_eq!(
            cloud
                .china
                .genshin
                .token
                .as_ref()
                .map(SecretString::expose_secret),
            Some("cn-cloud-secret")
        );
        let debug = format!("{:?}", loaded.config);
        assert!(!debug.contains("cn-cloud-secret"));
        assert!(!debug.contains("os-cloud-secret"));

        let yaml = to_yaml(&loaded.config).unwrap();
        let round_tripped: Config = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(
            round_tripped.accounts[0].cloud_games.overseas.language,
            "en-us"
        );
    }

    #[test]
    fn cloud_game_rejects_missing_token_and_unknown_language() {
        let source = MINIMAL.replace(
            "    tasks:",
            "    cloud_games:\n      china:\n        genshin:\n          enabled: true\n          token: null\n      overseas:\n        language: invalid\n    tasks:",
        );
        assert!(
            matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.len() == 2 && errors.iter().any(|error| error.contains("china.genshin.enabled")) && errors.iter().any(|error| error.contains("overseas.language")))
        );
    }

    #[test]
    fn cloud_game_rejects_whitespace_only_token() {
        let source = MINIMAL.replace(
            "    tasks:",
            "    cloud_games:\n      china:\n        genshin:\n          enabled: true\n          token: '   '\n    tasks:",
        );
        assert!(
            matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.iter().any(|error| error.contains("china.genshin.token")))
        );
    }

    #[test]
    fn cloud_game_unknown_nested_fields_are_reported() {
        let source = MINIMAL.replace(
            "    tasks:",
            "    cloud_games:\n      china:\n        genshin:\n          enabled: false\n          typo: true\n    tasks:",
        );
        let loaded = parse(&source).unwrap();
        assert_eq!(
            loaded.warnings,
            vec!["未知配置字段 accounts[0].cloud_games.china.genshin.typo"]
        );
    }

    #[test]
    fn telegram_applies_default_api_url_and_accepts_proxy() {
        let source = format!(
            "{MINIMAL}\nnotifications:\n  enabled: true\n  providers:\n    - type: telegram\n      bot_token: bot-secret\n      chat_id: '123456'\n      proxy: 127.0.0.1:7890\n"
        );
        let loaded = parse(&source).unwrap();

        let NotificationProvider::Telegram { api_url, proxy, .. } =
            &loaded.config.notifications.providers[0]
        else {
            panic!("expected Telegram provider");
        };
        assert_eq!(api_url.as_str(), "https://api.telegram.org/");
        assert_eq!(
            proxy.as_ref().map(|value| value.expose_secret()),
            Some("127.0.0.1:7890")
        );
        assert!(loaded.warnings.is_empty());
        let debug = format!("{:?}", loaded.config.notifications);
        assert!(!debug.contains("bot-secret"));
        assert!(!debug.contains("127.0.0.1:7890"));
    }

    #[test]
    fn telegram_accepts_legacy_null_api_url() {
        let source = format!(
            "{MINIMAL}\nnotifications:\n  providers:\n    - type: telegram\n      bot_token: bot-secret\n      chat_id: '123456'\n      api_url: null\n"
        );
        let loaded = parse(&source).unwrap();
        let NotificationProvider::Telegram { api_url, .. } =
            &loaded.config.notifications.providers[0]
        else {
            panic!("expected Telegram provider");
        };
        assert_eq!(api_url.as_str(), "https://api.telegram.org/");
    }

    #[test]
    fn telegram_rejects_unsafe_proxy_protocol() {
        let source = format!(
            "{MINIMAL}\nnotifications:\n  enabled: true\n  providers:\n    - type: telegram\n      bot_token: bot-secret\n      chat_id: '123456'\n      proxy: file:///private/proxy-secret\n"
        );
        assert!(
            matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.iter().any(|error| error.contains("notifications.providers[0].proxy")))
        );
    }

    #[test]
    fn smtp_accepts_tls_modes_and_redacts_credentials() {
        for tls in ["none", "starttls", "implicit"] {
            let source = format!(
                "{MINIMAL}\nnotifications:\n  enabled: true\n  providers:\n    - type: smtp\n      host: smtp.example.com\n      port: 465\n      from: sender@example.com\n      to: receiver@example.com\n      username: smtp-user-secret\n      password: smtp-password-secret\n      subject: MihoyoBBSTools RS\n      tls: {tls}\n      timeout_seconds: 30\n"
            );
            let loaded = parse(&source).unwrap();
            assert!(loaded.warnings.is_empty());
            let debug = format!("{:?}", loaded.config.notifications);
            assert!(!debug.contains("smtp-user-secret"));
            assert!(!debug.contains("smtp-password-secret"));
        }
    }

    #[test]
    fn smtp_rejects_invalid_addresses_port_and_timeout() {
        let source = format!(
            "{MINIMAL}\nnotifications:\n  enabled: true\n  providers:\n    - type: smtp\n      host: smtp.example.com\n      port: 0\n      from: invalid\n      to: invalid\n      username: user\n      password: password\n      subject: test\n      tls: implicit\n      timeout_seconds: 301\n"
        );
        assert!(
            matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.iter().any(|error| error.contains(".port")) && errors.iter().any(|error| error.contains(".from")) && errors.iter().any(|error| error.contains(".to")) && errors.iter().any(|error| error.contains(".timeout_seconds")))
        );
    }

    #[test]
    fn smtp_rejects_url_instead_of_host_name() {
        let source = format!(
            "{MINIMAL}\nnotifications:\n  enabled: true\n  providers:\n    - type: smtp\n      host: https://smtp.example.com/mail\n      port: 465\n      from: sender@example.com\n      to: receiver@example.com\n      username: user\n      password: password\n      subject: test\n      tls: implicit\n"
        );
        assert!(
            matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.iter().any(|error| error.contains(".host")))
        );
    }

    #[test]
    fn windows_toast_applies_default_prefix_and_rejects_control_characters() {
        let source = format!(
            "{MINIMAL}\nnotifications:\n  enabled: true\n  providers:\n    - type: windows_toast\n"
        );
        let loaded = parse(&source).unwrap();
        let NotificationProvider::WindowsToast { title_prefix } =
            &loaded.config.notifications.providers[0]
        else {
            panic!("expected Windows toast provider");
        };
        assert_eq!(title_prefix, "MihoyoBBSTools RS");
        assert!(loaded.warnings.is_empty());

        let invalid = format!(
            "{MINIMAL}\nnotifications:\n  enabled: true\n  providers:\n    - type: windows_toast\n      title_prefix: \"invalid\\nvalue\"\n"
        );
        assert!(
            matches!(parse(&invalid), Err(ConfigError::Validation(errors)) if errors.iter().any(|error| error.contains("title_prefix")))
        );
    }

    #[test]
    fn bbs_boolean_and_detailed_switches_are_compatible() {
        let legacy = parse(MINIMAL).unwrap();
        let legacy_bbs = &legacy.config.accounts[0].tasks.bbs;
        assert!(legacy_bbs.enabled && legacy_bbs.sign && legacy_bbs.read);
        assert_eq!(legacy_bbs.forums, vec![5, 2]);

        let detailed = parse(&MINIMAL.replace(
            "      bbs: true",
            "      bbs:\n        enabled: true\n        sign: false\n        forums: [2, 6]\n        read: true\n        like: false\n        cancel_like: false\n        share: false",
        ))
        .unwrap();
        let bbs = &detailed.config.accounts[0].tasks.bbs;
        assert!(bbs.enabled && bbs.read);
        assert_eq!(bbs.forums, vec![2, 6]);
        assert!(!bbs.sign && !bbs.like && !bbs.cancel_like && !bbs.share);
    }

    #[test]
    fn web_activity_boolean_and_detailed_forms_are_compatible() {
        let boolean = parse(&MINIMAL.replace(
            "      bbs: true",
            "      bbs: true\n      web_activity: true",
        ))
        .unwrap();
        assert!(boolean.config.accounts[0].tasks.web_activity.enabled);
        assert_eq!(
            boolean.config.accounts[0].tasks.web_activity.activities,
            vec![WebActivity::GenshinMizone]
        );

        let detailed = parse(&MINIMAL.replace(
            "      bbs: true",
            "      bbs: true\n      web_activity:\n        enabled: true\n        activities: []",
        ))
        .unwrap();
        assert!(detailed.config.accounts[0].tasks.web_activity.enabled);
        assert!(
            detailed.config.accounts[0]
                .tasks
                .web_activity
                .activities
                .is_empty()
        );
    }

    #[test]
    fn web_activity_rejects_unknown_and_duplicate_names() {
        let unknown = MINIMAL.replace(
            "      bbs: true",
            "      bbs: true\n      web_activity:\n        enabled: true\n        activities: [unknown_activity]",
        );
        assert!(matches!(parse(&unknown), Err(ConfigError::Yaml(_))));

        let duplicate = MINIMAL.replace(
            "      bbs: true",
            "      bbs: true\n      web_activity:\n        enabled: true\n        activities: [genshin_mizone, genshin_mizone]",
        );
        assert!(
            matches!(parse(&duplicate), Err(ConfigError::Validation(errors)) if errors.iter().any(|error| error.contains("重复活动")))
        );
    }

    #[test]
    fn bbs_forums_must_be_supported_unique_and_nonempty_when_enabled() {
        for forums in ["[]", "[2, 2]", "[7]"] {
            let source = MINIMAL.replace(
                "      bbs: true",
                &format!(
                    "      bbs:\n        enabled: true\n        sign: true\n        forums: {forums}"
                ),
            );
            assert!(
                matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.iter().any(|error| error.contains("tasks.bbs.forums")))
            );
        }
    }

    #[test]
    fn warns_about_unknown_bbs_fields() {
        let source = MINIMAL.replace(
            "      bbs: true",
            "      bbs:\n        enabled: true\n        typo: true",
        );
        let loaded = parse(&source).unwrap();
        assert_eq!(
            loaded.warnings,
            vec!["未知配置字段 accounts[0].tasks.bbs.typo"]
        );
    }

    #[test]
    fn missing_stoken_is_hydrated_from_cookie() {
        let mut config: Config = serde_yaml_ng::from_str(
            &MINIMAL
                .replace(
                    "${COOKIE}",
                    "account_id=123; stoken=v2_from_cookie; account_mid_v2=mid",
                )
                .replace("      stoken: \"${STOKEN}\"\n", ""),
        )
        .unwrap();
        hydrate_stokens_from_cookies(&mut config);
        assert_eq!(
            config.accounts[0].credentials.stoken.expose_secret(),
            "v2_from_cookie"
        );
    }

    #[test]
    fn device_config_serializes_and_round_trips() {
        let source = MINIMAL.replace(
            "    tasks:",
            "    device:\n      name: Configured Name\n      model: Configured Model\n      id: configured-id\n      fp: configured-fp\n    tasks:",
        );
        let loaded = parse(&source).unwrap();
        let expected = DeviceConfig {
            name: "Configured Name".to_owned(),
            model: "Configured Model".to_owned(),
            id: "configured-id".to_owned(),
            fp: "configured-fp".to_owned(),
        };
        assert_eq!(loaded.config.accounts[0].device, expected);
        assert!(loaded.warnings.is_empty());

        let yaml = to_yaml(&loaded.config).unwrap();
        let name_position = yaml.find("name: Configured Name").unwrap();
        let model_position = yaml.find("model: Configured Model").unwrap();
        let id_position = yaml.find("id: configured-id").unwrap();
        let fp_position = yaml.find("fp: configured-fp").unwrap();
        assert!(name_position < model_position);
        assert!(model_position < id_position);
        assert!(id_position < fp_position);
        let round_tripped: Config = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(round_tripped.accounts[0].device, expected);
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
            .replace(
                "    unexpected: true",
                "    unexpected: true\n    device:\n      typo: true",
            )
            .replace("      bbs: true", "      bbs: true\n      typo: true");
        let loaded = parse(&source).unwrap();
        assert_eq!(
            loaded.warnings,
            vec![
                "未知配置字段 accounts[0].unexpected",
                "未知配置字段 accounts[0].device.typo",
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
    fn rejects_game_checkin_attempts_outside_safe_range() {
        for max_attempts in [0, 11] {
            let source = MINIMAL.replace(
                "accounts:",
                &format!("runtime:\n  game_checkin_max_attempts: {max_attempts}\naccounts:"),
            );
            assert!(
                matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.iter().any(|error| error.contains("runtime.game_checkin_max_attempts")))
            );
        }
    }

    #[test]
    fn rejects_schedule_interval_outside_safe_range() {
        for interval in [0, 10_081] {
            let source = MINIMAL.replace(
                "accounts:",
                &format!("runtime:\n  schedule:\n    interval_minutes: {interval}\naccounts:"),
            );
            assert!(
                matches!(parse(&source), Err(ConfigError::Validation(errors)) if errors.iter().any(|error| error.contains("runtime.schedule.interval_minutes")))
            );
        }
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
    fn only_current_config_source_supports_persistent_refresh() {
        assert!(ConfigSource::Current.supports_persistent_refresh());
        assert!(!ConfigSource::PythonLegacy(15).supports_persistent_refresh());
        assert!(!ConfigSource::Dacapo.supports_persistent_refresh());
        assert!(!ConfigSource::StandardInput.supports_persistent_refresh());
    }

    #[test]
    fn string_loader_marks_current_yaml_as_standard_input() {
        let source = MINIMAL
            .replace("${COOKIE}", "account_id=123; cookie_token=secret")
            .replace("${STOKEN}", "v2_secret");
        let loaded = load_from_str(&source, "serverless").unwrap();
        assert_eq!(loaded.source, ConfigSource::StandardInput);
        assert_eq!(loaded.config.accounts[0].name, "first");
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
        assert_eq!(migrated.source, ConfigSource::PythonLegacy(15));
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
            assert_eq!(
                std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o600
            );
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
