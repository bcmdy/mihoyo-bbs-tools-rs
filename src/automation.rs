use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const TASK_NAME: &str = "MihoyoBBSToolsRS-Daily";

#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub config: PathBuf,
    pub time: String,
    pub only_when_logged_on: bool,
    pub retry_count: u8,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AutomationStatus {
    pub installed: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub next_run_time: Option<String>,
    #[serde(default)]
    pub last_run_time: Option<String>,
    #[serde(default)]
    pub last_result: Option<i64>,
    #[serde(default)]
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    #[serde(skip_deserializing, default)]
    pub executable_exists: bool,
    #[serde(skip_deserializing, default)]
    pub config_exists: bool,
}

#[derive(Debug, Error)]
pub enum AutomationError {
    #[error("自动运行管理首期仅支持 Windows 任务计划程序")]
    UnsupportedPlatform,
    #[error("每日执行时间必须使用 HH:MM 24 小时格式")]
    InvalidTime,
    #[error("配置文件不存在：{0}")]
    MissingConfig(PathBuf),
    #[error("任务失败重试次数必须在 0 到 10 之间")]
    InvalidRetryCount,
    #[error("无法定位当前程序：{0}")]
    CurrentExecutable(std::io::Error),
    #[error("无法执行 Windows PowerShell：{0}")]
    PowerShell(String),
    #[error("Windows 任务状态响应无效：{0}")]
    InvalidStatus(String),
}

pub fn task_name() -> &'static str {
    TASK_NAME
}

pub fn install(options: &InstallOptions) -> Result<(), AutomationError> {
    ensure_windows()?;
    validate_time(&options.time)?;
    if options.retry_count > 10 {
        return Err(AutomationError::InvalidRetryCount);
    }
    if !options.config.is_file() {
        return Err(AutomationError::MissingConfig(options.config.clone()));
    }
    let executable = std::env::current_exe().map_err(AutomationError::CurrentExecutable)?;
    let working_directory = executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    run_script(
        INSTALL_SCRIPT,
        &[
            ("TaskName", TASK_NAME.to_owned()),
            ("Executable", executable.display().to_string()),
            (
                "WorkingDirectory",
                working_directory.display().to_string(),
            ),
            ("Config", options.config.display().to_string()),
            ("Time", options.time.clone()),
            (
                "OnlyWhenLoggedOn",
                options.only_when_logged_on.to_string(),
            ),
            ("RetryCount", options.retry_count.to_string()),
        ],
    )?;
    Ok(())
}

pub fn status(config: &Path) -> Result<AutomationStatus, AutomationError> {
    ensure_windows()?;
    let output = run_script(STATUS_SCRIPT, &[("TaskName", TASK_NAME.to_owned())])?;
    let mut status: AutomationStatus = serde_json::from_str(output.trim())
        .map_err(|_| AutomationError::InvalidStatus("无法解析 PowerShell JSON".to_owned()))?;
    status.executable_exists = status
        .executable
        .as_ref()
        .is_some_and(|path| path.is_file());
    status.config_exists = config.is_file();
    Ok(status)
}

pub fn run_now() -> Result<(), AutomationError> {
    ensure_windows()?;
    run_script(RUN_NOW_SCRIPT, &[("TaskName", TASK_NAME.to_owned())])?;
    Ok(())
}

pub fn uninstall() -> Result<(), AutomationError> {
    ensure_windows()?;
    run_script(UNINSTALL_SCRIPT, &[("TaskName", TASK_NAME.to_owned())])?;
    Ok(())
}

fn validate_time(value: &str) -> Result<(), AutomationError> {
    let Some((hour, minute)) = value.split_once(':') else {
        return Err(AutomationError::InvalidTime);
    };
    let valid = hour.len() == 2
        && minute.len() == 2
        && hour.parse::<u8>().is_ok_and(|value| value < 24)
        && minute.parse::<u8>().is_ok_and(|value| value < 60);
    if valid {
        Ok(())
    } else {
        Err(AutomationError::InvalidTime)
    }
}

fn ensure_windows() -> Result<(), AutomationError> {
    if cfg!(windows) {
        Ok(())
    } else {
        Err(AutomationError::UnsupportedPlatform)
    }
}

fn run_script(script: &str, parameters: &[(&str, String)]) -> Result<String, AutomationError> {
    let path = std::env::temp_dir().join(format!(
        "mihoyo-bbs-tools-automation-{}-{}.ps1",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let script = format!(
        "$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)\n{script}"
    );
    fs::write(&path, script).map_err(|error| AutomationError::PowerShell(error.to_string()))?;
    let _cleanup = ScriptCleanup(path.clone());
    let mut command = Command::new("powershell.exe");
    command
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(&path);
    for (name, value) in parameters {
        command.arg(format!("-{name}")).arg(value);
    }
    let output = command
        .output()
        .map_err(|error| AutomationError::PowerShell(error.to_string()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        return Err(AutomationError::PowerShell(safe_powershell_error(
            &message,
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn safe_powershell_error(message: &str) -> String {
    let first = message.lines().find(|line| !line.trim().is_empty());
    first
        .map(str::trim)
        .filter(|line| line.len() <= 240)
        .unwrap_or("命令执行失败")
        .to_owned()
}

struct ScriptCleanup(PathBuf);

impl Drop for ScriptCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

const INSTALL_SCRIPT: &str = r#"
param(
    [Parameter(Mandatory=$true)][string]$TaskName,
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$WorkingDirectory,
    [Parameter(Mandatory=$true)][string]$Config,
    [Parameter(Mandatory=$true)][string]$Time,
    [Parameter(Mandatory=$true)][string]$OnlyWhenLoggedOn,
    [Parameter(Mandatory=$true)][int]$RetryCount
)
$ErrorActionPreference = 'Stop'
$action = New-ScheduledTaskAction -Execute $Executable -Argument ('run --config "' + $Config + '"') -WorkingDirectory $WorkingDirectory
$trigger = New-ScheduledTaskTrigger -Daily -At $Time
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
if ($RetryCount -gt 0) {
    $settings.RestartCount = $RetryCount
    $settings.RestartInterval = 'PT5M'
}
$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
$logonType = if ($OnlyWhenLoggedOn -eq 'true') { 'Interactive' } else { 'S4U' }
$principal = New-ScheduledTaskPrincipal -UserId $identity -LogonType $logonType -RunLevel Limited
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Force | Out-Null
"#;

const STATUS_SCRIPT: &str = r#"
param([Parameter(Mandatory=$true)][string]$TaskName)
$ErrorActionPreference = 'Stop'
$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($null -eq $task) {
    [pscustomobject]@{ installed = $false; enabled = $false } | ConvertTo-Json -Compress
    exit 0
}
$info = Get-ScheduledTaskInfo -TaskName $TaskName
$action = @($task.Actions)[0]
[pscustomobject]@{
    installed = $true
    enabled = [bool]$task.Settings.Enabled
    state = [string]$task.State
    next_run_time = if ($info.NextRunTime.Year -gt 1900) { $info.NextRunTime.ToString('yyyy-MM-dd HH:mm:ss') } else { $null }
    last_run_time = if ($info.LastRunTime.Year -gt 1900) { $info.LastRunTime.ToString('yyyy-MM-dd HH:mm:ss') } else { $null }
    last_result = [long]$info.LastTaskResult
    executable = [string]$action.Execute
    arguments = [string]$action.Arguments
    working_directory = [string]$action.WorkingDirectory
} | ConvertTo-Json -Compress
"#;

const RUN_NOW_SCRIPT: &str = r#"
param([Parameter(Mandatory=$true)][string]$TaskName)
$ErrorActionPreference = 'Stop'
Start-ScheduledTask -TaskName $TaskName
"#;

const UNINSTALL_SCRIPT: &str = r#"
param([Parameter(Mandatory=$true)][string]$TaskName)
$ErrorActionPreference = 'Stop'
$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($null -ne $task) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
}
"#;
