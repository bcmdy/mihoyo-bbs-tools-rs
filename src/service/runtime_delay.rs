use std::time::Duration;

use rand::Rng;

use crate::config::RuntimeConfig;

pub async fn apply_runtime_delay(runtime: &RuntimeConfig) -> u64 {
    let seconds = choose_delay(runtime.random_delay_seconds);
    if seconds > 0 {
        tracing::info!(seconds, "任务开始前应用随机延迟");
        tokio::time::sleep(Duration::from_secs(seconds)).await;
    }
    seconds
}

fn choose_delay(max_seconds: u64) -> u64 {
    if max_seconds == 0 {
        0
    } else {
        rand::rng().random_range(0..=max_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_delay_is_always_zero() {
        assert_eq!(choose_delay(0), 0);
    }

    #[test]
    fn random_delay_never_exceeds_configured_maximum() {
        for _ in 0..100 {
            assert!(choose_delay(10) <= 10);
        }
    }
}
