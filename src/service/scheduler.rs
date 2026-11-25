use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use crate::config::{ConfigError, LoadedConfig, ScheduleConfig};

/// 常驻调度等待期间检查配置变化的默认周期。
pub const DEFAULT_SCHEDULE_POLL_SECONDS: u64 = 30;

/// 运行中配置重载失败时的退避间隔（秒），超出后沿用最后一项。
pub const SCHEDULE_RELOAD_BACKOFF_SECONDS: [u64; 3] = [30, 60, 120];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitSignal {
    /// 等待到期。
    Elapsed,
    /// 收到 Ctrl+C 或 Unix 终止信号。
    Cancelled,
}

#[derive(Debug)]
pub enum ScheduleWait {
    /// 已到达下一轮开始时间，且配置有效、定时运行仍启用。
    ///
    /// 配置整体较大，装箱以避免枚举各分支尺寸差异过大。
    Ready(Box<LoadedConfig>),
    /// 配置中 `schedule.enabled=false`，应立即退出。
    Disabled,
    /// 收到退出信号，应在轮次边界停止。
    Cancelled,
}

/// 退出信号接收端；由 [`shutdown_channel`] 创建，可 `clone` 给多个等待者。
#[derive(Clone, Debug)]
pub struct ShutdownSignal {
    receiver: watch::Receiver<bool>,
}

impl ShutdownSignal {
    /// 立即判断是否已请求退出。
    pub fn is_requested(&self) -> bool {
        *self.receiver.borrow()
    }

    /// 等待退出请求；已请求时立即完成。
    async fn requested(&mut self) {
        // 发送端只在本进程退出路径上存在，通道关闭同样视为退出。
        let _ = self.receiver.wait_for(|requested| *requested).await;
    }
}

/// 创建退出信号通道；返回的发送端交给 [`spawn_shutdown_listener`]。
pub fn shutdown_channel() -> (watch::Sender<bool>, ShutdownSignal) {
    let (sender, receiver) = watch::channel(false);
    (sender, ShutdownSignal { receiver })
}

/// 在后台监听 Ctrl+C 与 SIGTERM，收到后向通道广播退出请求。
///
/// 后台任务只置位信号，不打断正在执行的任务；等待方在轮次边界退出，
/// 从而保证文件日志在 `main` 正常返回时被刷新。
pub fn spawn_shutdown_listener(sender: watch::Sender<bool>) {
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("已收到退出信号，将在当前轮次结束或等待中断时停止");
        let _ = sender.send(true);
    });
}

/// 等待一个调度间隔，并在等待期间按 `poll` 周期复查配置开关与退出信号。
///
/// 到达下一轮时间后必须重新读取到有效配置才会返回 `Ready`；运行中配置
/// 暂时无效时按 `reload_backoff` 限频重读，期间不启动任何新任务。
/// 退出信号只在等待期与轮次边界生效，绝不中断已经开始的任务提交。
pub async fn wait_for_next_round<F>(
    schedule: &ScheduleConfig,
    reload_backoff: &[Duration],
    shutdown: &mut ShutdownSignal,
    mut load: F,
) -> ScheduleWait
where
    F: FnMut() -> Result<LoadedConfig, ConfigError>,
{
    let poll = schedule_poll_interval(schedule.poll_seconds);
    if shutdown.is_requested() {
        return ScheduleWait::Cancelled;
    }
    let deadline = Instant::now() + schedule_interval(schedule);
    while !deadline.saturating_duration_since(Instant::now()).is_zero() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if wait_until(remaining.min(poll), shutdown).await == WaitSignal::Cancelled {
            return ScheduleWait::Cancelled;
        }
        // 等待期间只关心开关状态与退出信号；配置短暂无效不应终止进程。
        match load() {
            Ok(loaded) if !loaded.config.runtime.schedule.enabled => {
                return ScheduleWait::Disabled;
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(
                %error,
                "定时运行等待期间重新加载配置失败，将继续等待并在下一轮开始前重试"
            ),
        }
    }

    let fallback_backoff = Duration::from_secs(DEFAULT_SCHEDULE_POLL_SECONDS);
    let last_index = reload_backoff.len().saturating_sub(1);
    let mut attempt = 0_usize;
    loop {
        match load() {
            Ok(loaded) => {
                return if loaded.config.runtime.schedule.enabled {
                    ScheduleWait::Ready(Box::new(loaded))
                } else {
                    ScheduleWait::Disabled
                };
            }
            Err(error) => {
                let backoff = reload_backoff
                    .get(attempt.min(last_index))
                    .copied()
                    .unwrap_or(fallback_backoff);
                attempt = attempt.saturating_add(1);
                tracing::warn!(
                    %error,
                    retry_after_seconds = backoff.as_secs(),
                    "运行中重新加载配置失败，暂停启动新任务并按退避间隔重读"
                );
                // 退避期间分段等待，总时长不变但能及时响应退出信号。
                let mut remaining_backoff = backoff;
                while !remaining_backoff.is_zero() {
                    // 兜底保证步长大于 0，避免退避值过小时死循环。
                    let chunk = remaining_backoff.min(poll).max(Duration::from_millis(1));
                    if wait_until(chunk, shutdown).await == WaitSignal::Cancelled {
                        return ScheduleWait::Cancelled;
                    }
                    remaining_backoff = remaining_backoff.saturating_sub(chunk);
                }
            }
        }
    }
}

/// 可取消的一次等待，到期返回 `Elapsed`，收到退出信号返回 `Cancelled`。
pub async fn wait_with_cancel(duration: Duration, shutdown: &mut ShutdownSignal) -> WaitSignal {
    wait_until(duration, shutdown).await
}

async fn wait_until(duration: Duration, shutdown: &mut ShutdownSignal) -> WaitSignal {
    if shutdown.is_requested() {
        return WaitSignal::Cancelled;
    }
    tokio::select! {
        () = tokio::time::sleep(duration) => WaitSignal::Elapsed,
        () = shutdown.requested() => WaitSignal::Cancelled,
    }
}

/// 监听 Ctrl+C；Unix 额外监听 SIGTERM。Windows 控制台强制终止不触发该处理。
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "无法注册 Ctrl+C 监听，常驻调度将继续运行");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    {
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(error) => {
                    tracing::warn!(%error, "无法注册 SIGTERM 监听");
                    std::future::pending::<()>().await;
                }
            }
        };
        tokio::select! {
            () = ctrl_c => {}
            () = terminate => {}
        }
    }
    #[cfg(not(unix))]
    {
        ctrl_c.await;
    }
}

pub fn schedule_poll_interval(poll_seconds: Option<u64>) -> Duration {
    Duration::from_secs(
        poll_seconds
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_SCHEDULE_POLL_SECONDS),
    )
}

pub fn schedule_interval(schedule: &ScheduleConfig) -> Duration {
    Duration::from_secs(schedule.interval_minutes.saturating_mul(60))
}

#[cfg(test)]
mod tests {
    use std::time::Instant as StdInstant;

    use super::*;
    use crate::config::{
        CURRENT_CONFIG_VERSION, CaptchaConfig, Config, ConfigSource, NotificationsConfig,
        RuntimeConfig,
    };

    fn schedule(interval_minutes: u64, poll_seconds: Option<u64>) -> ScheduleConfig {
        ScheduleConfig {
            enabled: true,
            interval_minutes,
            run_on_start: true,
            poll_seconds,
        }
    }

    fn loaded(enabled: bool) -> LoadedConfig {
        let mut config = Config {
            version: CURRENT_CONFIG_VERSION,
            runtime: RuntimeConfig::default(),
            captcha: CaptchaConfig::default(),
            accounts: Vec::new(),
            notifications: NotificationsConfig::default(),
        };
        config.runtime.schedule.enabled = enabled;
        LoadedConfig {
            config,
            warnings: Vec::new(),
            source: ConfigSource::Current,
        }
    }

    fn invalid_yaml_error() -> ConfigError {
        ConfigError::Yaml(
            serde_yaml_ng::from_str::<serde_yaml_ng::Value>("a: [")
                .expect_err("测试用非法 YAML 必须解析失败"),
        )
    }

    fn short_backoff() -> [Duration; 3] {
        [
            Duration::from_millis(1),
            Duration::from_millis(2),
            Duration::from_millis(4),
        ]
    }

    #[test]
    fn interval_is_converted_to_seconds() {
        assert_eq!(
            schedule_interval(&schedule(720, None)),
            Duration::from_secs(43_200)
        );
        assert_eq!(schedule_interval(&schedule(0, None)), Duration::ZERO);
    }

    #[test]
    fn poll_interval_defaults_and_rejects_zero() {
        assert_eq!(
            schedule_poll_interval(None),
            Duration::from_secs(DEFAULT_SCHEDULE_POLL_SECONDS)
        );
        assert_eq!(schedule_poll_interval(Some(5)), Duration::from_secs(5));
        assert_eq!(
            schedule_poll_interval(Some(0)),
            Duration::from_secs(DEFAULT_SCHEDULE_POLL_SECONDS)
        );
    }

    #[tokio::test]
    async fn cancellable_wait_returns_elapsed_immediately_for_zero_duration() {
        let (_sender, mut shutdown) = shutdown_channel();
        assert_eq!(
            wait_with_cancel(Duration::ZERO, &mut shutdown).await,
            WaitSignal::Elapsed
        );
    }

    #[tokio::test]
    async fn cancellable_wait_returns_elapsed_after_short_duration() {
        let (_sender, mut shutdown) = shutdown_channel();
        let started = StdInstant::now();
        assert_eq!(
            wait_with_cancel(Duration::from_millis(30), &mut shutdown).await,
            WaitSignal::Elapsed
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn cancellable_wait_stops_as_soon_as_shutdown_is_requested() {
        let (sender, mut shutdown) = shutdown_channel();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let _ = sender.send(true);
        });
        let started = StdInstant::now();
        assert_eq!(
            wait_with_cancel(Duration::from_secs(600), &mut shutdown).await,
            WaitSignal::Cancelled
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "退出信号应立刻中断等待，而不是等满一个调度间隔"
        );
    }

    #[tokio::test]
    async fn wait_for_next_round_returns_cancelled_when_shutdown_is_requested() {
        let (sender, mut shutdown) = shutdown_channel();
        let _ = sender.send(true);
        let started = StdInstant::now();
        let wait = wait_for_next_round(
            &schedule(720, Some(1)),
            &short_backoff(),
            &mut shutdown,
            || Ok(loaded(true)),
        )
        .await;
        assert!(matches!(wait, ScheduleWait::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn wait_for_next_round_exits_quickly_when_schedule_is_disabled() {
        let (_sender, mut shutdown) = shutdown_channel();
        let started = StdInstant::now();
        let wait = wait_for_next_round(
            &schedule(0, Some(1)),
            &short_backoff(),
            &mut shutdown,
            || Ok(loaded(false)),
        )
        .await;
        assert!(matches!(wait, ScheduleWait::Disabled));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn wait_for_next_round_recovers_after_reload_failures() {
        let (_sender, mut shutdown) = shutdown_channel();
        let started = StdInstant::now();
        let mut calls = 0_u32;
        let wait = wait_for_next_round(
            &schedule(0, Some(1)),
            &short_backoff(),
            &mut shutdown,
            || {
                calls += 1;
                if calls <= 2 {
                    Err(invalid_yaml_error())
                } else {
                    Ok(loaded(true))
                }
            },
        )
        .await;
        match wait {
            ScheduleWait::Ready(loaded) => assert!(loaded.config.runtime.schedule.enabled),
            other => panic!("应在重载恢复后返回 Ready，实际为 {other:?}"),
        }
        assert_eq!(calls, 3);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn wait_for_next_round_keeps_retrying_when_config_stays_invalid() {
        let (_sender, mut shutdown) = shutdown_channel();
        let started = StdInstant::now();
        let mut calls = 0_u32;
        let wait = wait_for_next_round(
            &schedule(0, Some(1)),
            &short_backoff(),
            &mut shutdown,
            || {
                calls += 1;
                if calls < 4 {
                    Err(invalid_yaml_error())
                } else {
                    Ok(loaded(true))
                }
            },
        )
        .await;
        assert!(matches!(wait, ScheduleWait::Ready(_)));
        assert_eq!(calls, 4);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn wait_for_next_round_stops_when_schedule_disabled_after_reload_failure() {
        let (_sender, mut shutdown) = shutdown_channel();
        let started = StdInstant::now();
        let mut calls = 0_u32;
        let wait = wait_for_next_round(
            &schedule(0, Some(1)),
            &short_backoff(),
            &mut shutdown,
            || {
                calls += 1;
                if calls == 1 {
                    Err(invalid_yaml_error())
                } else {
                    Ok(loaded(false))
                }
            },
        )
        .await;
        assert!(matches!(wait, ScheduleWait::Disabled));
        assert_eq!(calls, 2);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
