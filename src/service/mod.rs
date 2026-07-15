mod bbs_runner;
mod cloud_runner;
mod credential_refresh;
mod report;
mod runner;
mod runtime_delay;
mod scheduler;
mod web_activity_runner;

use uuid::Uuid;

pub use bbs_runner::{run_bbs, run_bbs_with_refresh};
pub use cloud_runner::run_cloud_games;
pub use report::{RunReport, TaskOutcome, TaskRecord};
pub use runner::{run_china_checkin, run_china_checkin_with_refresh, run_hoyolab_checkin};
pub use runtime_delay::apply_runtime_delay;
pub use scheduler::wait_schedule_interval;
pub use web_activity_runner::run_web_activities;

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
}
