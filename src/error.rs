use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("配置错误：{0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("网络错误：{0}")]
    Http(#[from] crate::http::HttpError),
    #[error("认证错误：{0}")]
    Auth(#[from] crate::auth::AuthError),
    #[error("任务需要验证码，已停止高风险重试")]
    CaptchaRequired,
    #[error("任务执行失败：{0}")]
    Task(String),
    #[error("多配置运行错误：{0}")]
    ConfigDirectory(#[from] crate::service::ConfigDirectoryError),
    #[error("青龙环境配置错误：{0}")]
    Qinglong(#[from] crate::service::QinglongError),
    #[error("DaCapo 配置错误：{0}")]
    Dacapo(#[from] crate::config::DacapoError),
    #[error("无法读取标准输入配置")]
    StandardInput,
}

impl AppError {
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Config(_) => 2,
            Self::Auth(_) => 3,
            Self::CaptchaRequired => 4,
            Self::Http(_) => 5,
            Self::Task(_) => 1,
            Self::ConfigDirectory(_) => 2,
            Self::Qinglong(_) => 2,
            Self::Dacapo(_) => 2,
            Self::StandardInput => 2,
        }
    }
}
