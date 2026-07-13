use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "mihoyo-bbs-tools",
    version,
    about = "米游社与 HoYoLAB 自动任务工具",
    after_help = "示例：\n  mihoyo-bbs-tools validate-config\n  mihoyo-bbs-tools checkin --region china\n  mihoyo-bbs-tools run --task china-checkin,bbs\n  mihoyo-bbs-tools config setup\n\n使用 `mihoyo-bbs-tools <COMMAND> --help` 查看子命令的详细说明。"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 输出版本、Git 提交与目标平台信息
    Version,
    /// 校验配置文件，不访问远程接口
    ValidateConfig {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    /// 执行国内游戏和/或 HoYoLAB 游戏签到
    Checkin {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 本次执行的签到区域
        #[arg(long, value_enum, default_value_t = CheckinRegion::All)]
        region: CheckinRegion,
    },
    /// 执行游戏签到与米游社社区任务
    Run {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 仅执行指定任务；可重复提供或使用逗号分隔，省略时执行全部任务
        #[arg(long = "task", value_enum, value_delimiter = ',')]
        tasks: Vec<RunTask>,
    },
    /// 将 Python v11-v15 配置迁移到新版 YAML
    MigrateConfig(MigrationArgs),
    /// 将完整的脱敏默认配置模板输出到标准输出
    PrintExampleConfig,
    /// 交互设置、编辑、添加或删除配置账号
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// 通过数字菜单设置运行参数、验证码、账号、任务和通知
    Setup {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    /// 使用系统编辑器修改完整 YAML，保存前自动校验
    Edit {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    /// 从标准输入安全读取 Cookie，查询米游社昵称并添加账号
    AddAccount {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 可选账号备注，用于区分账号
        #[arg(short, long, alias = "remark", value_name = "REMARK")]
        name: Option<String>,
    },
    /// 按配置中的账号名称删除账号
    RemoveAccount {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 要删除的账号名称，通常为米游社昵称
        #[arg(value_name = "ACCOUNT_NAME")]
        name: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum CheckinRegion {
    /// 仅执行国内游戏签到
    China,
    /// 仅执行 HoYoLAB 游戏签到
    Hoyolab,
    /// 执行国内和 HoYoLAB 游戏签到
    All,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, ValueEnum)]
pub enum RunTask {
    /// 国内游戏签到
    ChinaCheckin,
    /// HoYoLAB 游戏签到
    HoyolabCheckin,
    /// 米游社社区签到与已启用的社区任务
    Bbs,
}

#[derive(Debug, Args)]
pub struct MigrationArgs {
    /// 迁移源配置文件
    #[arg(value_name = "SOURCE", conflicts_with = "input")]
    source: Option<PathBuf>,
    /// 迁移输出文件；省略时在源文件同目录生成 .migrated 文件
    #[arg(value_name = "TARGET", requires = "source", conflicts_with = "output")]
    target: Option<PathBuf>,
    /// 迁移源配置文件，与位置参数 SOURCE 二选一
    #[arg(short, long, value_name = "SOURCE", conflicts_with_all = ["source", "target"])]
    input: Option<PathBuf>,
    /// 迁移输出文件；需要同时使用 --input
    #[arg(short, long, value_name = "TARGET", requires = "input", conflicts_with_all = ["source", "target"])]
    output: Option<PathBuf>,
}

pub struct ResolvedMigrationArgs {
    pub input: PathBuf,
    pub output: PathBuf,
}

impl MigrationArgs {
    pub fn resolve(self) -> Result<ResolvedMigrationArgs, String> {
        let input = self
            .source
            .or(self.input)
            .ok_or_else(|| "必须提供迁移源配置".to_owned())?;
        let output = self
            .target
            .or(self.output)
            .unwrap_or_else(|| default_migration_output(&input));
        Ok(ResolvedMigrationArgs { input, output })
    }
}

fn default_migration_output(input: &Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new(""));
    let stem = input
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("config");
    let ext = input
        .extension()
        .and_then(|v| v.to_str())
        .filter(|v| matches!(*v, "yaml" | "yml"))
        .unwrap_or("yaml");
    parent.join(format!("{stem}.migrated.{ext}"))
}

pub fn version_text() -> String {
    format!(
        "mihoyo-bbs-tools {} (commit {}, target {})",
        crate::VERSION,
        option_env!("GIT_COMMIT").unwrap_or("unknown"),
        format_args!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
    )
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn all_commands_have_help_descriptions() {
        let command = Cli::command();
        for subcommand in command.get_subcommands() {
            assert!(
                subcommand.get_about().is_some(),
                "子命令 {} 缺少帮助说明",
                subcommand.get_name()
            );
        }

        let config = command
            .find_subcommand("config")
            .expect("config 子命令应存在");
        for subcommand in config.get_subcommands() {
            assert!(
                subcommand.get_about().is_some(),
                "config 子命令 {} 缺少帮助说明",
                subcommand.get_name()
            );
        }
    }

    #[test]
    fn top_level_help_contains_command_descriptions() {
        let help = Cli::command().render_help().to_string();
        assert!(help.contains("校验配置文件，不访问远程接口"));
        assert!(help.contains("执行游戏签到与米游社社区任务"));
        assert!(help.contains("交互设置、编辑、添加或删除配置账号"));
    }
}
