use std::process::ExitCode;

use clap::Parser;
use mihoyo_bbs_tools::{
    cli::{Cli, Command, ConfigCommand},
    config,
    error::AppError,
    push::{self, DeliveryStatus},
    service,
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    match run(Cli::parse()).await {
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
        Command::Checkin { config: path } => {
            let loaded = config::load(&path)?;
            for warning in &loaded.warnings {
                tracing::warn!("{warning}");
            }
            let mut report = service::run_china_checkin(&loaded.config).await;
            report.extend(service::run_hoyolab_checkin(&loaded.config).await);
            return Ok(finish_report(&loaded.config, &report).await);
        }
        Command::Run { config: path } => {
            let loaded = config::load(&path)?;
            for warning in &loaded.warnings {
                tracing::warn!("{warning}");
            }
            let mut report = service::run_china_checkin(&loaded.config).await;
            report.extend(service::run_hoyolab_checkin(&loaded.config).await);
            report.extend(service::run_bbs(&loaded.config).await);
            return Ok(finish_report(&loaded.config, &report).await);
        }
        Command::MigrateConfig { input, output } => {
            let loaded = config::write_migrated_config(&input, &output)?;
            for warning in &loaded.warnings {
                tracing::warn!("{warning}");
            }
            println!("配置已迁移到 {}", output.display());
        }
        Command::PrintExampleConfig => print!("{}", config::EXAMPLE_CONFIG),
        Command::Config { command } => match command {
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

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();
}
