use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "MihoyoBBSToolsRS",
    version = crate::VERSION,
    about = "米游社与 HoYoLAB 自动任务工具",
    after_help = "示例：\n  MihoyoBBSToolsRS validate-config\n  MihoyoBBSToolsRS checkin --region china\n  MihoyoBBSToolsRS run --task china-checkin,bbs\n  MihoyoBBSToolsRS run-directory config --prefix mhy_\n  MihoyoBBSToolsRS config setup\n  MihoyoBBSToolsRS create-launcher\n\n使用 `MihoyoBBSToolsRS <COMMAND> --help` 查看子命令的详细说明。"
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
    /// 检查配置、目录和平台；加 --online 后执行只读网络与凭据诊断
    Doctor {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 发起只读网络请求；不会执行签到、社区操作、领取或通知发送
        #[arg(long)]
        online: bool,
        /// 诊断报告输出格式
        #[arg(long, value_enum, default_value_t = ReportFormat::Text)]
        output: ReportFormat,
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
    /// 执行游戏签到、米游社社区任务、云游戏签到与 Web 活动状态处理
    Run {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 仅执行指定任务；可重复提供或使用逗号分隔，省略时执行全部任务
        #[arg(long = "task", value_enum, value_delimiter = ',')]
        tasks: Vec<RunTask>,
        /// 禁止将自动刷新的凭据写回配置文件；从标准输入读取时始终只读
        #[arg(long)]
        read_only: bool,
        /// 禁止发送所有外部通知
        #[arg(long)]
        no_notify: bool,
        /// 任务报告输出格式；JSON 模式只向标准输出写入一个 JSON 对象
        #[arg(long, value_enum, default_value_t = ReportFormat::Text)]
        output: ReportFormat,
        /// 展开全部成功、已完成和跳过记录；不改变 JSON 输出
        #[arg(long)]
        verbose: bool,
    },
    /// 依次执行目录中的多个 YAML 配置，单个文件失败不阻止后续文件
    #[command(alias = "run-multi")]
    RunDirectory(DirectoryRunArgs),
    /// 按青龙环境变量选择单配置或多配置模式并执行任务
    #[command(alias = "ql")]
    Qinglong(QinglongArgs),
    /// 读取 DaCapo 生成的 JSON 配置并以内存只读模式执行任务
    Dacapo {
        /// DaCapo 传入的 JSON 配置文件
        #[arg(value_name = "JSON_PATH")]
        config: PathBuf,
        /// 仅执行指定任务；语义与 run --task 相同
        #[arg(long = "task", value_enum, value_delimiter = ',')]
        tasks: Vec<RunTask>,
    },
    /// 按 runtime.schedule 间隔常驻执行完整任务，每轮重新加载配置
    Schedule {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 每轮仅执行指定任务；语义与 run --task 相同
        #[arg(long = "task", value_enum, value_delimiter = ',')]
        tasks: Vec<RunTask>,
    },
    /// 将 Python v11-v15 配置迁移到新版 YAML
    MigrateConfig(MigrationArgs),
    /// 将完整的脱敏默认配置模板输出到标准输出
    PrintExampleConfig,
    /// 在 Windows 创建可移动的异步启动 BAT
    CreateLauncher {
        /// BAT 输出路径；默认生成在当前 EXE 所在目录
        #[arg(short, long, value_name = "BAT_PATH")]
        output: Option<PathBuf>,
        /// 覆盖已经存在的 BAT
        #[arg(long)]
        force: bool,
    },
    /// 交互设置、编辑、添加或删除配置账号
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// 查看通知渠道或发送不含账号信息的测试通知
    Notification {
        #[command(subcommand)]
        command: NotificationCommand,
    },
    /// 安装、查看、立即运行或移除系统自动运行任务
    Automation {
        #[command(subcommand)]
        command: AutomationCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// 首次创建完整配置；已有配置不会被覆盖
    Init {
        /// 要创建的配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
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
        /// 要删除的账号名称，格式通常为 mys用户:<米游社昵称>
        #[arg(value_name = "ACCOUNT_NAME")]
        name: String,
    },
    /// 立即创建一份受控配置备份
    Backup {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 保留最近备份数量，范围 1 到 50
        #[arg(long, default_value_t = 5)]
        keep: usize,
    },
    /// 列出备份时间、版本与账号数量，不显示任何凭据
    ListBackups {
        /// 配置文件路径，用于确定对应备份目录
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    /// 从当前配置 backups 目录恢复一份已校验备份
    Restore {
        /// 要恢复的备份文件
        #[arg(value_name = "BACKUP")]
        backup: PathBuf,
        /// 要替换的配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 跳过交互确认；自动化环境必须显式提供
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum NotificationCommand {
    /// 列出全部通知渠道及脱敏接收目标
    List {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    /// 向全部或指定渠道发送测试通知
    Test {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 仅测试指定的 1-based 渠道编号
        #[arg(long, value_name = "NUMBER")]
        provider: Option<usize>,
    },
}

#[derive(Debug, Subcommand)]
pub enum AutomationCommand {
    /// 在 Windows 任务计划程序中安装或更新每日任务
    Install {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        /// 每日本地时间，使用 HH:MM 24 小时格式
        #[arg(long, default_value = "09:00")]
        time: String,
        /// 使用 S4U 在用户未登录时运行；本地桌面通知届时不可见
        #[arg(long)]
        run_whether_logged_on: bool,
        /// 整个任务失败后的五分钟重试次数
        #[arg(long, default_value_t = 3)]
        retry_count: u8,
        /// 在配置中启用 Windows 本地通知渠道
        #[arg(long)]
        enable_windows_notification: bool,
    },
    /// 查看任务状态、运行记录和路径有效性
    Status {
        /// 任务使用的配置文件路径
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    /// 立即触发已安装的任务
    RunNow,
    /// 仅移除本项目固定名称的任务，保留配置和日志
    Uninstall,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ReportFormat {
    /// 输出面向终端阅读的文本报告
    #[default]
    Text,
    /// 输出稳定的结构化 JSON 报告
    Json,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, ValueEnum)]
pub enum RunTask {
    /// 国内游戏签到
    ChinaCheckin,
    /// HoYoLAB 游戏签到
    HoyolabCheckin,
    /// 米游社社区签到与已启用的社区任务
    Bbs,
    /// 国内云原神与云绝区零签到
    ChinaCloudGame,
    /// 国际服云原神签到
    OverseasCloudGame,
    /// 已配置的 Web 活动；过期活动只生成跳过报告
    WebActivity,
}

#[derive(Debug, Args)]
pub struct DirectoryRunArgs {
    /// 配置目录路径
    #[arg(value_name = "DIRECTORY", default_value = "config")]
    pub directory: PathBuf,
    /// 只运行文件名以此前缀开头的 YAML；默认执行除 *.example.yaml 外的全部 YAML
    #[arg(long, value_name = "PREFIX")]
    pub prefix: Option<String>,
    /// 每个配置仅执行指定任务；语义与 run --task 相同
    #[arg(long = "task", value_enum, value_delimiter = ',')]
    pub tasks: Vec<RunTask>,
    /// 配置文件之间随机等待的最小秒数
    #[arg(long, default_value_t = 3)]
    pub delay_min_seconds: u64,
    /// 配置文件之间随机等待的最大秒数
    #[arg(long, default_value_t = 10)]
    pub delay_max_seconds: u64,
}

#[derive(Debug, Args)]
pub struct QinglongArgs {
    /// 仅执行指定任务；语义与 run --task 相同
    #[arg(long = "task", value_enum, value_delimiter = ',')]
    pub tasks: Vec<RunTask>,
    /// 多配置文件之间随机等待的最小秒数
    #[arg(long, default_value_t = 3)]
    pub delay_min_seconds: u64,
    /// 多配置文件之间随机等待的最大秒数
    #[arg(long, default_value_t = 10)]
    pub delay_max_seconds: u64,
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
        "MihoyoBBSToolsRS {} (commit {}, target {})",
        crate::VERSION,
        option_env!("GIT_COMMIT").unwrap_or("unknown"),
        format_args!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::{CommandFactory, Parser};

    use super::{Cli, Command, ReportFormat};

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
        assert!(help.contains("执行游戏签到、米游社社区任务、云游戏签到与 Web 活动状态处理"));
        assert!(help.contains("交互设置、编辑、添加或删除配置账号"));
        assert!(help.contains("创建可移动的异步启动 BAT"));
    }

    #[test]
    fn command_name_and_version_match_release_identity() {
        let command = Cli::command();
        assert_eq!(command.get_name(), "MihoyoBBSToolsRS");
        assert_eq!(command.get_version(), Some(crate::VERSION));
    }

    #[test]
    fn run_directory_parses_prefix_tasks_and_delay_range() {
        let cli = Cli::try_parse_from([
            "MihoyoBBSToolsRS",
            "run-directory",
            "configs",
            "--prefix",
            "mhy_",
            "--task",
            "china-checkin,bbs",
            "--delay-min-seconds",
            "0",
            "--delay-max-seconds",
            "1",
        ])
        .unwrap();
        let Command::RunDirectory(args) = cli.command else {
            panic!("expected run-directory command");
        };
        assert_eq!(args.directory, PathBuf::from("configs"));
        assert_eq!(args.prefix.as_deref(), Some("mhy_"));
        assert_eq!(args.tasks.len(), 2);
        assert_eq!((args.delay_min_seconds, args.delay_max_seconds), (0, 1));
    }

    #[test]
    fn qinglong_alias_accepts_task_filter() {
        let cli =
            Cli::try_parse_from(["MihoyoBBSToolsRS", "ql", "--task", "china-checkin,bbs"]).unwrap();
        let Command::Qinglong(args) = cli.command else {
            panic!("expected qinglong command");
        };
        assert_eq!(args.tasks.len(), 2);
    }

    #[test]
    fn dacapo_requires_json_path_and_accepts_task_filter() {
        let cli = Cli::try_parse_from([
            "MihoyoBBSToolsRS",
            "dacapo",
            "settings.json",
            "--task",
            "china-checkin",
        ])
        .unwrap();
        let Command::Dacapo { config, tasks } = cli.command else {
            panic!("expected dacapo command");
        };
        assert_eq!(config, PathBuf::from("settings.json"));
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn run_accepts_standard_input_read_only_json_without_notifications() {
        let cli = Cli::try_parse_from([
            "MihoyoBBSToolsRS",
            "run",
            "--config",
            "-",
            "--read-only",
            "--no-notify",
            "--output",
            "json",
        ])
        .unwrap();
        let Command::Run {
            config,
            read_only,
            no_notify,
            output,
            ..
        } = cli.command
        else {
            panic!("expected run command");
        };
        assert_eq!(config, PathBuf::from("-"));
        assert!(read_only);
        assert!(no_notify);
        assert_eq!(output, ReportFormat::Json);
    }

    #[test]
    fn run_accepts_verbose_text_output() {
        let cli = Cli::try_parse_from(["MihoyoBBSToolsRS", "run", "--verbose"]).unwrap();
        let Command::Run {
            verbose, output, ..
        } = cli.command
        else {
            panic!("expected run command");
        };
        assert!(verbose);
        assert_eq!(output, ReportFormat::Text);
    }
}
