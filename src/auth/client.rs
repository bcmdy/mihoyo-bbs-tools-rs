use reqwest::{
    Url,
    header::{COOKIE, HeaderMap, HeaderValue},
};
use serde::Deserialize;

use crate::{
    http::{HttpClient, HttpError},
    mihoyo::{
        APP_ID, APP_VERSION, CLIENT_TYPE_APP, PASSPORT_GAME_BIZ, PASSPORT_VERSION,
        SRC_TOKEN_TYPE_STOKEN, SYS_VERSION, TOKEN_TYPE_COOKIE_TOKEN, TOKEN_TYPE_LTOKEN,
    },
    signing::{DsSigner, SystemClock, ThreadRandom},
};

use super::{AuthError, CookieJar, Credentials, SecretString};

const MULTI_TOKEN_URL: &str = "https://api-takumi.mihoyo.com/auth/api/getMultiTokenByLoginTicket";
const COOKIE_TOKEN_URL: &str =
    "https://api-takumi.mihoyo.com/auth/api/getCookieAccountInfoBySToken";
const PASSPORT_EXCHANGE_URL: &str =
    "https://passport-api.mihoyo.com/account/ma-cn-session/app/exchange";

#[derive(Debug, Deserialize)]
pub struct MihoyoApiEnvelope<T> {
    pub retcode: i64,
    #[serde(default)]
    pub message: String,
    pub data: Option<T>,
}

#[derive(Clone, Debug)]
pub struct AuthEndpoints {
    pub multi_token: Url,
    pub cookie_token: Url,
    pub passport_exchange: Url,
}

impl AuthEndpoints {
    pub fn production() -> Self {
        Self {
            multi_token: Url::parse(MULTI_TOKEN_URL).expect("固定的 multi-token URL 应当有效"),
            cookie_token: Url::parse(COOKIE_TOKEN_URL).expect("固定的 cookie-token URL 应当有效"),
            passport_exchange: Url::parse(PASSPORT_EXCHANGE_URL)
                .expect("固定的 passport exchange URL 应当有效"),
        }
    }

    pub fn from_base_url(base_url: &Url) -> Result<Self, url::ParseError> {
        Ok(Self {
            multi_token: base_url.join("auth/api/getMultiTokenByLoginTicket")?,
            cookie_token: base_url.join("auth/api/getCookieAccountInfoBySToken")?,
            passport_exchange: base_url.join("account/ma-cn-session/app/exchange")?,
        })
    }
}

impl Default for AuthEndpoints {
    fn default() -> Self {
        Self::production()
    }
}

#[derive(Clone, Debug)]
pub struct AuthClient {
    http: HttpClient,
    endpoints: AuthEndpoints,
    device: PassportDevice,
}

/// 通行证接口要求的设备上下文。
#[derive(Clone, Debug, Default)]
pub struct PassportDevice {
    pub id: String,
    pub name: String,
    pub model: String,
    pub fp: String,
    /// 进程级 lifecycle id；为空时使用调用方传入的默认值。
    pub lifecycle_id: String,
}

impl PassportDevice {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        model: impl Into<String>,
        fp: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            model: model.into(),
            fp: fp.into(),
            lifecycle_id: String::new(),
        }
    }

    /// 设置进程级 lifecycle id（对应 `x-rpc-lifecycle_id`）。
    pub fn lifecycle_id(mut self, lifecycle_id: impl Into<String>) -> Self {
        self.lifecycle_id = lifecycle_id.into();
        self
    }
}

impl AuthClient {
    pub fn new(http: HttpClient) -> Self {
        Self {
            http,
            endpoints: AuthEndpoints::production(),
            device: PassportDevice::default(),
        }
    }

    pub fn with_endpoints(http: HttpClient, endpoints: AuthEndpoints) -> Self {
        Self {
            http,
            endpoints,
            device: PassportDevice::default(),
        }
    }

    /// 注入设备上下文；换票接口需要 `x-rpc-device_*` 与 `x-rpc-lifecycle_id`。
    pub fn device(mut self, device: PassportDevice) -> Self {
        self.device = device;
        self
    }

    pub async fn exchange_login_ticket(
        &self,
        login_ticket: &SecretString,
        uid: &str,
    ) -> Result<SecretString, AuthError> {
        if login_ticket.is_empty() {
            return Err(AuthError::MissingLoginTicket);
        }
        if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(AuthError::MissingUid);
        }

        let query = [
            ("login_ticket", login_ticket.expose_secret().to_owned()),
            ("token_types", "3".to_owned()),
            ("uid", uid.to_owned()),
        ];
        let envelope: MihoyoApiEnvelope<MultiTokenData> = self
            .http
            .get_json_with(self.endpoints.multi_token.clone(), HeaderMap::new(), &query)
            .await
            .map_err(map_http_error)?;

        if envelope.retcode != 0 {
            return Err(AuthError::LoginTicketExpired {
                retcode: envelope.retcode,
            });
        }
        let token = envelope
            .data
            .and_then(|data| data.list.into_iter().next())
            .map(|item| item.token)
            .filter(|token| !token.is_empty())
            .ok_or(AuthError::InvalidResponse {
                operation: "login_ticket 换取 SToken",
                field: "data.list[0].token",
            })?;
        Ok(SecretString::new(token))
    }

    pub async fn exchange_login_ticket_from_cookie(
        &self,
        credentials: &Credentials,
    ) -> Result<SecretString, AuthError> {
        if credentials.cookie.is_empty() {
            return Err(AuthError::MissingCookie);
        }
        let jar = CookieJar::parse(credentials.cookie.expose_secret())?;
        let ticket = jar.login_ticket().ok_or(AuthError::MissingLoginTicket)?;
        let uid = jar.uid().ok_or(AuthError::MissingUid)?;
        self.exchange_login_ticket(&SecretString::new(ticket), uid)
            .await
    }

    pub async fn exchange_cookie_token(
        &self,
        credentials: &Credentials,
    ) -> Result<SecretString, AuthError> {
        let cookie = credentials.stoken_cookie()?;
        let mut headers = HeaderMap::new();
        let value = HeaderValue::from_str(cookie.expose_secret())
            .map_err(|_| AuthError::InvalidCredentialHeader)?;
        headers.insert(COOKIE, value);

        let envelope: MihoyoApiEnvelope<CookieTokenData> = self
            .http
            .get_json_with(self.endpoints.cookie_token.clone(), headers, &[])
            .await
            .map_err(map_http_error)?;
        if envelope.retcode != 0 {
            return Err(AuthError::StokenExpired {
                retcode: envelope.retcode,
            });
        }
        let token = envelope
            .data
            .map(|data| data.cookie_token)
            .filter(|token| !token.is_empty())
            .ok_or(AuthError::InvalidResponse {
                operation: "SToken 换取 cookie_token",
                field: "data.cookie_token",
            })?;
        Ok(SecretString::new(token))
    }

    /// 通行证 V2 换票：用 SToken 换取 LToken 或 CookieToken。
    ///
    /// 请求体必须是**随后实际发送的同一组字节**：签名的 `b=` 段直接拼接原始 JSON，
    /// 若改用结构化序列化重新编码，字段顺序或空白变化都会导致验签失败。因此本方法
    /// 手工拼接紧凑 JSON 并只序列化一次。
    pub async fn exchange_passport_token(
        &self,
        credentials: &Credentials,
        target: PassportTokenTarget,
    ) -> Result<SecretString, AuthError> {
        if credentials.stoken.is_empty() {
            return Err(AuthError::MissingStoken);
        }
        let mid = credentials
            .mid
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(AuthError::MidRequired)?;

        let (token_type, target_type) = match target {
            PassportTokenTarget::LToken => (TOKEN_TYPE_LTOKEN, "ltoken"),
            PassportTokenTarget::CookieToken => (TOKEN_TYPE_COOKIE_TOKEN, "cookie_token"),
        };

        let body = passport_exchange_body(credentials.stoken.expose_secret(), mid, token_type);
        let mut signer = DsSigner::new(SystemClock, ThreadRandom);
        let ds = signer.sign_passport(&body).to_string();

        let mut headers = HeaderMap::new();
        insert_header(&mut headers, "accept", "application/json")?;
        insert_header(&mut headers, "content-type", "application/json")?;
        insert_header(&mut headers, "ds", &ds)?;
        insert_header(&mut headers, "x-rpc-app_id", APP_ID)?;
        insert_header(&mut headers, "x-rpc-client_type", CLIENT_TYPE_APP)?;
        insert_header(&mut headers, "x-rpc-app_version", APP_VERSION)?;
        insert_header(&mut headers, "x-rpc-game_biz", PASSPORT_GAME_BIZ)?;
        insert_header(&mut headers, "x-rpc-sys_version", SYS_VERSION)?;
        insert_header(&mut headers, "x-rpc-sdk_version", PASSPORT_VERSION)?;
        insert_header(&mut headers, "x-rpc-account_version", PASSPORT_VERSION)?;
        insert_header(
            &mut headers,
            "x-rpc-lifecycle_id",
            &self.device.lifecycle_id,
        )?;
        insert_header(&mut headers, "x-rpc-device_id", &self.device.id)?;
        insert_header(&mut headers, "x-rpc-device_name", &self.device.name)?;
        insert_header(&mut headers, "x-rpc-device_model", &self.device.model)?;
        insert_header(&mut headers, "user-agent", "okhttp/4.9.3")?;
        if !self.device.fp.is_empty() {
            insert_header(&mut headers, "x-rpc-device_fp", &self.device.fp)?;
        }

        let envelope: MihoyoApiEnvelope<PassportTokenData> = self
            .http
            .post_bytes_once(self.endpoints.passport_exchange.clone(), headers, body)
            .await
            .map_err(map_http_error)?;
        if envelope.retcode != 0 {
            return Err(match envelope.retcode {
                // -100 为凭据失效，-3260 为通行证侧会话失效。
                -100 | -3260 => AuthError::StokenExpired {
                    retcode: envelope.retcode,
                },
                retcode => AuthError::PassportExchange {
                    retcode,
                    target: target_type,
                },
            });
        }
        let token = envelope
            .data
            .map(|data| data.token.token)
            .filter(|token| !token.is_empty())
            .ok_or(AuthError::InvalidResponse {
                operation: "通行证换票",
                field: "data.token.token",
            })?;
        Ok(SecretString::new(token))
    }
}

/// 通行证换票的目标 Token 类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PassportTokenTarget {
    LToken,
    CookieToken,
}

/// 构造换票请求体；字段顺序固定，且与签名使用的字节完全一致。
fn passport_exchange_body(stoken: &str, mid: &str, dst_token_type: u32) -> Vec<u8> {
    format!(
        concat!(
            r#"{{"dst_token_type":{},"src_token":{{"token":"{}","token_type":{}}},"mid":"{}"}}"#
        ),
        dst_token_type, stoken, SRC_TOKEN_TYPE_STOKEN, mid
    )
    .into_bytes()
}

fn insert_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), AuthError> {
    let value = HeaderValue::from_str(value).map_err(|_| AuthError::InvalidCredentialHeader)?;
    headers.insert(name, value);
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RefreshOnce {
    attempted: bool,
}

impl RefreshOnce {
    pub fn attempted(&self) -> bool {
        self.attempted
    }

    /// 刷新 CookieToken，每个任务流程最多一次。
    ///
    /// V2 SToken 优先走通行证换票（V2 Cookie 的社区任务只认 `cookie_token_v2`）；
    /// 换票被服务端以非凭据原因拒绝时回退旧接口，保持对老账号的兼容。凭据失效类
    /// 错误直接返回，不再回退，避免重复无效请求。
    pub async fn refresh_cookie_token(
        &mut self,
        client: &AuthClient,
        credentials: &mut Credentials,
    ) -> Result<(), AuthError> {
        if self.attempted {
            return Err(AuthError::RefreshAlreadyAttempted);
        }

        credentials.hydrate_from_cookie()?;
        let jar = CookieJar::parse(credentials.cookie.expose_secret())?;
        // V2 Cookie 可能只有 `cookie_token_v2`，此时同样允许刷新。
        if jar.any_cookie_token().is_none() {
            return Err(AuthError::MissingCookieToken);
        }

        self.attempted = true;
        let token = if credentials.requires_mid() {
            match client
                .exchange_passport_token(credentials, PassportTokenTarget::CookieToken)
                .await
            {
                Ok(token) => token,
                Err(AuthError::PassportExchange { .. }) => {
                    client.exchange_cookie_token(credentials).await?
                }
                Err(error) => return Err(error),
            }
        } else {
            client.exchange_cookie_token(credentials).await?
        };
        credentials.replace_cookie_token(token.expose_secret())?;
        Ok(())
    }
}

/// 补齐缺失的 V2 凭据；已有值一律不覆盖。
///
/// 与 [`RefreshOnce`] 的区别是这是“补缺失”而不是“换失效”：只在 Cookie 缺少
/// `cookie_token` 时换 CookieToken，缺少 `ltoken` 时换 LToken。任何一步被服务端
/// 拒绝都只跳过该步，不影响其余任务。
pub async fn backfill_v2_tokens(
    client: &AuthClient,
    credentials: &mut Credentials,
) -> Result<BackfillOutcome, AuthError> {
    credentials.hydrate_from_cookie()?;
    if !credentials.requires_mid() {
        return Ok(BackfillOutcome::NotV2);
    }

    let mut outcome = BackfillOutcome::Unchanged;
    let jar = CookieJar::parse(credentials.cookie.expose_secret())?;

    if jar.any_cookie_token().is_none() {
        match client
            .exchange_passport_token(credentials, PassportTokenTarget::CookieToken)
            .await
        {
            Ok(token) => {
                credentials.replace_cookie_token(token.expose_secret())?;
                outcome = BackfillOutcome::Updated;
            }
            Err(AuthError::StokenExpired { retcode }) => {
                return Err(AuthError::StokenExpired { retcode });
            }
            Err(_) => {}
        }
    }

    if jar.any_ltoken().is_none()
        && let Ok(token) = client
            .exchange_passport_token(credentials, PassportTokenTarget::LToken)
            .await
    {
        credentials.replace_ltoken(token.expose_secret())?;
        outcome = BackfillOutcome::Updated;
    }

    Ok(outcome)
}

/// [`backfill_v2_tokens`] 的执行结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackfillOutcome {
    /// 非 V2 凭据，无需补齐。
    NotV2,
    /// 字段已齐全，未发起请求。
    Unchanged,
    /// 已补入至少一个字段。
    Updated,
}

#[derive(Debug, Deserialize)]
struct MultiTokenData {
    #[serde(default)]
    list: Vec<TokenItem>,
}

#[derive(Debug, Deserialize)]
struct TokenItem {
    token: String,
}

#[derive(Debug, Deserialize)]
struct CookieTokenData {
    cookie_token: String,
}

#[derive(Debug, Deserialize)]
struct PassportTokenData {
    token: PassportToken,
}

#[derive(Debug, Deserialize)]
struct PassportToken {
    token: String,
}

fn map_http_error(error: HttpError) -> AuthError {
    match error {
        HttpError::Status(status) => AuthError::HttpStatus(status.as_u16()),
        HttpError::Decode(_) => AuthError::InvalidResponse {
            operation: "解析认证响应",
            field: "JSON",
        },
        HttpError::InvalidUrl(_)
        | HttpError::InvalidProxy(_)
        | HttpError::Timeout
        | HttpError::Connect(_)
        | HttpError::Build(_) => AuthError::Network,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string, header, method, path, query_param},
    };

    use super::*;
    use crate::http::RetryPolicy;

    fn test_client(server: &MockServer) -> AuthClient {
        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        let base_url = Url::parse(&format!("{}/", server.uri())).unwrap();
        let endpoints = AuthEndpoints::from_base_url(&base_url).unwrap();
        AuthClient::with_endpoints(http, endpoints)
    }

    #[tokio::test]
    async fn exchanges_login_ticket_without_exposing_it_in_models() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/auth/api/getMultiTokenByLoginTicket"))
            .and(query_param("login_ticket", "secret-ticket"))
            .and(query_param("token_types", "3"))
            .and(query_param("uid", "123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "list": [{ "token": "new-stoken" }] }
            })))
            .mount(&server)
            .await;

        let client = test_client(&server);
        let token = client
            .exchange_login_ticket(&SecretString::new("secret-ticket"), "123")
            .await
            .unwrap();
        assert_eq!(token.expose_secret(), "new-stoken");
        assert!(!format!("{token:?}").contains("new-stoken"));
    }

    #[tokio::test]
    async fn classifies_expired_login_ticket() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": -100,
                "message": "expired",
                "data": null
            })))
            .mount(&server)
            .await;

        let error = test_client(&server)
            .exchange_login_ticket(&SecretString::new("secret-ticket"), "123")
            .await
            .unwrap_err();
        assert_eq!(error, AuthError::LoginTicketExpired { retcode: -100 });
    }

    fn passport_client(server: &MockServer, device: PassportDevice) -> AuthClient {
        test_client(server).device(device)
    }

    fn test_device() -> PassportDevice {
        PassportDevice::new("device-id", "Xiaomi MI 6", "Mi 6", "device-fp")
            .lifecycle_id("lifecycle-id")
    }

    #[tokio::test]
    async fn passport_exchange_posts_exact_signed_bytes() {
        let server = MockServer::start().await;
        let expected = r#"{"dst_token_type":4,"src_token":{"token":"v2_secret","token_type":1},"mid":"mid-value"}"#;
        Mock::given(method("POST"))
            .and(path("/account/ma-cn-session/app/exchange"))
            .and(header("content-type", "application/json"))
            .and(header("x-rpc-app_id", APP_ID))
            .and(header("x-rpc-game_biz", PASSPORT_GAME_BIZ))
            .and(header("x-rpc-sdk_version", PASSPORT_VERSION))
            .and(header("x-rpc-lifecycle_id", "lifecycle-id"))
            .and(header("x-rpc-device_id", "device-id"))
            .and(header("x-rpc-device_fp", "device-fp"))
            .and(body_string(expected))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "token": { "token": "new-cookie-token-v2", "token_type": 4 } }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut credentials = Credentials::new("account_id_v2=123; mid=mid-value", "v2_secret");
        credentials.hydrate_from_cookie().unwrap();
        let token = passport_client(&server, test_device())
            .exchange_passport_token(&credentials, PassportTokenTarget::CookieToken)
            .await
            .unwrap();
        assert_eq!(token.expose_secret(), "new-cookie-token-v2");
        assert!(!format!("{token:?}").contains("new-cookie-token-v2"));
    }

    #[tokio::test]
    async fn passport_exchange_requests_ltoken_with_type_two() {
        let server = MockServer::start().await;
        let expected = r#"{"dst_token_type":2,"src_token":{"token":"v2_secret","token_type":1},"mid":"mid-value"}"#;
        Mock::given(method("POST"))
            .and(body_string(expected))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "token": { "token": "new-ltoken-v2", "token_type": 2 } }
            })))
            .mount(&server)
            .await;

        let mut credentials = Credentials::new("account_id_v2=123; mid=mid-value", "v2_secret");
        credentials.hydrate_from_cookie().unwrap();
        let token = passport_client(&server, test_device())
            .exchange_passport_token(&credentials, PassportTokenTarget::LToken)
            .await
            .unwrap();
        assert_eq!(token.expose_secret(), "new-ltoken-v2");
    }

    #[tokio::test]
    async fn passport_exchange_classifies_expired_session() {
        for retcode in [-100, -3260] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "retcode": retcode,
                    "message": "expired",
                    "data": null
                })))
                .mount(&server)
                .await;

            let mut credentials = Credentials::new("account_id_v2=123; mid=mid-value", "v2_secret");
            credentials.hydrate_from_cookie().unwrap();
            assert_eq!(
                passport_client(&server, test_device())
                    .exchange_passport_token(&credentials, PassportTokenTarget::CookieToken)
                    .await
                    .unwrap_err(),
                AuthError::StokenExpired { retcode }
            );
        }
    }

    #[tokio::test]
    async fn passport_exchange_reports_other_retcodes_without_leaking_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 10001,
                "message": "invalid request",
                "data": null
            })))
            .mount(&server)
            .await;

        let mut credentials = Credentials::new("account_id_v2=123; mid=mid-value", "v2_secret");
        credentials.hydrate_from_cookie().unwrap();
        let error = passport_client(&server, test_device())
            .exchange_passport_token(&credentials, PassportTokenTarget::CookieToken)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::PassportExchange {
                retcode: 10001,
                target: "cookie_token"
            }
        );
        assert!(!error.to_string().contains("v2_secret"));
    }

    #[tokio::test]
    async fn refresh_prefers_v2_exchange_and_writes_v2_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/account/ma-cn-session/app/exchange"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "token": { "token": "fresh-token" } }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/auth/api/getCookieAccountInfoBySToken"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let mut credentials = Credentials::new(
            "account_id_v2=123; account_mid_v2=mid-value; cookie_token_v2=stale",
            "v2_secret",
        );
        credentials.hydrate_from_cookie().unwrap();
        let mut refresh = RefreshOnce::default();
        refresh
            .refresh_cookie_token(&passport_client(&server, test_device()), &mut credentials)
            .await
            .unwrap();

        let cookie = credentials.cookie.expose_secret();
        assert!(cookie.contains("cookie_token_v2=fresh-token"), "{cookie}");
        // 别名同步：V1 旧名与 V2 同值，不再残留 stale。
        assert!(cookie.contains("cookie_token=fresh-token"), "{cookie}");
        assert!(!cookie.contains("stale"), "{cookie}");
    }

    #[tokio::test]
    async fn refresh_falls_back_to_legacy_endpoint_on_passport_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/account/ma-cn-session/app/exchange"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 10001,
                "message": "invalid request",
                "data": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/auth/api/getCookieAccountInfoBySToken"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "cookie_token": "legacy-token" }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut credentials = Credentials::new(
            "account_id_v2=123; account_mid_v2=mid-value; cookie_token=stale",
            "v2_secret",
        );
        credentials.hydrate_from_cookie().unwrap();
        let mut refresh = RefreshOnce::default();
        refresh
            .refresh_cookie_token(&passport_client(&server, test_device()), &mut credentials)
            .await
            .unwrap();
        assert!(credentials.cookie.expose_secret().contains("legacy-token"));
    }

    #[tokio::test]
    async fn backfill_only_requests_missing_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/account/ma-cn-session/app/exchange"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "token": { "token": "backfilled" } }
            })))
            .expect(2)
            .mount(&server)
            .await;

        // 只有 identity 字段，缺 cookie_token 与 ltoken，两次补齐都应发生。
        let mut credentials =
            Credentials::new("account_id_v2=123; account_mid_v2=mid-value", "v2_secret");
        let outcome =
            backfill_v2_tokens(&passport_client(&server, test_device()), &mut credentials)
                .await
                .unwrap();
        assert_eq!(outcome, BackfillOutcome::Updated);
        let cookie = credentials.cookie.expose_secret();
        assert!(cookie.contains("cookie_token_v2=backfilled"), "{cookie}");
        assert!(cookie.contains("ltoken_v2=backfilled"), "{cookie}");
        assert!(cookie.contains("ltuid_v2=123"), "{cookie}");
        assert!(cookie.contains("ltmid_v2=mid-value"), "{cookie}");
    }

    #[tokio::test]
    async fn backfill_skips_complete_and_non_v2_credentials() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let mut complete = Credentials::new(
            "account_id_v2=123; account_mid_v2=mid; cookie_token_v2=a; ltoken_v2=b",
            "v2_secret",
        );
        assert_eq!(
            backfill_v2_tokens(&passport_client(&server, test_device()), &mut complete)
                .await
                .unwrap(),
            BackfillOutcome::Unchanged
        );

        let mut legacy = Credentials::new("account_id=123; cookie_token=old", "legacy-stoken");
        assert_eq!(
            backfill_v2_tokens(&passport_client(&server, test_device()), &mut legacy)
                .await
                .unwrap(),
            BackfillOutcome::NotV2
        );
    }

    #[tokio::test]
    async fn exchanges_cookie_token_with_stoken_cookie_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/auth/api/getCookieAccountInfoBySToken"))
            .and(header("cookie", "stuid=123;stoken=v2_secret;mid=mid-value"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "cookie_token": "new-cookie-token" }
            })))
            .mount(&server)
            .await;

        let mut credentials = Credentials::new(
            "account_id_v2=123; mid=mid-value; cookie_token=old",
            "v2_secret",
        );
        credentials.hydrate_from_cookie().unwrap();
        let token = test_client(&server)
            .exchange_cookie_token(&credentials)
            .await
            .unwrap();
        assert_eq!(token.expose_secret(), "new-cookie-token");
    }

    #[tokio::test]
    async fn refreshes_cookie_token_only_once() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/auth/api/getCookieAccountInfoBySToken"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "cookie_token": "new-token" }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server);
        let mut credentials = Credentials::new(
            "account_id=123; cookie_token=old; token_copy=old",
            "legacy-stoken",
        );
        let mut refresh = RefreshOnce::default();

        refresh
            .refresh_cookie_token(&client, &mut credentials)
            .await
            .unwrap();
        assert!(refresh.attempted());
        assert_eq!(
            credentials.cookie.expose_secret(),
            "account_id=123; cookie_token=new-token; token_copy=old"
        );
        assert_eq!(
            refresh
                .refresh_cookie_token(&client, &mut credentials)
                .await,
            Err(AuthError::RefreshAlreadyAttempted)
        );
    }

    #[tokio::test]
    async fn failed_remote_refresh_still_consumes_the_single_attempt() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": -100,
                "message": "expired",
                "data": null
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server);
        let mut credentials = Credentials::new("account_id=123; cookie_token=old", "expired");
        let mut refresh = RefreshOnce::default();

        assert!(matches!(
            refresh
                .refresh_cookie_token(&client, &mut credentials)
                .await,
            Err(AuthError::StokenExpired { .. })
        ));
        assert!(refresh.attempted());
        assert_eq!(
            refresh
                .refresh_cookie_token(&client, &mut credentials)
                .await,
            Err(AuthError::RefreshAlreadyAttempted)
        );
    }

    #[tokio::test]
    async fn rejects_success_response_missing_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "list": [] }
            })))
            .mount(&server)
            .await;

        let error = test_client(&server)
            .exchange_login_ticket(&SecretString::new("ticket"), "123")
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::InvalidResponse {
                operation: "login_ticket 换取 SToken",
                field: "data.list[0].token"
            }
        );
    }
}
