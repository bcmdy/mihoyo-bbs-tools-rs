use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use time::{Date, OffsetDateTime};

pub struct DailyFileAppender {
    directory: PathBuf,
    stem: String,
    date: Date,
    file: File,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
