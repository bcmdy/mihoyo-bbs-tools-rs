use std::{
    fmt::Write,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum ConfigDirectoryError {
    #[error("无法读取配置目录 {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("配置目录 {path} 中没有符合条件的 YAML 文件")]
    Empty { path: PathBuf },
}

pub fn discover_config_files(
    directory: &Path,
    prefix: Option<&str>,
) -> Result<Vec<PathBuf>, ConfigDirectoryError> {
    let entries = fs::read_dir(directory).map_err(|source| ConfigDirectoryError::Read {
        path: directory.to_path_buf(),
        source,
    })?;
    let prefix = prefix.filter(|value| !value.is_empty());
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ConfigDirectoryError::Read {
            path: directory.to_path_buf(),
            source,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|source| ConfigDirectoryError::Read {
                path: entry.path(),
                source,
            })?;
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let lower = name.to_ascii_lowercase();
        if !matches!(Path::new(name.as_ref()).extension().and_then(|value| value.to_str()), Some(extension) if extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml"))
            || lower.ends_with(".example.yaml")
            || lower.ends_with(".example.yml")
            || prefix.is_some_and(|prefix| !name.starts_with(prefix))
        {
            continue;
        }
        files.push(entry.path());
    }
    files.sort_by(|left, right| {
        let left_name = left
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let right_name = right
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        left_name.cmp(&right_name).then_with(|| left.cmp(right))
    });
    if files.is_empty() {
        return Err(ConfigDirectoryError::Empty {
            path: directory.to_path_buf(),
        });
    }
    Ok(files)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchEntry {
    pub source: String,
    pub exit_code: u8,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BatchReport {
    pub entries: Vec<BatchEntry>,
}

impl BatchReport {
    pub fn push_completed(&mut self, source: impl Into<String>, exit_code: u8) {
        self.entries.push(BatchEntry {
            source: source.into(),
            exit_code,
            error: None,
        });
    }

    /// `safe_error` 必须已经脱敏，不得包含配置原文或凭据。
    pub fn push_failed(
        &mut self,
        source: impl Into<String>,
        exit_code: u8,
        safe_error: impl Into<String>,
    ) {
        self.entries.push(BatchEntry {
            source: source.into(),
            exit_code,
            error: Some(safe_error.into()),
        });
    }

    pub fn exit_code(&self) -> u8 {
        for candidate in [10, 3, 4, 5, 2, 1] {
            if self
                .entries
                .iter()
                .any(|entry| entry.exit_code == candidate)
            {
                return candidate;
            }
        }
        if self.entries.iter().any(|entry| entry.exit_code != 0) {
            1
        } else {
            0
        }
    }

    pub fn render_summary(&self) -> String {
        let mut output = String::new();
        let _ = writeln!(output, "多配置批次汇总：");
        for entry in &self.entries {
            match &entry.error {
                Some(error) => {
                    let _ = writeln!(
                        output,
                        "[ConfigFailed] {}：{}（退出码 {}）",
                        entry.source, error, entry.exit_code
                    );
                }
                None if entry.exit_code == 0 => {
                    let _ = writeln!(output, "[Success] {}：执行完成", entry.source);
                }
                None => {
                    let _ = writeln!(
                        output,
                        "[TaskFailed] {}：任务未全部成功（退出码 {}）",
                        entry.source, entry.exit_code
                    );
                }
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_filters_examples_applies_prefix_and_sorts() {
        let directory = tempfile::tempdir().unwrap();
        for name in [
            "mhy_b.yaml",
            "mhy_A.YML",
            "mhy_config.example.yaml",
            "other.yaml",
            "notes.txt",
        ] {
            fs::write(directory.path().join(name), "test").unwrap();
        }
        fs::create_dir(directory.path().join("mhy_nested.yaml")).unwrap();

        let files = discover_config_files(directory.path(), Some("mhy_")).unwrap();
        let names = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, ["mhy_A.YML", "mhy_b.yaml"]);
    }

    #[test]
    fn empty_directory_is_an_explicit_error() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            discover_config_files(directory.path(), None),
            Err(ConfigDirectoryError::Empty { .. })
        ));
    }

    #[test]
    fn batch_exit_code_uses_semantic_priority_and_renders_failures() {
        let mut report = BatchReport::default();
        report.push_completed("success.yaml", 0);
        report.push_failed("invalid.yaml", 2, "配置无效");
        report.push_completed("captcha.yaml", 4);
        report.push_completed("auth.yaml", 3);
        assert_eq!(report.exit_code(), 3);
        let summary = report.render_summary();
        assert!(summary.contains("success.yaml"));
        assert!(summary.contains("invalid.yaml"));
        assert!(summary.contains("auth.yaml"));
        assert!(!summary.contains("cookie_token"));
    }
}
