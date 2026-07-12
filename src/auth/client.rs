use reqwest::{
    Url,
    header::{COOKIE, HeaderMap, HeaderValue},
};
use serde::Deserialize;

use crate::http::{HttpClient, HttpError};

use super::{AuthError, CookieJar, Credentials, SecretString};

const MULTI_TOKEN_URL: &str = "https://api-takumi.mihoyo.com/auth/api/getMultiTokenByLoginTicket";
const COOKIE_TOKEN_URL: &str =
    "https://api-takumi.mihoyo.com/auth/api/getCookieAccountInfoBySToken";

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
}

impl AuthEndpoints {
    pub fn production() -> Self {
        Self {
            multi_token: Url::parse(MULTI_TOKEN_URL).expect("固定的 multi-token URL 应当有效"),
            cookie_token: Url::parse(COOKIE_TOKEN_URL).expect("固定的 cookie-token URL 应当有效"),
        }
    }

    pub fn from_base_url(base_url: &Url) -> Result<Self, url::ParseError> {
        Ok(Self {
            multi_token: base_url.join("auth/api/getMultiTokenByLoginTicket")?,
            cookie_token: base_url.join("auth/api/getCookieAccountInfoBySToken")?,
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
}

impl AuthClient {
    pub fn new(http: HttpClient) -> Self {
        Self {
            http,
            endpoints: AuthEndpoints::production(),
        }
    }

    pub fn with_endpoints(http: HttpClient, endpoints: AuthEndpoints) -> Self {
        Self { http, endpoints }
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
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RefreshOnce {
    attempted: bool,
}

impl RefreshOnce {
    pub fn attempted(&self) -> bool {
        self.attempted
    }

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
        if jar.cookie_token().is_none() {
            return Err(AuthError::MissingCookieToken);
        }

        self.attempted = true;
        let token = client.exchange_cookie_token(credentials).await?;
        credentials.replace_cookie_token(token.expose_secret())?;
        Ok(())
    }
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
        matchers::{header, method, path, query_param},
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
