//! 统一的 HTTP 客户端工厂。
//!
//! 原先 runner、bbs_runner、cloud_runner 与 push 各自内联超时、重试与代理配置，
//! 任何一处调整都要重复修改，且容易让相同配置在不同模块语义漂移。工厂把这些规则
//! 收敛到一处：
//!
//! - 超时：`runtime.request_timeout_seconds`
//! - 重试：`RetryPolicy::for_extra_retries(runtime.retry_count)`
//! - 代理：账号级或渠道级代理配置（凭据仍由调用方以 [`SecretString`] 持有）
//!
//! 本次只做“提取”，不引入客户端缓存与连接池复用：复用需要基准测量，留待后续。
//! 为了让将来按“账号 + 代理”缓存有清晰边界，工厂把代理配置作为独立参数暴露，
//! 且**不**把 Cookie、UA、DS 等请求级数据固化进客户端。

use crate::{
    auth::SecretString,
    config::{AccountConfig, Config, NotificationProvider},
    http::{HttpClient, HttpError, RetryPolicy},
};

/// 账号相关客户端：国内游戏、HoYoLAB、BBS 与云游戏共用。
///
/// 代理来自账号配置；Cookie/UA/DS 仍按请求设置，不进入客户端。
pub fn for_account(config: &Config, account: &AccountConfig) -> Result<HttpClient, HttpError> {
    let proxy = account.proxy.url.as_ref().map(SecretString::expose_secret);
    build(config, proxy)
}

/// 通知渠道客户端：代理只来自 Telegram 渠道配置。
///
/// 其它渠道不使用代理，保持既有行为。
pub fn for_provider(
    config: &Config,
    provider: &NotificationProvider,
) -> Result<HttpClient, HttpError> {
    let proxy = match provider {
        NotificationProvider::Telegram { proxy, .. } => {
            proxy.as_ref().map(SecretString::expose_secret)
        }
        _ => None,
    };
    build(config, proxy)
}

/// 更新检查客户端：不读取用户代理配置，采用固定重试次数。
///
/// `update` 命令在配置加载前也可能被调用，因此不接受 `Config`。
pub fn for_update() -> Result<HttpClient, HttpError> {
    HttpClient::builder()
        .retry(RetryPolicy::for_extra_retries(
            RetryPolicy::FIXED_EXTRA_RETRIES,
        ))
        .build()
}

/// 昵称查询等配置编辑场景的客户端：不读取代理与超时配置，固定重试次数。
pub fn for_config_lookup() -> Result<HttpClient, HttpError> {
    for_update()
}

/// 诊断探测客户端：只发送一次，避免与业务重试叠加。
pub fn for_diagnostic(config: &Config, proxy: Option<&str>) -> Result<HttpClient, HttpError> {
    HttpClient::builder()
        .timeout(std::time::Duration::from_secs(
            config.runtime.request_timeout_seconds,
        ))
        .retry(RetryPolicy::with_attempts(1))
        .proxy(proxy)?
        .build()
}

/// 统一构建：超时与重试取自 `runtime`，代理作为独立参数。
///
/// 参数中只出现“连接配置”，不包含账号 Cookie、User-Agent 或签名数据；后续若按
/// “账号 + 代理”缓存客户端，键可以直接用 `(account.name, proxy)`。
fn build(config: &Config, proxy: Option<&str>) -> Result<HttpClient, HttpError> {
    HttpClient::builder()
        .timeout(std::time::Duration::from_secs(
            config.runtime.request_timeout_seconds,
        ))
        .retry(RetryPolicy::for_extra_retries(
            usize::try_from(config.runtime.retry_count).unwrap_or(usize::MAX),
        ))
        .proxy(proxy)?
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(retry_count: u32, timeout_seconds: u64) -> Config {
        let source = format!(
            "version: 1\naccounts: []\nruntime:\n  retry_count: {retry_count}\n  request_timeout_seconds: {timeout_seconds}\n"
        );
        serde_yaml_ng::from_str(&source).unwrap()
    }

    fn account_with(proxy: Option<&str>) -> AccountConfig {
        let proxy_line = proxy
            .map(|value| format!("    proxy:\n      url: '{value}'\n"))
            .unwrap_or_default();
        let source = format!(
            "version: 1\naccounts:\n  - name: first\n{proxy_line}    credentials:\n      cookie: 'cookie_token=secret'\n      stoken: 'stoken-secret'\n"
        );
        let config: Config = serde_yaml_ng::from_str(&source).unwrap();
        config.accounts.into_iter().next().unwrap()
    }

    #[test]
    fn account_clients_take_timeout_and_retry_from_config() {
        let config = config_with(0, 12);
        let client = for_account(&config, &account_with(None)).unwrap();
        assert_eq!(client.retry_policy().attempts, 1);
        assert_eq!(client.retry_policy().base_delay.as_millis(), 500);

        let config = config_with(3, 12);
        let client = for_account(&config, &account_with(None)).unwrap();
        // 额外重试 3 次 = 首次 + 3 次重试。
        assert_eq!(client.retry_policy().attempts, 4);
        assert_eq!(client.backoff().max_total_delay.as_secs(), 30);
    }

    #[test]
    fn account_clients_accept_valid_proxy_and_reject_invalid_one() {
        let config = config_with(0, 12);
        assert!(for_account(&config, &account_with(None)).is_ok());
        assert!(for_account(&config, &account_with(Some("socks5h://127.0.0.1:1080"))).is_ok());
        assert!(matches!(
            for_account(&config, &account_with(Some("file:///tmp/proxy"))),
            Err(HttpError::InvalidProxy(_))
        ));
    }

    #[test]
    fn provider_clients_only_use_telegram_proxy() {
        let config = config_with(1, 15);
        let telegram = NotificationProvider::Telegram {
            bot_token: SecretString::new("bot-secret"),
            chat_id: "123456".to_owned(),
            api_url: "https://api.telegram.org".parse().unwrap(),
            proxy: Some(SecretString::new("socks5h://127.0.0.1:1080")),
        };
        assert!(for_provider(&config, &telegram).is_ok());

        let webhook = NotificationProvider::Webhook {
            url: SecretString::new("https://example.com/hook"),
        };
        let client = for_provider(&config, &webhook).unwrap();
        assert_eq!(client.retry_policy().attempts, 2);
    }

    #[test]
    fn provider_client_reports_invalid_telegram_proxy() {
        let config = config_with(0, 15);
        let telegram = NotificationProvider::Telegram {
            bot_token: SecretString::new("bot-secret"),
            chat_id: "123456".to_owned(),
            api_url: "https://api.telegram.org".parse().unwrap(),
            proxy: Some(SecretString::new("file:///tmp/proxy")),
        };
        assert!(matches!(
            for_provider(&config, &telegram),
            Err(HttpError::InvalidProxy(_))
        ));
    }

    #[test]
    fn account_clients_are_built_per_proxy_without_caching() {
        // 本次只做提取、不做缓存：不同代理账号分别构建客户端；未来缓存键必须包含代理。
        let config = config_with(2, 20);
        let direct = for_account(&config, &account_with(None)).unwrap();
        let proxied =
            for_account(&config, &account_with(Some("socks5h://127.0.0.1:1080"))).unwrap();
        // 同一份配置派生出相同策略，但两者各自独立构建，不共享运行期客户端。
        assert_eq!(direct.retry_policy(), proxied.retry_policy());
        assert_eq!(direct.backoff(), proxied.backoff());
        assert!(for_account(&config, &account_with(None)).is_ok());
    }

    #[test]
    fn update_and_lookup_clients_use_fixed_retries() {
        let client = for_update().unwrap();
        assert_eq!(
            client.retry_policy().attempts,
            RetryPolicy::for_extra_retries(RetryPolicy::FIXED_EXTRA_RETRIES).attempts
        );
        let lookup = for_config_lookup().unwrap();
        assert_eq!(lookup.retry_policy(), client.retry_policy());
    }

    #[test]
    fn diagnostic_client_sends_single_attempt() {
        let config = config_with(5, 9);
        let client = for_diagnostic(&config, None).unwrap();
        assert_eq!(client.retry_policy().attempts, 1);
        assert!(matches!(
            for_diagnostic(&config, Some("file:///tmp/proxy")),
            Err(HttpError::InvalidProxy(_))
        ));
    }
}
