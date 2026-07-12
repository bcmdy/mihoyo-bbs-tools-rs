use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "mihoyo-bbs-tools",
    version,
    about = "米游社与 HoYoLAB 自动任务工具"
)]
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
    /// 执行国内游戏签到
    Checkin {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    /// 执行游戏签到与米游社任务
    Run {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    /// 将 Python v11-v15 配置迁移到新版 YAML
    MigrateConfig {
        #[arg(short, long)]
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// 输出脱敏的新版配置示例
    PrintExampleConfig,
    /// 编辑、添加或删除配置账号
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// 使用系统编辑器修改完整 YAML，保存前自动校验
    Edit {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    /// 从标准输入安全读取 Cookie 并添加账号
    AddAccount {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 可选备注；省略时使用 Cookie 中的 UID
        #[arg(short, long)]
        name: Option<String>,
    },
    /// 按备注/账号名称删除账号
    RemoveAccount {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        name: String,
    },
}

pub fn version_text() -> String {
    format!(
        "mihoyo-bbs-tools {} (commit {}, target {})",
        crate::VERSION,
        option_env!("GIT_COMMIT").unwrap_or("unknown"),
        format_args!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
    )
}
