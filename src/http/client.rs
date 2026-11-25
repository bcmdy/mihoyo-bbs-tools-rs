use std::time::Duration;

use reqwest::{Method, Proxy, StatusCode, Url, header::HeaderMap};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

use super::error::{describe_proxy, sanitize_reqwest_error};

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub attempts: usize,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base_delay: Duration::from_millis(500),
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
}

/// 手写 Debug：不打印 reqwest 客户端内部配置（其 Debug 会输出代理规则），只暴露重试策略。
impl std::fmt::Debug for HttpClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpClient")
            .field("retry", &self.retry)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct HttpClientBuilder {
    timeout: Duration,
    retry: RetryPolicy,
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
        for attempt in 0..attempts {
            match self
                .inner
                .request(Method::GET, url.clone())
                .headers(headers.clone())
                .query(query)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    return response
                        .json()
                        .await
                        .map_err(|error| HttpError::Decode(sanitize_reqwest_error(error)));
                }
                Ok(response)
                    if should_retry_status(response.status()) && attempt + 1 < attempts => {}
                Ok(response) => return Err(HttpError::Status(response.status())),
                Err(error) if is_retryable(&error) && attempt + 1 < attempts => {}
                Err(error) if error.is_timeout() => return Err(HttpError::Timeout),
                Err(error) => return Err(HttpError::Connect(sanitize_reqwest_error(error))),
            }
            tokio::time::sleep(self.retry.base_delay * (attempt as u32 + 1)).await;
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

fn should_retry_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn is_retryable(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
