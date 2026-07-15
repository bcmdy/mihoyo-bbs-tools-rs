mod batch;
mod bbs_runner;
mod cloud_runner;
mod credential_refresh;
mod qinglong;
mod report;
mod runner;
mod runtime_delay;
mod scheduler;
mod web_activity_runner;

use std::path::Path;
use uuid::Uuid;

pub use batch::{BatchEntry, BatchReport, ConfigDirectoryError, discover_config_files};
pub use bbs_runner::{run_bbs, run_bbs_with_persistence, run_bbs_with_refresh};
pub use cloud_runner::run_cloud_games;
pub use qinglong::{QinglongError, QinglongSettings, qinglong_settings};
pub use report::{RunReport, TaskOutcome, TaskRecord};
pub use runner::{
    run_china_checkin, run_china_checkin_with_persistence, run_china_checkin_with_refresh,
    run_hoyolab_checkin,
};
pub use runtime_delay::apply_runtime_delay;
pub use scheduler::wait_schedule_interval;
pub use web_activity_runner::run_web_activities;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialPersistence<'a> {
    ReadOnly,
    CurrentConfig(&'a Path),
}

impl CredentialPersistence<'_> {
    fn path(self) -> Option<&Path> {
        match self {
            Self::ReadOnly => None,
            Self::CurrentConfig(path) => Some(path),
        }
    }
}

fn resolve_device_id(configured: &str, cookie: &str) -> String {
    if configured.is_empty() {
        Uuid::new_v3(&Uuid::NAMESPACE_URL, cookie.as_bytes()).to_string()
    } else {
        configured.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_device_id_takes_priority_over_cookie_derived_id() {
        assert_eq!(
            resolve_device_id("fixed-device-id", "cookie_token=secret"),
            "fixed-device-id"
        );
    }

    #[test]
    fn empty_device_id_is_stably_derived_from_cookie() {
        let expected =
            Uuid::new_v3(&Uuid::NAMESPACE_URL, b"account_id=123; cookie_token=secret").to_string();
        assert_eq!(
            resolve_device_id("", "account_id=123; cookie_token=secret"),
            expected
        );
    }

    #[test]
    fn credential_persistence_exposes_only_current_config_path() {
        let path = Path::new("config.yaml");
        assert_eq!(CredentialPersistence::ReadOnly.path(), None);
        assert_eq!(
            CredentialPersistence::CurrentConfig(path).path(),
            Some(path)
        );
    }
}
