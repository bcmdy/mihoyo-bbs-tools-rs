use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use thiserror::Error;

const DEFAULT_LAUNCHER_NAME: &str = "MihoyoBBSToolsRS-run.bat";

#[derive(Debug, Error)]
pub enum LauncherError {
    #[error("创建 BAT 仅支持 Windows")]
    UnsupportedPlatform,
    #[error("无法获取当前程序路径：{0}")]
    CurrentExe(std::io::Error),
    #[error("当前程序路径不是有效 Unicode，无法写入 BAT")]
    NonUnicodeExePath,
    #[error("当前程序没有可用的所在目录")]
    MissingExeDirectory,
    #[error("BAT 已存在：{0}；如需覆盖请添加 --force")]
    AlreadyExists(PathBuf),
    #[error("无法写入 BAT {path}：{source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn create_windows_launcher(
    output: Option<&Path>,
    force: bool,
) -> Result<PathBuf, LauncherError> {
    if !cfg!(windows) {
        return Err(LauncherError::UnsupportedPlatform);
    }
    let executable = std::env::current_exe().map_err(LauncherError::CurrentExe)?;
    create_launcher_for_executable(&executable, output, force)
}

fn create_launcher_for_executable(
    executable: &Path,
    output: Option<&Path>,
    force: bool,
) -> Result<PathBuf, LauncherError> {
    let directory = executable
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(LauncherError::MissingExeDirectory)?;
    let executable = executable
        .to_str()
        .ok_or(LauncherError::NonUnicodeExePath)?;
    let directory = directory.to_str().ok_or(LauncherError::NonUnicodeExePath)?;
    let output = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(directory).join(DEFAULT_LAUNCHER_NAME));
    let content = render_launcher(directory, executable);

    if force {
        fs::write(&output, content).map_err(|source| LauncherError::Write {
            path: output.clone(),
            source,
        })?;
    } else {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|source| {
                if source.kind() == std::io::ErrorKind::AlreadyExists {
                    LauncherError::AlreadyExists(output.clone())
                } else {
                    LauncherError::Write {
                        path: output.clone(),
                        source,
                    }
                }
            })?;
        file.write_all(content.as_bytes())
            .map_err(|source| LauncherError::Write {
                path: output.clone(),
                source,
            })?;
    }
    Ok(output)
}

fn render_launcher(directory: &str, executable: &str) -> String {
    let directory = escape_batch_value(directory);
    let executable = escape_batch_value(executable);
    format!(
        "@echo off\r\nchcp 65001 >nul\r\n\r\nstart \"\" /D \"{directory}\" \"{executable}\" run\r\nexit /b\r\n"
    )
}

fn escape_batch_value(value: &str) -> String {
    value.replace('%', "%%")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_embeds_executable_and_working_directory_instead_of_batch_location() {
        let content = render_launcher(r"D:\YS\tools\qd", r"D:\YS\tools\qd\MihoyoBBSToolsRS.exe");
        assert!(
            content.contains(
                r#"start "" /D "D:\YS\tools\qd" "D:\YS\tools\qd\MihoyoBBSToolsRS.exe" run"#
            )
        );
        assert!(!content.contains("%~dp0"));
        assert!(content.ends_with("exit /b\r\n"));
    }

    #[test]
    fn launcher_escapes_percent_characters_used_by_cmd_expansion() {
        let content = render_launcher(r"D:\100%\tool", r"D:\100%\tool\MihoyoBBSToolsRS.exe");
        assert!(content.contains(r"D:\100%%\tool"));
        assert!(!content.contains(r"D:\100%\tool"));
    }

    #[test]
    fn refuses_to_overwrite_launcher_without_force() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("MihoyoBBSToolsRS.exe");
        let output = directory.path().join("custom.bat");
        fs::write(&output, "existing").unwrap();

        assert!(matches!(
            create_launcher_for_executable(&executable, Some(&output), false),
            Err(LauncherError::AlreadyExists(path)) if path == output
        ));
        create_launcher_for_executable(&executable, Some(&output), true).unwrap();
        assert!(fs::read_to_string(output).unwrap().contains("start \"\""));
    }
}
