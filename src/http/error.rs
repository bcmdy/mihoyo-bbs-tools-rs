//! HTTP 错误的统一脱敏工具。
//!
//! reqwest 的 `Error::Display` 会无条件追加请求 URL，而 URL 的用户信息、路径与查询
//! 参数都可能携带代理凭据或接口 Token，因此应用层错误必须在构造侧脱敏，避免凭据
//! 经由 Display、Debug、控制台日志或 JSON 报告泄漏。

use reqwest::Url;

use super::client::HttpError;

/// 错误类别。只保留可安全展示的分类信息，不携带任何 URL 或响应原文。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpErrorCategory {
    InvalidUrl,
    InvalidProxy,
    Timeout,
    Connect,
    Status(u16),
    Decode,
    Build,
}

/// 把 [`HttpError`] 归入可安全展示的类别。
pub fn classify_http_error(error: &HttpError) -> HttpErrorCategory {
    match error {
        HttpError::InvalidUrl(_) => HttpErrorCategory::InvalidUrl,
        HttpError::InvalidProxy(_) => HttpErrorCategory::InvalidProxy,
        HttpError::Timeout => HttpErrorCategory::Timeout,
        HttpError::Connect(_) => HttpErrorCategory::Connect,
        HttpError::Status(status) => HttpErrorCategory::Status(status.as_u16()),
        HttpError::Decode(_) => HttpErrorCategory::Decode,
        HttpError::Build(_) => HttpErrorCategory::Build,
    }
}

/// 面向诊断报告的错误类别描述，不含任何 URL、代理凭据或响应原文。
pub fn safe_http_reason(error: &HttpError) -> &'static str {
    match classify_http_error(error) {
        HttpErrorCategory::InvalidUrl => "服务地址无效",
        HttpErrorCategory::InvalidProxy => "代理地址无效",
        HttpErrorCategory::Timeout => "请求超时",
        HttpErrorCategory::Connect => "连接失败（可能是 DNS、TLS、网络或代理问题）",
        HttpErrorCategory::Status(_) => "服务返回了 HTTP 错误状态",
        HttpErrorCategory::Decode => "服务响应格式无效",
        HttpErrorCategory::Build => "HTTP 客户端初始化失败",
    }
}

/// 只描述 URL 的协议、主机与端口，丢弃用户信息、路径、查询参数与片段。
///
/// 用于代理地址等需要保留“连到哪里”信息、但不能回显凭据的场景。
pub fn describe_origin(url: &Url) -> String {
    let Some(host) = url.host_str() else {
        return "[REDACTED]".to_owned();
    };
    let mut output = format!("{}://{host}", url.scheme());
    if let Some(port) = url.port() {
        output.push(':');
        output.push_str(&port.to_string());
    }
    output
}

/// 代理地址的脱敏描述：只保留协议、主机与端口，路径与用户信息一律折叠。
pub fn describe_proxy(url: &Url) -> String {
    if url.host_str().is_none() {
        return "[REDACTED]".to_owned();
    }
    format!("{}/[REDACTED]", describe_origin(url))
}

/// 脱敏文本中出现的 URL：保留协议、主机与端口，其余部分折叠为占位符。
///
/// 无法解析的 URL 候选整段替换为占位符，宁可多脱敏也不回显未知内容。
pub fn redact_url_in_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(found) = text[cursor..].find("://") {
        let separator = cursor + found;
        let candidate_start = scheme_start(text, cursor, separator).unwrap_or(separator);
        let tail = &text[separator + 3..];
        let url_end = tail
            .find(|value: char| {
                value.is_whitespace()
                    || matches!(value, '"' | '\'' | '<' | '>' | ')' | ']' | '}' | ',')
            })
            .map_or(text.len(), |offset| separator + 3 + offset);
        output.push_str(&text[cursor..candidate_start]);
        output.push_str(&redact_url_candidate(&text[candidate_start..url_end]));
        cursor = url_end;
    }
    output.push_str(&text[cursor..]);
    output
}

/// 从 `separator` 向前回溯连续协议字符，返回协议起始的字符边界。
///
/// 使用字符边界而不是字节偏移，避免多字节文本中出现切片越界。
fn scheme_start(text: &str, cursor: usize, separator: usize) -> Option<usize> {
    let prefix = &text[cursor..separator];
    let mut start = None;
    for (index, value) in prefix.char_indices().rev() {
        if value.is_ascii_alphanumeric() || matches!(value, '+' | '-' | '.') {
            start = Some(cursor + index);
        } else {
            break;
        }
    }
    start
}

fn redact_url_candidate(candidate: &str) -> String {
    let Ok(url) = Url::parse(candidate) else {
        return "[REDACTED]".to_owned();
    };
    let mut output = describe_origin(&url);
    if url.path().is_empty() || url.path() == "/" {
        output.push('/');
    } else {
        // Webhook Token、Bot Token 与 SendKey 常直接位于路径段中，一律折叠。
        output.push_str("/[REDACTED]");
    }
    if url.query().is_some() {
        output.push_str("?[REDACTED]");
    }
    output
}

/// 把 reqwest 错误转换为可安全写入日志或报告的文本。
///
/// reqwest 0.13 提供 [`reqwest::Error::without_url`]，优先用它移除 Display 追加的
/// URL；随后仍做一次文本级兜底脱敏。
pub fn sanitize_reqwest_error(error: reqwest::Error) -> String {
    redact_url_in_text(&error.without_url().to_string())
}

/// 把 JSON 语法错误转换为只含位置信息的类别描述。
///
/// `serde_json::Error` 的 Display 在类型不匹配时可能回显响应片段，因此这里只保留
/// 行列定位，丢弃原文内容，同时保证错误仍可诊断。
pub fn describe_json_error(error: &serde_json::Error) -> String {
    format!(
        "JSON 语法错误（第 {} 行第 {} 列）",
        error.line(),
        error.column()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROXY_USER: &str = "proxy-user";
    const PROXY_PASSWORD: &str = "proxy-password";
    const PATH_TOKEN: &str = "webhook-token-value";
    const QUERY_TOKEN: &str = "query-secret-token";

    #[test]
    fn url_credentials_and_tokens_are_redacted() {
        let text = format!(
            "error sending request for url (https://{PROXY_USER}:{PROXY_PASSWORD}@example.com/hook/{PATH_TOKEN}?access_token={QUERY_TOKEN})"
        );
        let redacted = redact_url_in_text(&text);

        assert!(!redacted.contains(PROXY_USER));
        assert!(!redacted.contains(PROXY_PASSWORD));
        assert!(!redacted.contains(PATH_TOKEN));
        assert!(!redacted.contains(QUERY_TOKEN));
        assert!(redacted.contains("https://example.com"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(redacted.starts_with("error sending request for url ("));
        assert!(redacted.ends_with(')'));
    }

    #[test]
    fn plain_text_without_url_is_unchanged() {
        let text = "连接失败（可能是 DNS、TLS、网络或代理问题）";
        assert_eq!(redact_url_in_text(text), text);
    }

    #[test]
    fn url_without_credentials_keeps_origin() {
        let redacted = redact_url_in_text("connect to https://example.com failed");
        assert_eq!(redacted, "connect to https://example.com/ failed");
    }

    #[test]
    fn describe_origin_drops_userinfo_and_path() {
        let url = Url::parse(&format!(
            "socks5h://{PROXY_USER}:{PROXY_PASSWORD}@127.0.0.1:1080/private?token={QUERY_TOKEN}"
        ))
        .unwrap();
        let described = describe_origin(&url);
        assert_eq!(described, "socks5h://127.0.0.1:1080");
        assert!(!described.contains(PROXY_USER));
        assert!(!described.contains(PROXY_PASSWORD));
        assert!(!described.contains(QUERY_TOKEN));

        let proxy = describe_proxy(&url);
        assert_eq!(proxy, "socks5h://127.0.0.1:1080/[REDACTED]");
        assert!(!proxy.contains(PROXY_PASSWORD));
        assert_eq!(
            describe_proxy(&Url::parse("http://127.0.0.1:7890/").unwrap()),
            "http://127.0.0.1:7890/[REDACTED]"
        );
    }

    #[test]
    fn classification_keeps_category_without_text() {
        let error = HttpError::Connect(format!("url https://example.com/{PATH_TOKEN} 不可用"));
        assert_eq!(classify_http_error(&error), HttpErrorCategory::Connect);
        assert_eq!(
            safe_http_reason(&error),
            "连接失败（可能是 DNS、TLS、网络或代理问题）"
        );
        assert_eq!(
            classify_http_error(&HttpError::Status(reqwest::StatusCode::UNAUTHORIZED)),
            HttpErrorCategory::Status(401)
        );
        assert_eq!(
            safe_http_reason(&HttpError::Decode("invalid".to_owned())),
            "服务响应格式无效"
        );
    }
}
