use std::process::ExitCode;

use clap::Parser;
use mihoyo_bbs_tools::{
    cli::{CheckinRegion, Cli, Command, ConfigCommand, RunTask},
    config,
    error::AppError,
    push::{self, DeliveryStatus},
    service,
};
use tracing_subscriber::filter::LevelFilter;

mod file_logging;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("安装 ring TLS provider");
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
            let mut loaded = config::load(&path)?;
            for warning in &loaded.warnings {
                tracing::warn!("{warning}");
            }
            let mut report = service::RunReport::default();
            if matches!(region, CheckinRegion::China | CheckinRegion::All) {
                report.extend(
                    service::run_china_checkin_with_refresh(&mut loaded.config, &path).await,
                );
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
            let mut loaded = config::load(&path)?;
            for warning in &loaded.warnings {
                tracing::warn!("{warning}");
            }
            let all = tasks.is_empty();
            let mut report = service::RunReport::default();
            if all || tasks.contains(&RunTask::ChinaCheckin) {
                report.extend(
                    service::run_china_checkin_with_refresh(&mut loaded.config, &path).await,
                );
            }
            if all || tasks.contains(&RunTask::HoyolabCheckin) {
                report.extend(service::run_hoyolab_checkin(&loaded.config).await);
            }
            if all || tasks.contains(&RunTask::Bbs) {
                report.extend(service::run_bbs_with_refresh(&mut loaded.config, &path).await);
            }
            let china_cloud = all || tasks.contains(&RunTask::ChinaCloudGame);
            let overseas_cloud = all || tasks.contains(&RunTask::OverseasCloudGame);
            if china_cloud || overseas_cloud {
                report.extend(
                    service::run_cloud_games(&loaded.config, china_cloud, overseas_cloud).await,
                );
            }
            if all || tasks.contains(&RunTask::WebActivity) {
                report.extend(service::run_web_activities(&loaded.config));
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
        Command::CreateLauncher { output, force } => {
            let path =
                mihoyo_bbs_tools::launcher::create_windows_launcher(output.as_deref(), force)
                    .map_err(|error| AppError::Task(error.to_string()))?;
            println!("已创建异步启动 BAT：{}", path.display());
            println!("该 BAT 可移动到其他位置，仍会从当前程序目录运行 MihoyoBBSToolsRS。");
        }
        Command::Config { command } => match command {
            ConfigCommand::Setup { config: path } => config::interactive_setup(&path).await?,
            ConfigCommand::Edit { config: path } => {
                config::edit_file(&path)?;
                println!("配置已更新：{}", path.display());
            }
            ConfigCommand::AddAccount { config: path, name } => {
                let added = config::add_account_from_stdin(&path, name.as_deref()).await?;
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
    let level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|value| value.parse::<LevelFilter>().ok())
        .unwrap_or_else(|| match runtime.map(|value| value.log_level) {
            Some(config::LogLevel::Trace) => LevelFilter::TRACE,
            Some(config::LogLevel::Debug) => LevelFilter::DEBUG,
            Some(config::LogLevel::Warn) => LevelFilter::WARN,
            Some(config::LogLevel::Error) => LevelFilter::ERROR,
            Some(config::LogLevel::Info) | None => LevelFilter::INFO,
        });
    if let Some(logging) = runtime.map(|r| &r.logging).filter(|v| v.enabled) {
        if let Ok(appender) =
            file_logging::DailyFileAppender::new(&logging.directory, &logging.file_prefix)
        {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            tracing_subscriber::fmt()
                .with_max_level(level)
                .with_target(false)
                .with_ansi(false)
                .with_writer(writer)
                .init();
            return Some(guard);
        }
    }
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .without_time()
        .init();
    None
}
