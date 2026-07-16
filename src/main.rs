use std::{io::Read, process::ExitCode};

use clap::Parser;
use mihoyo_bbs_tools::{
    cli::{
        CheckinRegion, Cli, Command, ConfigCommand, DirectoryRunArgs, QinglongArgs, ReportFormat,
        RunTask,
    },
    config,
    error::AppError,
    push::{self, DeliveryStatus},
    service,
};
use rand::Rng;
use serde::Serialize;
use tracing_subscriber::filter::LevelFilter;

mod file_logging;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("安装 ring TLS provider");
    let cli = Cli::parse();
    let runtime = initial_runtime(&cli);
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
        Command::Doctor {
            config: path,
            online,
            output,
        } => {
            let report = mihoyo_bbs_tools::doctor::run(&path, online).await;
            let exit_code = report.exit_code();
            match output {
                ReportFormat::Text => print!("{}", report.render_text()),
                ReportFormat::Json => println!(
                    "{}",
                    serde_json::to_string(&report).map_err(|error| {
                        AppError::Task(format!("JSON 诊断报告序列化失败：{error}"))
                    })?
                ),
            }
            return Ok(exit_code);
        }
        Command::Checkin {
            config: path,
            region,
        } => {
            let mut loaded = config::load(&path)?;
            for warning in &loaded.warnings {
                tracing::warn!("{warning}");
            }
            service::apply_runtime_delay(&loaded.config.runtime).await;
            let mut report = service::RunReport::default();
            if matches!(region, CheckinRegion::China | CheckinRegion::All) {
                let persistence = credential_persistence(loaded.source, &path);
                report.extend(
                    service::run_china_checkin_with_persistence(&mut loaded.config, persistence)
                        .await,
                );
            }
            if matches!(region, CheckinRegion::Hoyolab | CheckinRegion::All) {
                report.extend(service::run_hoyolab_checkin(&loaded.config).await);
            }
            return finish_report(&loaded.config, &report, ReportFormat::Text, false, false).await;
        }
        Command::Run {
            config: path,
            tasks,
            read_only,
            no_notify,
            output,
            verbose,
        } => {
            return execute_run_command(&path, &tasks, read_only, no_notify, output, verbose).await;
        }
        Command::RunDirectory(args) => return execute_directory(args).await,
        Command::Qinglong(args) => return execute_qinglong(args).await,
        Command::Dacapo {
            config: path,
            tasks,
        } => return execute_dacapo(&path, &tasks).await,
        Command::Schedule {
            config: path,
            tasks,
        } => {
            let mut first = true;
            let mut last_exit = 0;
            loop {
                let loaded = config::load(&path)?;
                for warning in &loaded.warnings {
                    tracing::warn!("{warning}");
                }
                let schedule = loaded.config.runtime.schedule.clone();
                if !schedule.enabled {
                    if first {
                        return Err(AppError::Task(
                            "runtime.schedule.enabled=false，拒绝启动定时运行".to_owned(),
                        ));
                    }
                    tracing::info!("检测到定时运行已关闭，退出 schedule");
                    return Ok(last_exit);
                }
                if first && !schedule.run_on_start {
                    first = false;
                    service::wait_schedule_interval(&schedule).await;
                    continue;
                }
                let persistence = credential_persistence(loaded.source, &path);
                let (config, report) = execute_run(loaded.config, persistence, &tasks).await;
                last_exit =
                    finish_report(&config, &report, ReportFormat::Text, false, false).await?;
                tracing::info!(exit_code = last_exit, "定时任务本轮结束");
                first = false;
                service::wait_schedule_interval(&schedule).await;
            }
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
            ConfigCommand::Init { config: path } => {
                let result = config::interactive_init(&path).await?;
                if result.created && result.run_now {
                    return execute_run_command(
                        &path,
                        &[],
                        false,
                        false,
                        ReportFormat::Text,
                        false,
                    )
                    .await;
                }
            }
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

#[derive(Serialize)]
struct StructuredRunOutput<'a> {
    schema_version: u8,
    exit_code: u8,
    records: &'a [service::TaskRecord],
    notifications: &'a [push::DeliveryResult],
}

async fn finish_report(
    config: &config::Config,
    report: &service::RunReport,
    output: ReportFormat,
    no_notify: bool,
    verbose: bool,
) -> Result<u8, AppError> {
    let rendered = if verbose {
        report.render_verbose_text()
    } else {
        report.render_text()
    };
    tracing::info!("任务报告\n{}", rendered.trim_end());
    let push_report = if no_notify {
        push::PushReport::default()
    } else {
        push::send_report(config, report).await
    };
    for delivery in &push_report.deliveries {
        match &delivery.status {
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
    let exit_code = report.exit_code();
    match output {
        ReportFormat::Text => print!("{rendered}"),
        ReportFormat::Json => {
            let output = StructuredRunOutput {
                schema_version: 1,
                exit_code,
                records: &report.records,
                notifications: &push_report.deliveries,
            };
            println!(
                "{}",
                serde_json::to_string(&output)
                    .map_err(|error| AppError::Task(format!("JSON 报告序列化失败：{error}")))?
            );
        }
    }
    Ok(exit_code)
}

fn cli_config_path(cli: &Cli) -> Option<&std::path::Path> {
    match &cli.command {
        Command::ValidateConfig { config }
        | Command::Doctor { config, .. }
        | Command::Checkin { config, .. }
        | Command::Run { config, .. }
        | Command::Schedule { config, .. } => Some(config),
        Command::Config { command } => match command {
            ConfigCommand::Init { config }
            | ConfigCommand::Setup { config }
            | ConfigCommand::Edit { config }
            | ConfigCommand::AddAccount { config, .. }
            | ConfigCommand::RemoveAccount { config, .. } => Some(config),
        },
        _ => None,
    }
}

fn initial_runtime(cli: &Cli) -> Option<config::RuntimeConfig> {
    match &cli.command {
        Command::RunDirectory(args) => {
            return initial_directory_runtime(&args.directory, args.prefix.as_deref());
        }
        Command::Qinglong(_) => {
            let settings = service::qinglong_settings().ok()?;
            return if settings.multi {
                initial_directory_runtime(&settings.directory, settings.prefix.as_deref())
            } else {
                config::load(&settings.single_config)
                    .ok()
                    .map(|loaded| loaded.config.runtime)
            };
        }
        Command::Dacapo { config: path, .. } => {
            return config::load_dacapo(path)
                .ok()
                .map(|loaded| loaded.config.runtime);
        }
        _ => {}
    }
    cli_config_path(cli)
        .and_then(|path| config::load(path).ok().map(|loaded| loaded.config.runtime))
}

fn initial_directory_runtime(
    directory: &std::path::Path,
    prefix: Option<&str>,
) -> Option<config::RuntimeConfig> {
    service::discover_config_files(directory, prefix)
        .ok()?
        .into_iter()
        .find_map(|path| config::load(&path).ok().map(|loaded| loaded.config.runtime))
}

async fn execute_config_path(path: &std::path::Path, tasks: &[RunTask]) -> Result<u8, AppError> {
    let loaded = config::load(path)?;
    for warning in &loaded.warnings {
        tracing::warn!("{warning}");
    }
    let persistence = credential_persistence(loaded.source, path);
    let (config, report) = execute_run(loaded.config, persistence, tasks).await;
    finish_report(&config, &report, ReportFormat::Text, false, false).await
}

async fn execute_run_command(
    path: &std::path::Path,
    tasks: &[RunTask],
    read_only: bool,
    no_notify: bool,
    output: ReportFormat,
    verbose: bool,
) -> Result<u8, AppError> {
    let loaded = if path == std::path::Path::new("-") {
        let mut source = String::new();
        std::io::stdin()
            .read_to_string(&mut source)
            .map_err(|_| AppError::StandardInput)?;
        config::load_from_str(&source, "stdin")?
    } else {
        config::load(path)?
    };
    for warning in &loaded.warnings {
        tracing::warn!("{warning}");
    }
    let persistence = if read_only || path == std::path::Path::new("-") {
        service::CredentialPersistence::ReadOnly
    } else {
        credential_persistence(loaded.source, path)
    };
    let (config, report) = execute_run(loaded.config, persistence, tasks).await;
    finish_report(&config, &report, output, no_notify, verbose).await
}

async fn execute_directory(args: DirectoryRunArgs) -> Result<u8, AppError> {
    if args.delay_min_seconds > args.delay_max_seconds || args.delay_max_seconds > 3_600 {
        return Err(AppError::Task(
            "多配置等待范围必须满足 0 <= 最小秒数 <= 最大秒数 <= 3600".to_owned(),
        ));
    }
    let paths = service::discover_config_files(&args.directory, args.prefix.as_deref())?;
    let total = paths.len();
    let mut batch = service::BatchReport::default();
    for (index, path) in paths.into_iter().enumerate() {
        let source = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        println!("=== 配置文件：{source} ===");
        match config::load(&path) {
            Ok(loaded) => {
                for warning in &loaded.warnings {
                    tracing::warn!(config_file = %source, "{warning}");
                }
                let persistence = credential_persistence(loaded.source, &path);
                let (config, report) = execute_run(loaded.config, persistence, &args.tasks).await;
                let exit_code =
                    finish_report(&config, &report, ReportFormat::Text, false, false).await?;
                batch.push_completed(source, exit_code);
            }
            Err(error) => {
                let safe_error = format!("配置错误：{error}");
                tracing::error!(config_file = %source, "{safe_error}");
                batch.push_failed(source, 2, safe_error);
            }
        }
        if index + 1 < total {
            wait_directory_delay(args.delay_min_seconds, args.delay_max_seconds).await;
        }
    }
    let summary = batch.render_summary();
    print!("{summary}");
    tracing::info!("{}", summary.trim_end());
    Ok(batch.exit_code())
}

async fn execute_qinglong(args: QinglongArgs) -> Result<u8, AppError> {
    let settings = service::qinglong_settings()?;
    if !settings.project_notifications {
        tracing::warn!(
            "Rust 版不加载青龙 notify.py；仅使用各配置的 notifications，建议设置 AutoMihoyoBBS_push_project=1"
        );
    }
    if settings.multi {
        tracing::info!(
            directory = %settings.directory.display(),
            prefix = settings.prefix.as_deref().unwrap_or(""),
            "青龙多配置模式"
        );
        execute_directory(DirectoryRunArgs {
            directory: settings.directory,
            prefix: settings.prefix,
            tasks: args.tasks,
            delay_min_seconds: args.delay_min_seconds,
            delay_max_seconds: args.delay_max_seconds,
        })
        .await
    } else {
        tracing::info!(config = %settings.single_config.display(), "青龙单配置模式");
        execute_config_path(&settings.single_config, &args.tasks).await
    }
}

async fn execute_dacapo(path: &std::path::Path, tasks: &[RunTask]) -> Result<u8, AppError> {
    let loaded = config::load_dacapo(path)?;
    for warning in &loaded.warnings {
        tracing::warn!("{warning}");
    }
    let (config, report) = execute_run(
        loaded.config,
        service::CredentialPersistence::ReadOnly,
        tasks,
    )
    .await;
    finish_report(&config, &report, ReportFormat::Text, false, false).await
}

async fn wait_directory_delay(minimum: u64, maximum: u64) {
    let seconds = if minimum == maximum {
        minimum
    } else {
        rand::rng().random_range(minimum..=maximum)
    };
    if seconds > 0 {
        tracing::info!(seconds, "多配置运行等待下一个文件");
        tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
    }
}

async fn execute_run(
    mut config: config::Config,
    persistence: service::CredentialPersistence<'_>,
    tasks: &[RunTask],
) -> (config::Config, service::RunReport) {
    service::apply_runtime_delay(&config.runtime).await;
    let all = tasks.is_empty();
    let mut report = service::RunReport::default();
    if all || tasks.contains(&RunTask::ChinaCheckin) {
        report.extend(service::run_china_checkin_with_persistence(&mut config, persistence).await);
    }
    if all || tasks.contains(&RunTask::HoyolabCheckin) {
        report.extend(service::run_hoyolab_checkin(&config).await);
    }
    if all || tasks.contains(&RunTask::Bbs) {
        report.extend(service::run_bbs_with_persistence(&mut config, persistence).await);
    }
    let china_cloud = all || tasks.contains(&RunTask::ChinaCloudGame);
    let overseas_cloud = all || tasks.contains(&RunTask::OverseasCloudGame);
    if china_cloud || overseas_cloud {
        report.extend(service::run_cloud_games(&config, china_cloud, overseas_cloud).await);
    }
    if all || tasks.contains(&RunTask::WebActivity) {
        report.extend(service::run_web_activities(&config));
    }
    (config, report)
}

fn credential_persistence<'a>(
    source: config::ConfigSource,
    path: &'a std::path::Path,
) -> service::CredentialPersistence<'a> {
    if source.supports_persistent_refresh() {
        service::CredentialPersistence::CurrentConfig(path)
    } else {
        service::CredentialPersistence::ReadOnly
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
        .with_writer(std::io::stderr)
        .init();
    None
}
