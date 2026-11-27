use std::{
    io::{IsTerminal, Read},
    process::ExitCode,
    time::Duration,
};

use clap::Parser;
use mihoyo_bbs_tools::{
    auth,
    cli::{
        AutomationCommand, CheckinRegion, Cli, Command, ConfigCommand, DirectoryRunArgs,
        NotificationCommand, QinglongArgs, ReportFormat, RunTask, UpdateCommand,
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
            // 首次配置错误明确失败退出；运行中失败由可取消等待按退避间隔重读恢复。
            let mut current = config::load(&path)?;
            let (shutdown_sender, mut shutdown) = service::shutdown_channel();
            service::spawn_shutdown_listener(shutdown_sender);
            let reload_backoff = schedule_reload_backoff();
            let mut first_round = true;
            let mut last_exit = 0;
            loop {
                for warning in &current.warnings {
                    tracing::warn!("{warning}");
                }
                let schedule = current.config.runtime.schedule.clone();
                if !schedule.enabled {
                    if first_round {
                        return Err(AppError::Task(
                            "runtime.schedule.enabled=false，拒绝启动定时运行".to_owned(),
                        ));
                    }
                    return Ok(finish_schedule(last_exit, "配置已关闭定时运行"));
                }
                let skip_run = first_round && !schedule.run_on_start;
                first_round = false;
                if skip_run {
                    tracing::info!(
                        minutes = schedule.interval_minutes,
                        "定时运行配置为先等待一个完整间隔"
                    );
                } else {
                    let persistence = credential_persistence(current.source, &path);
                    let (config, report) = execute_run(current.config, persistence, &tasks).await;
                    last_exit =
                        finish_report(&config, &report, ReportFormat::Text, false, false).await?;
                    tracing::info!(exit_code = last_exit, "定时任务本轮结束");
                    // 退出信号只在轮次边界生效，不会中断已经开始的任务提交。
                }
                match service::wait_for_next_round(
                    &schedule,
                    &reload_backoff,
                    &mut shutdown,
                    || config::load(&path),
                )
                .await
                {
                    service::ScheduleWait::Ready(next) => current = *next,
                    service::ScheduleWait::Disabled => {
                        return Ok(finish_schedule(last_exit, "等待期间检测到定时运行已关闭"));
                    }
                    service::ScheduleWait::Cancelled => {
                        return Ok(finish_schedule(last_exit, "收到退出信号"));
                    }
                }
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
        Command::QbotOpenId {
            config: path,
            appid,
            secret,
            timeout,
        } => {
            return execute_qbot_openid(&path, appid, secret, timeout).await;
        }
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
                let mut notification_failed = false;
                if result.created && result.test_notifications {
                    let loaded = config::load(&path)?;
                    let report = push::test_providers(&loaded.config, None)
                        .await
                        .map_err(AppError::Task)?;
                    for delivery in &report.deliveries {
                        println!(
                            "[{}] {}：{}",
                            if delivery.status == DeliveryStatus::Sent {
                                "成功"
                            } else {
                                "失败"
                            },
                            delivery.provider.display_name(),
                            delivery.message
                        );
                    }
                    notification_failed = !report.all_succeeded();
                }
                if let Some(time) = result.automation_time {
                    let path = std::fs::canonicalize(&path).map_err(|error| {
                        AppError::Task(format!("无法解析配置绝对路径：{error}"))
                    })?;
                    mihoyo_bbs_tools::automation::install(
                        &mihoyo_bbs_tools::automation::InstallOptions {
                            config: path,
                            time,
                            only_when_logged_on: true,
                            retry_count: 3,
                        },
                    )
                    .map_err(|error| AppError::Task(error.to_string()))?;
                    println!("Windows 每日自动运行任务已安装");
                }
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
                if notification_failed {
                    return Ok(1);
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
            ConfigCommand::RemoveAccount {
                config: path,
                name,
                yes,
            } => {
                let loaded = config::load(&path)?;
                let account = loaded
                    .config
                    .accounts
                    .iter()
                    .find(|account| account.name == name)
                    .ok_or_else(|| AppError::Task(format!("未找到账号 {name:?}")))?;
                println!("即将删除账号：{}", account.name);
                println!("备注：{}", account.remark.as_deref().unwrap_or("未设置"));
                let confirmed = if yes {
                    true
                } else if std::io::stdin().is_terminal() {
                    config::confirm_interactive("确认删除该账号？", false)?
                } else {
                    return Err(AppError::Task(
                        "非交互环境删除账号必须显式提供 --yes".to_owned(),
                    ));
                };
                if confirmed {
                    config::remove_account(&path, &name)?;
                    println!("已删除账号：{name}");
                } else {
                    println!("已取消删除账号");
                }
            }
            ConfigCommand::Backup { config: path, keep } => {
                let backup = config::create_backup(&path, keep)?;
                println!("配置备份已创建：{}", backup.display());
            }
            ConfigCommand::ListBackups { config: path } => {
                let backups = config::list_backups(&path)?;
                if backups.is_empty() {
                    println!("尚无可用配置备份");
                }
                for (index, backup) in backups.iter().enumerate() {
                    println!(
                        "{}. {} / version {} / {} 个账号 / {}",
                        index + 1,
                        backup.created,
                        backup
                            .version
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "无效".to_owned()),
                        backup
                            .account_count
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "未知".to_owned()),
                        backup.path.display()
                    );
                }
            }
            ConfigCommand::Restore {
                config: path,
                backup,
                yes,
            } => {
                let preview = config::load(&backup)?;
                println!("即将恢复配置备份：");
                println!("- 备份：{}", backup.display());
                println!("- 目标：{}", path.display());
                println!("- 版本：{}", preview.config.version);
                println!("- 账号数量：{}", preview.config.accounts.len());
                let confirmed = if yes {
                    true
                } else if std::io::stdin().is_terminal() {
                    config::confirm_interactive("恢复前会先备份当前配置，确认继续？", false)?
                } else {
                    return Err(AppError::Task(
                        "非交互环境恢复配置必须显式提供 --yes".to_owned(),
                    ));
                };
                if confirmed {
                    let current_backup = config::restore_backup(&path, &backup)?;
                    println!("配置已恢复；恢复前配置备份：{}", current_backup.display());
                } else {
                    println!("已取消恢复，当前配置未修改");
                }
            }
        },
        Command::Notification { command } => match command {
            NotificationCommand::List { config: path } => {
                let loaded = config::load(&path)?;
                let summaries = push::provider_summaries(&loaded.config);
                println!(
                    "通知总开关：{}；已配置 {} 个渠道",
                    if loaded.config.notifications.enabled {
                        "已启用"
                    } else {
                        "已关闭"
                    },
                    summaries.len()
                );
                for provider in summaries {
                    println!(
                        "{}. {}（{}） / {}",
                        provider.index,
                        provider.kind.display_name(),
                        provider.kind.as_str(),
                        provider.target
                    );
                }
            }
            NotificationCommand::Test {
                config: path,
                provider,
            } => {
                let loaded = config::load(&path)?;
                let report = push::test_providers(&loaded.config, provider)
                    .await
                    .map_err(AppError::Task)?;
                for delivery in &report.deliveries {
                    println!(
                        "[{}] {}（{}）：{}",
                        if delivery.status == DeliveryStatus::Sent {
                            "成功"
                        } else {
                            "失败"
                        },
                        delivery.provider.display_name(),
                        delivery.provider.as_str(),
                        delivery.message
                    );
                }
                return Ok(u8::from(!report.all_succeeded()));
            }
        },
        Command::Automation { command } => match command {
            AutomationCommand::Install {
                config: path,
                time,
                run_whether_logged_on,
                retry_count,
                enable_windows_notification,
            } => {
                if !cfg!(windows) {
                    return Err(AppError::Task(
                        "自动运行管理首期仅支持 Windows 任务计划程序".to_owned(),
                    ));
                }
                let path = std::fs::canonicalize(&path)
                    .map_err(|_| AppError::Task(format!("配置文件不存在：{}", path.display())))?;
                if enable_windows_notification {
                    let loaded = config::load(&path)?;
                    let has_toast = push::provider_summaries(&loaded.config)
                        .iter()
                        .any(|provider| provider.kind == push::ProviderKind::WindowsToast);
                    if !has_toast {
                        config::set_notification_provider(
                            &path,
                            None,
                            "windows_toast",
                            &[(
                                "title_prefix".to_owned(),
                                Some("MihoyoBBSTools RS".to_owned()),
                            )],
                        )?;
                    }
                    let loaded = config::load(&path)?;
                    config::set_notification_options(
                        &path,
                        true,
                        loaded.config.notifications.error_only,
                        loaded.config.notifications.block_keywords,
                    )?;
                }
                let executable = std::env::current_exe()
                    .map_err(|error| AppError::Task(format!("无法定位当前程序：{error}")))?;
                let working_directory = executable
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."));
                let loaded = config::load(&path)?;
                println!("将安装或更新 Windows 自动运行任务：");
                println!("- 任务名称：{}", mihoyo_bbs_tools::automation::task_name());
                println!("- 程序：{}", executable.display());
                println!("- 工作目录：{}", working_directory.display());
                println!("- 参数：run --config \"{}\"", path.display());
                println!(
                    "- 每日本地时间：{time}（配置时区：{}）",
                    loaded.config.runtime.timezone
                );
                println!(
                    "- 登录条件：{}",
                    if run_whether_logged_on {
                        "用户未登录时也运行（S4U）"
                    } else {
                        "仅在用户登录时运行"
                    }
                );
                println!("- 失败后重试：{retry_count} 次，每次间隔 5 分钟");
                mihoyo_bbs_tools::automation::install(
                    &mihoyo_bbs_tools::automation::InstallOptions {
                        config: path,
                        time,
                        only_when_logged_on: !run_whether_logged_on,
                        retry_count,
                    },
                )
                .map_err(|error| AppError::Task(error.to_string()))?;
                println!("自动运行任务已安装");
            }
            AutomationCommand::Status { config: path } => {
                let status = mihoyo_bbs_tools::automation::status(&path)
                    .map_err(|error| AppError::Task(error.to_string()))?;
                if !status.installed {
                    println!("自动运行任务尚未安装");
                    return Ok(1);
                }
                println!("任务名称：{}", mihoyo_bbs_tools::automation::task_name());
                println!("状态：{}", status.state.as_deref().unwrap_or("未知"));
                println!("已启用：{}", if status.enabled { "是" } else { "否" });
                println!(
                    "下次运行：{}",
                    status.next_run_time.as_deref().unwrap_or("未知")
                );
                println!(
                    "上次运行：{}",
                    status.last_run_time.as_deref().unwrap_or("从未运行")
                );
                println!(
                    "上次退出码：{}",
                    status
                        .last_result
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "未知".to_owned())
                );
                println!(
                    "程序路径：{}（{}）",
                    status
                        .executable
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "未知".to_owned()),
                    if status.executable_exists {
                        "有效"
                    } else {
                        "失效"
                    }
                );
                println!(
                    "配置路径：{}（{}）",
                    path.display(),
                    if status.config_exists {
                        "有效"
                    } else {
                        "失效"
                    }
                );
                return Ok(u8::from(
                    !status.enabled || !status.executable_exists || !status.config_exists,
                ));
            }
            AutomationCommand::RunNow => {
                mihoyo_bbs_tools::automation::run_now()
                    .map_err(|error| AppError::Task(error.to_string()))?;
                println!("已触发自动运行任务；可使用 `automation status` 查看结果");
            }
            AutomationCommand::Uninstall => {
                mihoyo_bbs_tools::automation::uninstall()
                    .map_err(|error| AppError::Task(error.to_string()))?;
                println!("自动运行任务已移除；配置和日志未删除");
            }
        },
        Command::Update { command } => match command {
            UpdateCommand::Check => {
                let update = mihoyo_bbs_tools::update::check().await?;
                println!("当前版本：{}", update.current_version);
                println!("最新稳定版本：{}", update.latest_version);
                println!(
                    "检查结果：{}",
                    if update.update_available {
                        "发现新版本"
                    } else {
                        "当前已是最新稳定版本"
                    }
                );
                println!("发布页面：{}", update.release_url);
                println!("配置兼容：{}", update.config_compatibility);
            }
        },
    }
    Ok(0)
}

/// JSON 报告结构版本；A15 起为 2，新增字段均为可选，旧消费者可继续读取。
const STRUCTURED_OUTPUT_SCHEMA_VERSION: u8 = 2;

#[derive(Serialize)]
struct StructuredRunOutput<'a> {
    schema_version: u8,
    exit_code: u8,
    /// 仅统计真正的任务结果，不含诊断与汇总记录。
    task_counts: service::TaskCounts,
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
                schema_version: STRUCTURED_OUTPUT_SCHEMA_VERSION,
                exit_code,
                task_counts: report.task_counts(),
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
        | Command::Schedule { config, .. }
        | Command::QbotOpenId { config, .. } => Some(config),
        Command::Config { command } => match command {
            ConfigCommand::Init { config }
            | ConfigCommand::Setup { config }
            | ConfigCommand::Edit { config }
            | ConfigCommand::AddAccount { config, .. }
            | ConfigCommand::RemoveAccount { config, .. }
            | ConfigCommand::Backup { config, .. }
            | ConfigCommand::ListBackups { config }
            | ConfigCommand::Restore { config, .. } => Some(config),
        },
        Command::Notification { command } => match command {
            NotificationCommand::List { config } | NotificationCommand::Test { config, .. } => {
                Some(config)
            }
        },
        Command::Automation { command } => match command {
            AutomationCommand::Install { config, .. } | AutomationCommand::Status { config } => {
                Some(config)
            }
            AutomationCommand::RunNow | AutomationCommand::Uninstall => None,
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
            let settings = match service::qinglong_settings() {
                Ok(settings) => settings,
                Err(error) => {
                    eprintln!("警告：读取青龙环境配置失败，暂不启用文件日志：{error}");
                    return None;
                }
            };
            return if settings.multi {
                initial_directory_runtime(&settings.directory, settings.prefix.as_deref())
            } else {
                load_initial_runtime(&settings.single_config)
            };
        }
        Command::Dacapo { config: path, .. } => {
            return match config::load_dacapo(path) {
                Ok(loaded) => Some(loaded.config.runtime),
                Err(error) => {
                    eprintln!(
                        "警告：读取 DaCapo 配置 {} 失败，本次不启用文件日志（命令执行时会再次报告完整错误）：{error}",
                        path.display()
                    );
                    None
                }
            };
        }
        _ => {}
    }
    cli_config_path(cli).and_then(load_initial_runtime)
}

fn load_initial_runtime(path: &std::path::Path) -> Option<config::RuntimeConfig> {
    match config::load(path) {
        Ok(loaded) => Some(loaded.config.runtime),
        Err(error) => {
            eprintln!(
                "警告：无法为日志初始化读取配置 {}，本次不启用文件日志（命令执行时会再次报告完整错误）：{error}",
                path.display()
            );
            None
        }
    }
}

fn initial_directory_runtime(
    directory: &std::path::Path,
    prefix: Option<&str>,
) -> Option<config::RuntimeConfig> {
    let paths = match service::discover_config_files(directory, prefix) {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!(
                "警告：无法枚举配置目录 {}，本次不启用文件日志：{error}",
                directory.display()
            );
            return None;
        }
    };
    paths
        .into_iter()
        .find_map(|path| match config::load(&path) {
            Ok(loaded) => Some(loaded.config.runtime),
            Err(error) => {
                eprintln!(
                    "警告：无法为日志初始化读取配置 {}，跳过该文件：{error}",
                    path.display()
                );
                None
            }
        })
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

/// 获取 QQ 机器人单聊 OpenID 并写回配置。
///
/// 交互约定：AppID 可以来自参数或配置；AppSecret 属于敏感凭据，优先从参数/配置读取，
/// 缺失时走星号掩码输入，避免出现在命令行历史与日志里。
async fn execute_qbot_openid(
    path: &std::path::Path,
    appid: Option<String>,
    secret: Option<String>,
    timeout: u64,
) -> Result<u8, AppError> {
    let loaded = config::load(path)?;
    let existing = loaded
        .config
        .notifications
        .providers
        .iter()
        .find_map(|provider| {
            if let config::NotificationProvider::Qbot {
                appid: configured_appid,
                secret: configured_secret,
                ..
            } = provider
            {
                Some((configured_appid.clone(), configured_secret.clone()))
            } else {
                None
            }
        });
    let (configured_appid, configured_secret) = match existing {
        Some((appid, secret)) => (Some(appid), Some(secret)),
        None => (None, None),
    };

    let appid = match appid.or(configured_appid) {
        Some(appid) if !appid.trim().is_empty() => appid,
        _ => config::prompt_interactive("QQ 机器人 AppID")?,
    };
    if appid.trim().is_empty() {
        return Err(AppError::Task("AppID 不能为空".to_owned()));
    }
    let secret = match secret {
        Some(secret) if !secret.trim().is_empty() => secret,
        _ => match configured_secret.filter(|secret| !secret.expose_secret().trim().is_empty()) {
            Some(secret) => secret.expose_secret().to_owned(),
            None => config::prompt_secret("QQ 机器人 AppSecret")?,
        },
    };
    if secret.trim().is_empty() {
        return Err(AppError::Task("AppSecret 不能为空".to_owned()));
    }

    let http = mihoyo_bbs_tools::http::factory::for_update()?;
    println!("正在获取访问令牌并连接 QQ 机器人网关…");
    println!("请使用手机 QQ 向该机器人发送一条单聊消息，收到后会自动写入配置。");
    let openid = mihoyo_bbs_tools::push::fetch_qbot_open_id(
        &http,
        &appid,
        &auth::SecretString::new(secret.clone()),
        None,
        timeout,
    )
    .await
    .map_err(|error| AppError::Task(error.to_string()))?;

    let index = config::set_qbot_open_id(path, &openid.openid, &appid, &secret)?;
    println!("已获取单聊 OpenID 并写入第 {index} 个通知渠道（qbot）。");
    println!("配置中的 openid 已脱敏保存；如需推送请确认 notifications.enabled 为 true。");
    if openid.expires_in > 0 {
        println!(
            "本次访问令牌有效约 {} 秒，仅用于本次操作。",
            openid.expires_in
        );
    }
    Ok(0)
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
fn schedule_reload_backoff() -> Vec<Duration> {
    service::SCHEDULE_RELOAD_BACKOFF_SECONDS
        .iter()
        .map(|seconds| Duration::from_secs(*seconds))
        .collect()
}

/// 常驻调度退出时记录停止原因，并返回最后一次完整轮次的退出码。
fn finish_schedule(exit_code: u8, reason: &str) -> u8 {
    tracing::info!(exit_code, reason = %reason, "定时运行已停止");
    exit_code
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
        match file_logging::DailyFileAppender::new(&logging.directory, &logging.file_prefix) {
            Ok(appender) => {
                let (writer, guard) = tracing_appender::non_blocking(appender);
                tracing_subscriber::fmt()
                    .with_max_level(level)
                    .with_target(false)
                    .with_ansi(false)
                    .with_writer(writer)
                    .init();
                prune_logs(logging);
                return Some(guard);
            }
            Err(error) => {
                // 这里尚未初始化 subscriber，直接写标准错误，只输出路径与错误类别。
                eprintln!(
                    "警告：无法创建文件日志（目录 {}，原因类别：{:?}）；本次运行只写入标准错误输出。",
                    logging.directory.display(),
                    error.kind()
                );
            }
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

fn prune_logs(logging: &config::LoggingConfig) {
    let Some(retention_days) = logging.retention_days else {
        return;
    };
    let stem = file_logging::prefix_stem(&logging.file_prefix);
    let report = file_logging::prune(&logging.directory, &stem, retention_days);
    if !report.removed.is_empty() {
        tracing::info!(
            removed = report.removed.len(),
            retention_days,
            directory = %logging.directory.display(),
            "已清理过期日志文件"
        );
    }
    for error in &report.errors {
        tracing::warn!("{error}");
    }
}
