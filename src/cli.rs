use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

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
    Version,
    ValidateConfig {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    Checkin {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        #[arg(long, value_enum, default_value_t = CheckinRegion::All)]
        region: CheckinRegion,
    },
    Run {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        #[arg(long = "task", value_enum, value_delimiter = ',')]
        tasks: Vec<RunTask>,
    },
    MigrateConfig(MigrationArgs),
    PrintExampleConfig,
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    Setup {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    Edit {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
    },
    AddAccount {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        #[arg(short, long, alias = "remark")]
        name: Option<String>,
    },
    RemoveAccount {
        #[arg(short, long, default_value = "config/config.yaml")]
        config: PathBuf,
        name: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum CheckinRegion {
    China,
    Hoyolab,
    All,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, ValueEnum)]
pub enum RunTask {
    ChinaCheckin,
    HoyolabCheckin,
    Bbs,
}

#[derive(Debug, Args)]
pub struct MigrationArgs {
    #[arg(value_name = "SOURCE", conflicts_with = "input")]
    source: Option<PathBuf>,
    #[arg(value_name = "TARGET", requires = "source", conflicts_with = "output")]
    target: Option<PathBuf>,
    #[arg(short, long, value_name = "SOURCE", conflicts_with_all = ["source", "target"])]
    input: Option<PathBuf>,
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
