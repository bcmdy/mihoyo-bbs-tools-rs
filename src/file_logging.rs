use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use time::{Date, Month, OffsetDateTime};

pub struct DailyFileAppender {
    directory: PathBuf,
    stem: String,
    date: Date,
    file: File,
}

/// 日志清理结果。清理属于维护动作，任何失败只记录原因，不影响业务结果。
#[derive(Debug, Default)]
pub struct PruneReport {
    pub removed: Vec<PathBuf>,
    pub errors: Vec<String>,
}

impl DailyFileAppender {
    pub fn new(directory: &Path, prefix: &str) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let date = current_date();
        let stem = log_stem(prefix);
        let file = open_log_file(directory, &stem, date)?;
        Ok(Self {
            directory: directory.to_path_buf(),
            stem,
            date,
            file,
        })
    }

    fn rotate_if_needed(&mut self) -> io::Result<()> {
        let date = current_date();
        if date != self.date {
            self.file.flush()?;
            self.file = open_log_file(&self.directory, &self.stem, date)?;
            self.date = date;
        }
        Ok(())
    }
}

impl Write for DailyFileAppender {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.rotate_if_needed()?;
        self.file.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// 清理目录内严格匹配 `{stem}_YYYY-MM-DD.log` 的普通文件，只保留最近
/// `retention_days` 个日期。不递归、不跟随符号链接，无法解析的文件一律不动；
/// 当前及未来日期的条目（时钟回拨或手动放置）永远不会被删除。
pub fn prune(directory: &Path, stem: &str, retention_days: u64) -> PruneReport {
    let mut report = PruneReport::default();
    if retention_days == 0 {
        report
            .errors
            .push("日志保留天数必须大于 0，已跳过清理".to_owned());
        return report;
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            report
                .errors
                .push(format!("无法读取日志目录 {}：{error}", directory.display()));
            return report;
        }
    };
    let today = current_date();
    let mut dated = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report.errors.push(format!(
                    "无法读取日志目录 {} 的条目：{error}",
                    directory.display()
                ));
                continue;
            }
        };
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(date) = parse_log_date(name, stem) else {
            continue;
        };
        let path = entry.path();
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => dated.push((date, path)),
            Ok(_) => continue,
            Err(error) => {
                report.errors.push(format!(
                    "无法读取日志文件 {} 的元数据：{error}",
                    path.display()
                ));
                continue;
            }
        }
    }

    dated.sort_by(|left, right| right.0.cmp(&left.0));
    let keep_limit = usize::try_from(retention_days).unwrap_or(usize::MAX);
    for (date, path) in dated.into_iter().skip(keep_limit) {
        if date >= today {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => report.removed.push(path),
            Err(error) => report
                .errors
                .push(format!("无法删除过期日志 {}：{error}", path.display())),
        }
    }
    report
}

fn parse_log_date(name: &str, stem: &str) -> Option<Date> {
    let rest = name.strip_prefix(stem)?.strip_prefix('_')?;
    let rest = rest.strip_suffix(".log")?;
    let mut parts = rest.split('-');
    let year = parts.next()?;
    let month = parts.next()?;
    let day = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if year.len() != 4
        || month.len() != 2
        || day.len() != 2
        || !year.bytes().all(|byte| byte.is_ascii_digit())
        || !month.bytes().all(|byte| byte.is_ascii_digit())
        || !day.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let year = year.parse::<i32>().ok()?;
    let month = Month::try_from(month.parse::<u8>().ok()?).ok()?;
    let day = day.parse::<u8>().ok()?;
    Date::from_calendar_date(year, month, day).ok()
}

fn current_date() -> Date {
    OffsetDateTime::now_local()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .date()
}

fn open_log_file(directory: &Path, stem: &str, date: Date) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(log_filename(stem, date)))
}

fn log_stem(prefix: &str) -> String {
    let trimmed = prefix.trim();
    let stem = trimmed.strip_suffix(".log").unwrap_or(trimmed);
    if stem.is_empty() {
        "mihoyo-bbs-tools".to_owned()
    } else {
        stem.to_owned()
    }
}

fn log_filename(stem: &str, date: Date) -> String {
    format!("{stem}_{date}.log")
}

/// 供初始化日志时按配置前缀换算实际文件名前缀。
pub fn prefix_stem(prefix: &str) -> String {
    log_stem(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_log(directory: &Path, stem: &str, date: &str) {
        std::fs::write(directory.join(format!("{stem}_{date}.log")), b"line\n").unwrap();
    }

    #[test]
    fn filename_uses_underscore_date_and_log_suffix() {
        let date = Date::from_calendar_date(2026, time::Month::July, 13).unwrap();
        assert_eq!(
            log_filename(&log_stem("mihoyo-bbs-tools"), date),
            "mihoyo-bbs-tools_2026-07-13.log"
        );
        assert_eq!(
            log_filename(&log_stem("mihoyo-bbs-tools.log"), date),
            "mihoyo-bbs-tools_2026-07-13.log"
        );
        assert_eq!(
            log_filename(&log_stem(".log"), date),
            "mihoyo-bbs-tools_2026-07-13.log"
        );
    }

    #[test]
    fn appender_creates_expected_file() {
        let directory = tempfile::tempdir().unwrap();
        let mut appender = DailyFileAppender::new(directory.path(), "custom.log").unwrap();
        appender.write_all(b"test").unwrap();
        appender.flush().unwrap();
        let expected = directory
            .path()
            .join(log_filename("custom", current_date()));
        assert_eq!(std::fs::read(expected).unwrap(), b"test");
    }

    #[test]
    fn parse_log_date_rejects_non_canonical_names() {
        let date = Date::from_calendar_date(2026, Month::July, 13).unwrap();
        assert_eq!(parse_log_date("app_2026-07-13.log", "app"), Some(date));
        for name in [
            "app_2026-7-13.log",
            "app_2026-07-13.log.bak",
            "app-2026-07-13.log",
            "other_2026-07-13.log",
            "app_2026-13-01.log",
            "app_2026-02-30.log",
            "app_2026-07-13.txt",
            "app2026-07-13.log",
            "app_2026-07-13.log.old",
        ] {
            assert_eq!(parse_log_date(name, "app"), None, "应拒绝 {name}");
        }
    }

    #[test]
    fn prune_keeps_only_the_most_recent_days() {
        let directory = tempfile::tempdir().unwrap();
        for date in [
            "2026-01-01",
            "2026-07-10",
            "2026-07-11",
            "2026-07-12",
            "2026-07-13",
        ] {
            write_log(directory.path(), "app", date);
        }
        let report = prune(directory.path(), "app", 3);
        assert_eq!(report.removed.len(), 2);
        assert!(report.errors.is_empty());
        assert!(!directory.path().join("app_2026-01-01.log").exists());
        assert!(!directory.path().join("app_2026-07-10.log").exists());
        assert!(directory.path().join("app_2026-07-11.log").exists());
        assert!(directory.path().join("app_2026-07-12.log").exists());
        assert!(directory.path().join("app_2026-07-13.log").exists());
    }

    #[test]
    fn prune_never_removes_the_current_date_file() {
        let directory = tempfile::tempdir().unwrap();
        let today = current_date();
        std::fs::write(directory.path().join(log_filename("app", today)), b"now").unwrap();
        std::fs::write(directory.path().join("app_2099-01-01.log"), b"future").unwrap();

        // 未来日期文件占掉唯一保留名额，当前日期文件仍必须保留。
        let report = prune(directory.path(), "app", 1);
        assert!(report.removed.is_empty());
        assert!(report.errors.is_empty());
        assert!(directory.path().join(log_filename("app", today)).exists());
        assert!(directory.path().join("app_2099-01-01.log").exists());
    }

    #[test]
    fn prune_ignores_other_prefixes_directories_and_invalid_names() {
        let directory = tempfile::tempdir().unwrap();
        write_log(directory.path(), "app", "2026-01-01");
        write_log(directory.path(), "app", "2026-07-13");
        write_log(directory.path(), "other", "2026-01-01");
        std::fs::write(directory.path().join("notes.txt"), b"user").unwrap();
        std::fs::write(directory.path().join("app_2026-13-40.log"), b"bad").unwrap();
        std::fs::write(directory.path().join("app_2026-01-01.log.bak"), b"bak").unwrap();
        std::fs::create_dir(directory.path().join("app_2025-01-01.log")).unwrap();

        let report = prune(directory.path(), "app", 1);
        assert_eq!(report.removed.len(), 1);
        assert!(!directory.path().join("app_2026-01-01.log").exists());
        assert!(directory.path().join("app_2026-07-13.log").exists());
        assert!(directory.path().join("other_2026-01-01.log").exists());
        assert!(directory.path().join("notes.txt").exists());
        assert!(directory.path().join("app_2026-13-40.log").exists());
        assert!(directory.path().join("app_2026-01-01.log.bak").exists());
        assert!(directory.path().join("app_2025-01-01.log").is_dir());
    }

    #[test]
    fn prune_does_nothing_when_all_logs_fit_in_retention() {
        let directory = tempfile::tempdir().unwrap();
        write_log(directory.path(), "app", "2000-01-01");
        write_log(directory.path(), "app", "2026-07-13");
        let report = prune(directory.path(), "app", 5);
        assert!(report.removed.is_empty());
        assert!(directory.path().join("app_2000-01-01.log").exists());
    }

    #[test]
    fn prune_reports_reason_when_directory_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("absent");
        let report = prune(&missing, "app", 1);
        assert!(report.removed.is_empty());
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains("无法读取日志目录"));
    }

    #[test]
    fn zero_retention_days_is_rejected_without_deleting_anything() {
        let directory = tempfile::tempdir().unwrap();
        write_log(directory.path(), "app", "2000-01-01");
        let report = prune(directory.path(), "app", 0);
        assert!(report.removed.is_empty());
        assert_eq!(report.errors.len(), 1);
        assert!(directory.path().join("app_2000-01-01.log").exists());
    }

    #[test]
    fn prefix_stem_matches_appender_naming() {
        assert_eq!(prefix_stem("mihoyo-bbs-tools"), "mihoyo-bbs-tools");
        assert_eq!(prefix_stem("mihoyo-bbs-tools.log"), "mihoyo-bbs-tools");
        assert_eq!(prefix_stem("  "), "mihoyo-bbs-tools");
    }

    #[cfg(unix)]
    #[test]
    fn prune_reports_reason_when_directory_is_not_writable() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        write_log(directory.path(), "app", "2000-01-01");
        write_log(directory.path(), "app", "2026-07-13");
        let original = std::fs::metadata(directory.path()).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_mode(0o555);
        std::fs::set_permissions(directory.path(), readonly).unwrap();

        let probe = directory.path().join("probe");
        if std::fs::write(&probe, b"").is_ok() {
            let _ = std::fs::remove_file(&probe);
            std::fs::set_permissions(directory.path(), original).unwrap();
            return;
        }

        let report = prune(directory.path(), "app", 1);
        std::fs::set_permissions(directory.path(), original).unwrap();
        assert!(report.removed.is_empty());
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains("无法删除过期日志"));
        assert!(directory.path().join("app_2000-01-01.log").exists());
    }
}
