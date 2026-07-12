use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "mihoyo-bbs-tools", version, about = "米游社与 HoYoLAB 自动任务工具")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 输出版本、提交与目标平台信息
    Version,
    /// 校验配置，不访问远程接口
    ValidateConfig {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    /// 输出脱敏的新版配置示例
    PrintExampleConfig,
}

pub fn version_text() -> String {
    format!(
        "mihoyo-bbs-tools {} (commit {}, target {})",
        crate::VERSION,
        option_env!("GIT_COMMIT").unwrap_or("unknown"),
        format_args!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
    )
}
