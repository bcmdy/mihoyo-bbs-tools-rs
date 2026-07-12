mod bbs_runner;
mod report;
mod runner;

pub use bbs_runner::run_bbs;
pub use report::{RunReport, TaskOutcome, TaskRecord};
pub use runner::{run_china_checkin, run_hoyolab_checkin};
