use std::time::Duration;

use crate::config::ScheduleConfig;

pub async fn wait_schedule_interval(schedule: &ScheduleConfig) {
    let duration = schedule_interval(schedule);
    tracing::info!(minutes = schedule.interval_minutes, "定时运行等待下一轮");
    tokio::time::sleep(duration).await;
}

fn schedule_interval(schedule: &ScheduleConfig) -> Duration {
    Duration::from_secs(schedule.interval_minutes.saturating_mul(60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_is_converted_to_seconds() {
        let schedule = ScheduleConfig {
            enabled: true,
            interval_minutes: 720,
            run_on_start: true,
        };
        assert_eq!(schedule_interval(&schedule), Duration::from_secs(43_200));
    }
}
