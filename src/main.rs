use std::process::ExitCode;

use clap::Parser;
use mihoyo_bbs_tools::{
    cli::{CheckinRegion, Cli, Command, ConfigCommand, RunTask},
    config,
    error::AppError,
    push::{self, DeliveryStatus},
    service,
};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime =
        cli_config_path(&cli).and_then(|path| config::load(path).ok().map(|v| v.config.runtime));
    let _log_guard = init_tracing(runtime.as_ref());
    match run(cli).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            tracing::error!(%error);
            ExitCode::from(error.exit_code())
        }
    }
}

async fn run(cli: Cli) -> Result<u8, AppError> {
    match cli.command {
        Command::Version => println!("{}", mihoyo_bbs_tools::cli::version_text()),
        Command::ValidateConfig { config: path } => {
            let loaded = config::load(&path)?;
            for warning in &loaded.warnings {
                tracing::warn!("{warning}");
            }
            println!("配置有效：{} 个账号", loaded.config.accounts.len());
        }
        Command::Checkin {
            config: path,
            region,
        } => {
            let loaded = config::load(&path)?;
            for warning in &loaded.warnings {
                tracing::warn!("{warning}");
            }
            let mut report = service::RunReport::default();
            if matches!(region, CheckinRegion::China | CheckinRegion::All) {
                report.extend(service::run_china_checkin(&loaded.config).await);
            }
            if matches!(region, CheckinRegion::Hoyolab | CheckinRegion::All) {
                report.extend(service::run_hoyolab_checkin(&loaded.config).await);
            }
            return Ok(finish_report(&loaded.config, &report).await);
        }
        Command::Run {
            config: path,
            tasks,
        } => {
            let loaded = config::load(&path)?;
            for warning in &loaded.warnings {
                tracing::warn!("{warning}");
            }
            let all = tasks.is_empty();
            let mut report = service::RunReport::default();
            if all || tasks.contains(&RunTask::ChinaCheckin) {
                report.extend(service::run_china_checkin(&loaded.config).await);
            }
            if all || tasks.contains(&RunTask::HoyolabCheckin) {
                report.extend(service::run_hoyolab_checkin(&loaded.config).await);
            }
            if all || tasks.contains(&RunTask::Bbs) {
                report.extend(service::run_bbs(&loaded.config).await);
            }
            return Ok(finish_report(&loaded.config, &report).await);
        }
        Command::MigrateConfig(args) => {
            let resolved = args.resolve().map_err(AppError::Task)?;
            let input = resolved.input;
            let output = resolved.output;
            let loaded = config::write_migrated_config(&input, &output)?;
            for warning in &loaded.warnings {
                tracing::warn!("{warning}");
            }
            println!("配置已迁移到 {}", output.display());
        }
        Command::PrintExampleConfig => print!("{}", config::EXAMPLE_CONFIG),
        Command::Config { command } => match command {
            ConfigCommand::Setup { config: path } => config::interactive_setup(&path)?,
            ConfigCommand::Edit { config: path } => {
                config::edit_file(&path)?;
                println!("配置已更新：{}", path.display());
            }
            ConfigCommand::AddAccount { config: path, name } => {
                let added = config::add_account_from_stdin(&path, name.as_deref())?;
                println!("已添加账号：{added}");
            }
            ConfigCommand::RemoveAccount { config: path, name } => {
                config::remove_account(&path, &name)?;
                println!("已删除账号：{name}");
            }
        },
    }
    Ok(0)
}

async fn finish_report(config: &config::Config, report: &service::RunReport) -> u8 {
    print!("{}", report.render_text());
    tracing::info!("任务报告\n{}", report.render_text().trim_end());
    let push_report = push::send_report(config, report).await;
    for delivery in push_report.deliveries {
        match delivery.status {
            DeliveryStatus::Sent => tracing::info!(
                provider = delivery.provider.as_str(),
                "{}",
                delivery.message
            ),
            DeliveryStatus::Failed => tracing::warn!(
                provider = delivery.provider.as_str(),
                "{}",
                delivery.message
            ),
        }
    }
    report.exit_code()
}

fn cli_config_path(cli: &Cli) -> Option<&std::path::Path> {
    match &cli.command {
        Command::ValidateConfig { config }
        | Command::Checkin { config, .. }
        | Command::Run { config, .. } => Some(config),
        Command::Config { command } => match command {
            ConfigCommand::Setup { config }
            | ConfigCommand::Edit { config }
            | ConfigCommand::AddAccount { config, .. }
            | ConfigCommand::RemoveAccount { config, .. } => Some(config),
        },
        _ => None,
    }
}
fn init_tracing(
    runtime: Option<&config::RuntimeConfig>,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let level = runtime
        .map(|r| format!("{:?}", r.log_level).to_lowercase())
        .unwrap_or_else(|| "info".into());
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    let console = tracing_subscriber::fmt::layer()
        .with_target(false)
        .without_time();
    if let Some(logging) = runtime.map(|r| &r.logging).filter(|v| v.enabled) {
        if std::fs::create_dir_all(&logging.directory).is_ok() {
            let appender =
                tracing_appender::rolling::daily(&logging.directory, &logging.file_prefix);
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let file = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(writer);
            tracing_subscriber::registry()
                .with(filter)
                .with(console)
                .with(file)
                .init();
            return Some(guard);
        }
    }
    tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .init();
    None
}
