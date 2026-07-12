use std::process::ExitCode;

use clap::Parser;
use mihoyo_bbs_tools::{
    cli::{Cli, Command},
    config,
    error::AppError,
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
            print!("{}", report.render_text());
            return Ok(report.exit_code());
        }
        Command::MigrateConfig { input, output } => {
            let loaded = config::write_migrated_config(&input, &output)?;
            for warning in &loaded.warnings {
                tracing::warn!("{warning}");
            }
            println!("配置已迁移到 {}", output.display());
        }
        Command::PrintExampleConfig => print!("{}", config::EXAMPLE_CONFIG),
    }
    Ok(0)
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();
}
