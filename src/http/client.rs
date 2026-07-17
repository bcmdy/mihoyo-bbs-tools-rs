use std::time::Duration;

use reqwest::{Method, Proxy, StatusCode, Url, header::HeaderMap};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

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

#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
    retry: RetryPolicy,
}

#[derive(Debug, Clone)]
pub struct HttpClientBuilder {
    timeout: Duration,
    retry: RetryPolicy,
    proxy: Option<Url>,
    user_agent: String,
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
                    .map_err(|error| HttpError::InvalidProxy(error.to_string()))?,
            );
        }
        let inner = builder
            .build()
            .map_err(|error| HttpError::Build(error.to_string()))?;
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
                        .map_err(|error| HttpError::Decode(error.to_string()));
                }
                Ok(response)
                    if should_retry_status(response.status()) && attempt + 1 < attempts => {}
                Ok(response) => return Err(HttpError::Status(response.status())),
                Err(error) if is_retryable(&error) && attempt + 1 < attempts => {}
                Err(error) if error.is_timeout() => return Err(HttpError::Timeout),
                Err(error) => return Err(HttpError::Connect(error.to_string())),
            }
            tokio::time::sleep(self.retry.base_delay * (attempt as u32 + 1)).await;
        }
        Err(HttpError::Connect("重试次数已耗尽".to_owned()))
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
                .map_err(|error| HttpError::Decode(error.to_string())),
            Ok(response) => Err(HttpError::Status(response.status())),
            Err(error) if error.is_timeout() => Err(HttpError::Timeout),
            Err(error) => Err(HttpError::Connect(error.to_string())),
        }
    }

    /// 发送一次 JSON POST，并只校验 HTTP 状态码。
    ///
    /// 适用于 Webhook 等成功响应可能为空或并非 JSON 的接口。与
    /// [`Self::post_json_once`] 一样，本方法不会自动重试可能产生外部副作用的请求。
    pub async fn post_json_once_without_response<B>(
        &self,
        url: Url,
        headers: HeaderMap,
        body: &B,
    ) -> Result<(), HttpError>
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
            Ok(response) if response.status().is_success() => Ok(()),
            Ok(response) => Err(HttpError::Status(response.status())),
            Err(error) if error.is_timeout() => Err(HttpError::Timeout),
            Err(error) => Err(HttpError::Connect(error.to_string())),
        }
    }

    pub async fn post_form_once_without_response<B: Serialize + ?Sized>(
        &self,
        url: Url,
        body: &B,
    ) -> Result<(), HttpError> {
        match self.inner.post(url).form(body).send().await {
            Ok(response) if response.status().is_success() => Ok(()),
            Ok(response) => Err(HttpError::Status(response.status())),
            Err(error) if error.is_timeout() => Err(HttpError::Timeout),
            Err(error) => Err(HttpError::Connect(error.to_string())),
        }
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
            Err(error) => Err(HttpError::Connect(error.to_string())),
        }
    }
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
}
