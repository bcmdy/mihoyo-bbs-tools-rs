use std::time::Duration;

use reqwest::{
    Method, Proxy, StatusCode, Url,
    header::{HeaderMap, RETRY_AFTER},
};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

use super::error::{describe_json_error, describe_proxy, sanitize_reqwest_error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub attempts: usize,
    pub base_delay: Duration,
}

impl RetryPolicy {
    /// 由文档语义的“额外重试次数”构造策略：`attempts = extra + 1`。
    ///
    /// `runtime.retry_count` 定义为额外重试次数（`0` 表示不额外重试），而 HTTP 层
    /// 配置的是**总尝试次数**。所有消费点都必须经本构造函数换算，避免同一配置在
    /// 游戏签到、BBS 与推送鉴权之间含义不同。
    pub const fn for_extra_retries(extra: usize) -> Self {
        Self {
            attempts: extra.saturating_add(1),
            base_delay: Duration::from_millis(500),
        }
    }

    /// 显式指定总尝试次数，基座延迟沿用默认值。
    pub const fn with_attempts(attempts: usize) -> Self {
        Self {
            attempts,
            base_delay: Duration::from_millis(500),
        }
    }

    /// 无配置上下文时沿用的固定“额外重试次数”。
    ///
    /// 等于历史默认的总尝试 3 次（首次 + 额外 2 次）。`update` 与昵称查询不读取
    /// 用户配置，通过该常量走统一构造函数，保证“额外重试”语义一致且行为不变。
    pub const FIXED_EXTRA_RETRIES: usize = 2;
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base_delay: Duration::from_millis(500),
        }
    }
}

/// 只读请求的重试退避参数。
///
/// 与服务端 `Retry-After` 提示、抖动和整次调用的等待预算配合，避免 429/503 时
/// 盲目重试或长时间阻塞。默认值保证既有的尝试次数语义不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// 单次重试等待的上限，服务器 `Retry-After` 也会被裁剪到这个值。
    pub max_retry_after: Duration,
    /// 一次调用累计允许的等待预算，耗尽后不再重试。
    pub max_total_delay: Duration,
    /// 是否给等待加入抖动，避免多实例同时重试。
    pub jitter: bool,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            max_retry_after: Duration::from_secs(10),
            max_total_delay: Duration::from_secs(30),
            jitter: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("无效 URL：{0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("代理配置无效：{0}")]
    InvalidProxy(String),
    #[error("请求超时")]
    Timeout,
    #[error("连接失败：{0}")]
    Connect(String),
    #[error("服务器返回 HTTP {0}")]
    Status(StatusCode),
    #[error("响应 JSON 无效：{0}")]
    Decode(String),
    #[error("HTTP 客户端初始化失败：{0}")]
    Build(String),
}

#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
    retry: RetryPolicy,
    backoff: Backoff,
}

/// 手写 Debug：不打印 reqwest 客户端内部配置（其 Debug 会输出代理规则），只暴露重试策略。
impl std::fmt::Debug for HttpClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpClient")
            .field("retry", &self.retry)
            .field("backoff", &self.backoff)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct HttpClientBuilder {
    timeout: Duration,
    retry: RetryPolicy,
    backoff: Backoff,
    proxy: Option<Url>,
    user_agent: String,
}

/// 手写 Debug：代理地址可能包含用户名与密码，只输出协议、主机与端口。
impl std::fmt::Debug for HttpClientBuilder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpClientBuilder")
            .field("timeout", &self.timeout)
            .field("retry", &self.retry)
            .field("backoff", &self.backoff)
            .field("proxy", &self.proxy.as_ref().map(describe_proxy))
            .field("user_agent", &self.user_agent)
            .finish()
    }
}

impl Default for HttpClientBuilder {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            retry: RetryPolicy::default(),
            backoff: Backoff::default(),
            proxy: None,
            user_agent: format!("MihoyoBBSToolsRS/{}", crate::VERSION),
        }
    }
}

impl HttpClientBuilder {
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// 覆盖只读请求的退避参数（`Retry-After` 上限、总等待预算与抖动）。
    pub fn backoff(mut self, backoff: Backoff) -> Self {
        self.backoff = backoff;
        self
    }

    pub fn proxy(mut self, proxy: Option<&str>) -> Result<Self, HttpError> {
        self.proxy = proxy.map(normalize_proxy_url).transpose()?;
        Ok(self)
    }

    pub fn build(self) -> Result<HttpClient, HttpError> {
        // 库测试和外部调用不会经过二进制入口；重复安装只表示已有同一进程级 provider。
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut builder = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(self.user_agent);
        if let Some(proxy) = self.proxy {
            builder = builder.proxy(
                Proxy::all(proxy.as_str())
                    .map_err(|error| HttpError::InvalidProxy(sanitize_reqwest_error(error)))?,
            );
        }
        let inner = builder
            .build()
            .map_err(|error| HttpError::Build(sanitize_reqwest_error(error)))?;
        Ok(HttpClient {
            inner,
            retry: self.retry,
            backoff: self.backoff,
        })
    }
}

impl HttpClient {
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// 覆盖只读请求的退避参数。
    pub fn with_backoff(mut self, backoff: Backoff) -> Self {
        self.backoff = backoff;
        self
    }

    pub fn retry_policy(&self) -> RetryPolicy {
        self.retry
    }

    pub fn backoff(&self) -> Backoff {
        self.backoff
    }

    pub async fn get_json<T>(&self, url: Url) -> Result<T, HttpError>
    where
        T: DeserializeOwned,
    {
        self.get_json_with(url, HeaderMap::new(), &[]).await
    }

    pub async fn get_json_with<T>(
        &self,
        url: Url,
        headers: HeaderMap,
        query: &[(&str, String)],
    ) -> Result<T, HttpError>
    where
        T: DeserializeOwned,
    {
        let attempts = self.retry.attempts.max(1);
        let mut spent_delay = Duration::ZERO;
        for attempt in 0..attempts {
            let last_attempt = attempt + 1 >= attempts;
            let decision = match self
                .inner
                .request(Method::GET, url.clone())
                .headers(headers.clone())
                .query(query)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    match response.bytes().await {
                        Ok(body) => {
                            // JSON 语法错误不重试：重试无法修复服务端返回的内容。
                            return serde_json::from_slice(&body)
                                .map_err(|error| HttpError::Decode(describe_json_error(&error)));
                        }
                        Err(error) => {
                            match body_failure_action(classify_body_failure(&error), last_attempt) {
                                BodyFailureAction::Retry => {
                                    // 响应头成功但响应体读取超时属于只读请求的可重试失败，
                                    // 预算耗尽后仍保留 Timeout 分类。
                                    RetryDecision::wait(HttpError::Timeout)
                                }
                                BodyFailureAction::ReturnTimeout => {
                                    return Err(HttpError::Timeout);
                                }
                                // 传输中断等 body 错误按解码失败保留（非语法错误，不重试）。
                                BodyFailureAction::ReturnDecode => {
                                    return Err(HttpError::Decode(sanitize_reqwest_error(error)));
                                }
                            }
                        }
                    }
                }
                Ok(response) if should_retry_status(response.status()) && !last_attempt => {
                    RetryDecision {
                        hint: retry_after_hint(response.headers(), self.backoff.max_retry_after),
                        exhausted: HttpError::Status(response.status()),
                    }
                }
                Ok(response) => return Err(HttpError::Status(response.status())),
                Err(error) if is_retryable(&error) && !last_attempt => {
                    let exhausted = if error.is_timeout() {
                        HttpError::Timeout
                    } else {
                        HttpError::Connect(sanitize_reqwest_error(error))
                    };
                    RetryDecision::wait(exhausted)
                }
                Err(error) if error.is_timeout() => return Err(HttpError::Timeout),
                Err(error) => return Err(HttpError::Connect(sanitize_reqwest_error(error))),
            };
            let delay = retry_delay(
                self.retry.base_delay,
                attempt,
                decision.hint,
                self.backoff.jitter,
            );
            // 预算耗尽后不再等待，也不再发送额外请求；按真实失败类别返回。
            if !fits_budget(spent_delay, delay, self.backoff.max_total_delay) {
                return Err(decision.exhausted);
            }
            spent_delay += delay;
            tokio::time::sleep(delay).await;
        }
        Err(HttpError::Connect("重试次数已耗尽".to_owned()))
    }

    /// 对可能产生外部副作用的 GET 只发送一次，并解析 JSON 响应。
    ///
    /// 少数第三方接口使用 GET 记录阅读或分享等业务动作；这些请求不能套用通用网络重试，
    /// 是否再次执行必须由业务层在状态复查后决定。
    pub async fn get_json_once_with<T>(
        &self,
        url: Url,
        headers: HeaderMap,
        query: &[(&str, String)],
    ) -> Result<T, HttpError>
    where
        T: DeserializeOwned,
    {
        match self
            .inner
            .request(Method::GET, url)
            .headers(headers)
            .query(query)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => response
                .json()
                .await
                .map_err(|error| HttpError::Decode(sanitize_reqwest_error(error))),
            Ok(response) => Err(HttpError::Status(response.status())),
            Err(error) if error.is_timeout() => Err(HttpError::Timeout),
            Err(error) => Err(HttpError::Connect(sanitize_reqwest_error(error))),
        }
    }

    /// 对可能产生外部副作用的 POST 只发送一次，由业务层决定是否可以再次尝试。
    pub async fn post_json_once<B, T>(
        &self,
        url: Url,
        headers: HeaderMap,
        body: &B,
    ) -> Result<T, HttpError>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        match self
            .inner
            .request(Method::POST, url)
            .headers(headers)
            .json(body)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => response
                .json()
                .await
                .map_err(|error| HttpError::Decode(sanitize_reqwest_error(error))),
            Ok(response) => Err(HttpError::Status(response.status())),
            Err(error) if error.is_timeout() => Err(HttpError::Timeout),
            Err(error) => Err(HttpError::Connect(sanitize_reqwest_error(error))),
        }
    }

    /// 发送一次原始字节 POST，并解析 JSON 响应。
    ///
    /// 与 [`Self::post_json_once`] 的区别是**不重新序列化请求体**：通行证换票等接口
    /// 的签名基于实际发送的字节，用结构化 JSON 重新编码可能改变空白与字段顺序而
    /// 导致验签失败。调用方必须自行设置 `Content-Type`。
    pub async fn post_bytes_once<T>(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> Result<T, HttpError>
    where
        T: DeserializeOwned,
    {
        match self
            .inner
            .request(Method::POST, url)
            .headers(headers)
            .body(body)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => response
                .json()
                .await
                .map_err(|error| HttpError::Decode(sanitize_reqwest_error(error))),
            Ok(response) => Err(HttpError::Status(response.status())),
            Err(error) if error.is_timeout() => Err(HttpError::Timeout),
            Err(error) => Err(HttpError::Connect(sanitize_reqwest_error(error))),
        }
    }

    /// 发送一次 JSON POST，并在成功时读取响应体用于业务码校验。
    ///
    /// 适用于 Webhook 等成功响应可能为空或并非 JSON 的接口。与
    /// [`Self::post_json_once`] 一样，本方法不会自动重试可能产生外部副作用的请求。
    /// 空响应体返回 `Ok(None)`，以便调用方兼容 `204 No Content`；返回
    /// `Ok(Some(text))` 时由调用方按各服务协议校验成功码。
    pub async fn post_json_once_body<B>(
        &self,
        url: Url,
        headers: HeaderMap,
        body: &B,
    ) -> Result<Option<String>, HttpError>
    where
        B: Serialize + ?Sized,
    {
        match self
            .inner
            .request(Method::POST, url)
            .headers(headers)
            .json(body)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => read_once_body(response).await,
            Ok(response) => Err(HttpError::Status(response.status())),
            Err(error) if error.is_timeout() => Err(HttpError::Timeout),
            Err(error) => Err(HttpError::Connect(sanitize_reqwest_error(error))),
        }
    }

    /// 发送一次表单 POST，并在成功时读取响应体用于业务码校验。
    ///
    /// 语义与 [`Self::post_json_once_body`] 一致：只发送一次，成功时返回响应体。
    pub async fn post_form_once_body<B: Serialize + ?Sized>(
        &self,
        url: Url,
        body: &B,
    ) -> Result<Option<String>, HttpError> {
        match self.inner.post(url).form(body).send().await {
            Ok(response) if response.status().is_success() => read_once_body(response).await,
            Ok(response) => Err(HttpError::Status(response.status())),
            Err(error) if error.is_timeout() => Err(HttpError::Timeout),
            Err(error) => Err(HttpError::Connect(sanitize_reqwest_error(error))),
        }
    }

    /// 发送一次 JSON POST，并只校验 HTTP 状态码。
    ///
    /// 适用于响应体为空、非 JSON 或无需业务码校验的接口（如通用 Webhook）。
    /// 与 [`Self::post_json_once`] 一样，本方法不会自动重试可能产生外部副作用的请求。
    pub async fn post_json_once_without_response<B>(
        &self,
        url: Url,
        headers: HeaderMap,
        body: &B,
    ) -> Result<(), HttpError>
    where
        B: Serialize + ?Sized,
    {
        self.post_json_once_body(url, headers, body)
            .await
            .map(|_| ())
    }

    pub async fn post_form_once_without_response<B: Serialize + ?Sized>(
        &self,
        url: Url,
        body: &B,
    ) -> Result<(), HttpError> {
        self.post_form_once_body(url, body).await.map(|_| ())
    }

    pub async fn get_once_without_response(
        &self,
        url: Url,
        query: &[(&str, String)],
    ) -> Result<(), HttpError> {
        match self.inner.get(url).query(query).send().await {
            Ok(response) if response.status().is_success() => Ok(()),
            Ok(response) => Err(HttpError::Status(response.status())),
            Err(error) if error.is_timeout() => Err(HttpError::Timeout),
            Err(error) => Err(HttpError::Connect(sanitize_reqwest_error(error))),
        }
    }
}

/// 读取一次成功响应的响应体；空响应体返回 `Ok(None)`，兼容 `204 No Content`。
async fn read_once_body(response: reqwest::Response) -> Result<Option<String>, HttpError> {
    let text = response
        .text()
        .await
        .map_err(|error| HttpError::Decode(sanitize_reqwest_error(error)))?;
    Ok(if text.trim().is_empty() {
        None
    } else {
        Some(text)
    })
}

fn normalize_proxy_url(raw: &str) -> Result<Url, HttpError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(HttpError::InvalidProxy("代理地址为空".to_owned()));
    }
    let normalized = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("http://{raw}")
    };
    let url = Url::parse(&normalized)?;
    match url.scheme() {
        "http" | "https" | "socks5" | "socks5h" => Ok(url),
        scheme => Err(HttpError::InvalidProxy(format!("不支持协议 {scheme}"))),
    }
}

/// 响应体读取失败的类别。
///
/// 读取超时与 JSON 语法错误必须区别对待：前者是只读请求可重试的传输问题，后者重试无意义。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BodyFailure {
    /// 读取超时（可重试）。
    ReadTimeout,
    /// 传输中断等其它 body 错误（不重试，按解码失败返回）。
    Transport,
}

/// 本次 body 失败后的处理动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BodyFailureAction {
    Retry,
    ReturnTimeout,
    ReturnDecode,
}

/// 一次失败后是否继续重试，以及预算耗尽时应返回的类别。
struct RetryDecision {
    /// 服务端提示的等待时长（已裁剪），`None` 表示按线性退避。
    hint: Option<Duration>,
    /// 重试预算耗尽时返回的真实失败类别。
    exhausted: HttpError,
}

impl RetryDecision {
    fn wait(exhausted: HttpError) -> Self {
        Self {
            hint: None,
            exhausted,
        }
    }
}

fn classify_body_failure(error: &reqwest::Error) -> BodyFailure {
    if error.is_timeout() {
        BodyFailure::ReadTimeout
    } else {
        BodyFailure::Transport
    }
}

fn body_failure_action(failure: BodyFailure, last_attempt: bool) -> BodyFailureAction {
    match failure {
        // 传输中断与 JSON 语法错误都不重试，统一按解码失败返回。
        BodyFailure::Transport => BodyFailureAction::ReturnDecode,
        BodyFailure::ReadTimeout => {
            if last_attempt {
                BodyFailureAction::ReturnTimeout
            } else {
                BodyFailureAction::Retry
            }
        }
    }
}

fn should_retry_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

/// 解析 `Retry-After` 并裁剪到本地上限。
///
/// 只接受秒数形式；HTTP 日期形式需要额外的时间解析，这里保守忽略，避免误算长时间等待。
fn retry_after_hint(headers: &HeaderMap, max_retry_after: Duration) -> Option<Duration> {
    let raw = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    let seconds: u64 = raw.parse().ok()?;
    Some(Duration::from_secs(seconds).min(max_retry_after))
}

/// 计算本次重试的等待时长：优先服从 `Retry-After`，否则线性退避；可选抖动。
fn retry_delay(
    base_delay: Duration,
    attempt: usize,
    retry_after: Option<Duration>,
    jitter: bool,
) -> Duration {
    let expected = retry_after.unwrap_or_else(|| base_delay * (attempt as u32 + 1));
    if !jitter || expected.is_zero() {
        return expected;
    }
    // 抖动范围 [50%, 100%]：可确定性地缩短等待，不会超过服务器提示或预算。
    let half = expected / 2u32;
    half + random_fraction(half)
}

/// 以时间纳秒做简单的伪随机抖动，避免引入额外随机数依赖。
fn random_fraction(span: Duration) -> Duration {
    let nanos = span.as_nanos().max(1) as u64;
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_nanos(seed % nanos)
}

/// 判断再等一次是否会超出整次调用的等待预算。
fn fits_budget(spent: Duration, next: Duration, max_total: Duration) -> bool {
    spent.saturating_add(next) <= max_total
}

fn is_retryable(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[test]
    fn proxy_without_scheme_defaults_to_http() {
        assert_eq!(
            normalize_proxy_url("127.0.0.1:7890").unwrap().as_str(),
            "http://127.0.0.1:7890/"
        );
    }

    #[test]
    fn proxy_rejects_unsafe_scheme() {
        assert!(matches!(
            normalize_proxy_url("file:///tmp/proxy"),
            Err(HttpError::InvalidProxy(_))
        ));
    }

    #[tokio::test]
    async fn side_effect_get_is_sent_once_even_when_retry_policy_allows_more_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/side-effect"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let client = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 3,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        let result: Result<serde_json::Value, HttpError> = client
            .get_json_once_with(
                Url::parse(&format!("{}/side-effect", server.uri())).unwrap(),
                HeaderMap::new(),
                &[],
            )
            .await;

        assert!(matches!(
            result,
            Err(HttpError::Status(StatusCode::INTERNAL_SERVER_ERROR))
        ));
    }

    #[test]
    fn builder_debug_does_not_expose_proxy_credentials() {
        let proxy =
            "socks5h://proxy-user:proxy-password@127.0.0.1:1080/private?token=proxy-query-token";
        let builder = HttpClient::builder()
            .timeout(Duration::from_secs(5))
            .proxy(Some(proxy))
            .unwrap();
        let debug = format!("{builder:?}");

        assert!(!debug.contains("proxy-user"));
        assert!(!debug.contains("proxy-password"));
        assert!(!debug.contains("proxy-query-token"));
        assert!(!debug.contains("/private"));
        assert!(debug.contains("socks5h://127.0.0.1:1080"));
        assert!(debug.contains("HttpClientBuilder"));
        assert!(debug.contains("timeout"));
    }

    #[test]
    fn builder_debug_without_proxy_is_plain() {
        let debug = format!("{:?}", HttpClient::builder());
        assert!(debug.contains("proxy: None"));
        assert!(debug.contains("retry"));
    }

    #[tokio::test]
    async fn decode_error_does_not_expose_request_url_secrets() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/webhook/secret-token-value"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .expect(1)
            .mount(&server)
            .await;
        let client = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        let url = Url::parse(&format!(
            "{}/webhook/secret-token-value?access_token=query-secret-token",
            server.uri()
        ))
        .unwrap();

        let error = client
            .get_json_with::<serde_json::Value>(url, HeaderMap::new(), &[])
            .await
            .unwrap_err();

        // reqwest 的解码错误在 Display 中恒追加请求 URL，构造侧必须已脱敏。
        let HttpError::Decode(text) = &error else {
            panic!("预期解码失败，实际为 {error:?}");
        };
        assert!(!text.contains("secret-token-value"));
        assert!(!text.contains("query-secret-token"));
        let display = error.to_string();
        assert!(display.contains("响应 JSON 无效"));
        assert!(!display.contains("secret-token-value"));
        assert!(!display.contains("query-secret-token"));
    }

    #[test]
    fn for_extra_retries_maps_documented_semantics_to_attempts() {
        // 文档定义 runtime.retry_count 为“额外重试次数”，0 表示只请求一次。
        assert_eq!(RetryPolicy::for_extra_retries(0).attempts, 1);
        assert_eq!(RetryPolicy::for_extra_retries(1).attempts, 2);
        assert_eq!(RetryPolicy::for_extra_retries(3).attempts, 4);
        assert_eq!(
            RetryPolicy::for_extra_retries(0).base_delay.as_millis(),
            500
        );
        // 极端值饱和而不是溢出。
        assert_eq!(
            RetryPolicy::for_extra_retries(usize::MAX).attempts,
            usize::MAX
        );
        assert_eq!(RetryPolicy::with_attempts(1).attempts, 1);
        assert_eq!(
            RetryPolicy::for_extra_retries(RetryPolicy::FIXED_EXTRA_RETRIES).attempts,
            3
        );
    }

    /// 统计服务端实际收到的请求次数。
    async fn count_attempts(server: &MockServer) -> usize {
        server
            .received_requests()
            .await
            .map(|requests| requests.len())
            .unwrap_or_default()
    }

    async fn count_requests_with_retryable_failures(extra_retries: usize) -> usize {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/always-500"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let client = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: RetryPolicy::for_extra_retries(extra_retries).attempts,
                base_delay: Duration::ZERO,
            })
            .backoff(Backoff {
                max_retry_after: Duration::from_secs(1),
                max_total_delay: Duration::from_secs(5),
                jitter: false,
            })
            .build()
            .unwrap();

        let result = client
            .get_json::<serde_json::Value>(
                Url::parse(&format!("{}/always-500", server.uri())).unwrap(),
            )
            .await;
        assert!(matches!(
            result,
            Err(HttpError::Status(StatusCode::INTERNAL_SERVER_ERROR))
        ));
        count_attempts(&server).await
    }

    #[tokio::test]
    async fn extra_retries_zero_sends_one_request() {
        assert_eq!(count_requests_with_retryable_failures(0).await, 1);
    }

    #[tokio::test]
    async fn extra_retries_one_sends_two_requests() {
        assert_eq!(count_requests_with_retryable_failures(1).await, 2);
    }

    #[tokio::test]
    async fn extra_retries_three_sends_four_requests() {
        assert_eq!(count_requests_with_retryable_failures(3).await, 4);
    }

    #[tokio::test]
    async fn success_ends_retry_loop_immediately() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        Mock::given(method("GET"))
            .and(path("/flaky"))
            .respond_with(move |_: &wiremock::Request| {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(503)
                } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true }))
                }
            })
            .mount(&server)
            .await;
        let client = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: RetryPolicy::for_extra_retries(3).attempts,
                base_delay: Duration::ZERO,
            })
            .backoff(Backoff {
                max_retry_after: Duration::from_secs(1),
                max_total_delay: Duration::from_secs(5),
                jitter: false,
            })
            .build()
            .unwrap();

        let value = client
            .get_json::<serde_json::Value>(Url::parse(&format!("{}/flaky", server.uri())).unwrap())
            .await
            .unwrap();
        assert!(value["ok"].as_bool().unwrap_or_default());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn non_retryable_status_ends_immediately() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        let client = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: RetryPolicy::for_extra_retries(3).attempts,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();

        let result = client
            .get_json::<serde_json::Value>(
                Url::parse(&format!("{}/missing", server.uri())).unwrap(),
            )
            .await;
        assert!(matches!(
            result,
            Err(HttpError::Status(StatusCode::NOT_FOUND))
        ));
    }

    #[tokio::test]
    async fn side_effect_requests_ignore_extra_retry_configuration() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/once-get"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/once-post"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let client = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: RetryPolicy::for_extra_retries(3).attempts,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();

        let get_result = client
            .get_json_once_with::<serde_json::Value>(
                Url::parse(&format!("{}/once-get", server.uri())).unwrap(),
                HeaderMap::new(),
                &[],
            )
            .await;
        assert!(get_result.is_err());
        let post_result = client
            .post_json_once_without_response(
                Url::parse(&format!("{}/once-post", server.uri())).unwrap(),
                HeaderMap::new(),
                &serde_json::json!({ "value": 1 }),
            )
            .await;
        assert!(post_result.is_err());
    }

    #[test]
    fn retry_after_hint_is_capped_by_local_limit() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("120"));
        assert_eq!(
            retry_after_hint(&headers, Duration::from_secs(10)),
            Some(Duration::from_secs(10))
        );
        headers.insert(RETRY_AFTER, HeaderValue::from_static("2"));
        assert_eq!(
            retry_after_hint(&headers, Duration::from_secs(10)),
            Some(Duration::from_secs(2))
        );
        let mut invalid = HeaderMap::new();
        invalid.insert(
            RETRY_AFTER,
            HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"),
        );
        assert_eq!(retry_after_hint(&invalid, Duration::from_secs(10)), None);
    }

    #[test]
    fn retry_delay_prefers_retry_after_then_linear_backoff() {
        let base = Duration::from_millis(500);
        assert_eq!(
            retry_delay(base, 0, Some(Duration::from_secs(3)), false),
            Duration::from_secs(3)
        );
        assert_eq!(
            retry_delay(base, 0, None, false),
            Duration::from_millis(500)
        );
        assert_eq!(
            retry_delay(base, 2, None, false),
            Duration::from_millis(1500)
        );
        // 抖动落在 [50%, 100%]，不会超过原始等待。
        for attempt in 0..4 {
            let delay = retry_delay(base, attempt, None, true);
            let upper = base * (attempt as u32 + 1);
            assert!(delay <= upper, "抖动超过上限：{delay:?} > {upper:?}");
            assert!(delay >= upper / 2, "抖动低于下限：{delay:?}");
        }
    }

    #[test]
    fn retry_budget_limits_total_wait() {
        let budget = Duration::from_secs(1);
        assert!(fits_budget(
            Duration::ZERO,
            Duration::from_millis(400),
            budget
        ));
        assert!(fits_budget(
            Duration::from_millis(600),
            Duration::from_millis(400),
            budget
        ));
        assert!(!fits_budget(
            Duration::from_millis(600),
            Duration::from_millis(401),
            budget
        ));
        assert!(!fits_budget(Duration::ZERO, Duration::from_secs(2), budget));
    }

    #[test]
    fn body_failure_classification_separates_timeout_from_transport() {
        assert_eq!(
            body_failure_action(BodyFailure::ReadTimeout, false),
            BodyFailureAction::Retry
        );
        assert_eq!(
            body_failure_action(BodyFailure::ReadTimeout, true),
            BodyFailureAction::ReturnTimeout
        );
        // 传输中断不重试，避免与只读重试策略混淆。
        assert_eq!(
            body_failure_action(BodyFailure::Transport, false),
            BodyFailureAction::ReturnDecode
        );
        assert_eq!(
            body_failure_action(BodyFailure::Transport, true),
            BodyFailureAction::ReturnDecode
        );
    }

    #[tokio::test]
    async fn retry_after_is_honored_and_capped_for_429() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        Mock::given(method("GET"))
            .and(path("/limited"))
            .respond_with(move |_: &wiremock::Request| {
                let index = counter.fetch_add(1, Ordering::SeqCst);
                if index == 0 {
                    ResponseTemplate::new(429).insert_header("Retry-After", "0")
                } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true }))
                }
            })
            .mount(&server)
            .await;
        let client = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 2,
                base_delay: Duration::ZERO,
            })
            .backoff(Backoff {
                max_retry_after: Duration::from_secs(1),
                max_total_delay: Duration::from_secs(5),
                jitter: false,
            })
            .build()
            .unwrap();

        let value: serde_json::Value = client
            .get_json(Url::parse(&format!("{}/limited", server.uri())).unwrap())
            .await
            .unwrap();
        assert!(value["ok"].as_bool().unwrap_or_default());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn inline_json_syntax_error_is_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bad-json"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{ not json "))
            .expect(1)
            .mount(&server)
            .await;
        let client = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 3,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();

        let result = client
            .get_json::<serde_json::Value>(
                Url::parse(&format!("{}/bad-json", server.uri())).unwrap(),
            )
            .await;
        let HttpError::Decode(message) = result.unwrap_err() else {
            panic!("预期解码错误");
        };
        assert!(message.contains("JSON 语法错误"));
        assert!(message.contains("行"));
    }

    #[tokio::test]
    async fn read_only_timeout_is_retried_and_keeps_timeout_classification() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        // 服务端响应慢于客户端超时上限：只读路径按策略重试，耗尽后保留 Timeout 分类。
        Mock::given(method("GET"))
            .and(path("/slow-body"))
            .respond_with(move |_: &wiremock::Request| {
                counter.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(300))
                    .set_body_json(serde_json::json!({ "ok": true }))
            })
            .mount(&server)
            .await;
        let client = HttpClient::builder()
            .timeout(Duration::from_millis(60))
            .retry(RetryPolicy {
                attempts: 2,
                base_delay: Duration::ZERO,
            })
            .backoff(Backoff {
                max_retry_after: Duration::from_secs(1),
                max_total_delay: Duration::from_secs(5),
                jitter: false,
            })
            .build()
            .unwrap();

        let result = client
            .get_json::<serde_json::Value>(
                Url::parse(&format!("{}/slow-body", server.uri())).unwrap(),
            )
            .await;
        assert!(matches!(result, Err(HttpError::Timeout)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn side_effect_paths_stay_single_attempt_with_backoff_configured() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/side-effect"))
            .respond_with(ResponseTemplate::new(503).insert_header("Retry-After", "0"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/side-effect-post"))
            .respond_with(ResponseTemplate::new(503).insert_header("Retry-After", "0"))
            .expect(1)
            .mount(&server)
            .await;
        let client = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 4,
                base_delay: Duration::ZERO,
            })
            .backoff(Backoff {
                max_retry_after: Duration::from_secs(1),
                max_total_delay: Duration::from_secs(5),
                jitter: false,
            })
            .build()
            .unwrap();

        let get_result = client
            .get_json_once_with::<serde_json::Value>(
                Url::parse(&format!("{}/side-effect", server.uri())).unwrap(),
                HeaderMap::new(),
                &[],
            )
            .await;
        assert!(matches!(
            get_result,
            Err(HttpError::Status(StatusCode::SERVICE_UNAVAILABLE))
        ));
        let post_result = client
            .post_json_once_without_response(
                Url::parse(&format!("{}/side-effect-post", server.uri())).unwrap(),
                HeaderMap::new(),
                &serde_json::json!({ "value": 1 }),
            )
            .await;
        assert!(matches!(
            post_result,
            Err(HttpError::Status(StatusCode::SERVICE_UNAVAILABLE))
        ));
    }
}
