use reqwest::{
    Url,
    header::{
        ACCEPT, ACCEPT_LANGUAGE, COOKIE, HeaderMap, HeaderName, HeaderValue, ORIGIN, REFERER,
        USER_AGENT,
    },
};
use serde::Serialize;
use thiserror::Error;

use crate::{
    auth::SecretString,
    http::{HttpClient, HttpError},
};

use super::{
    games::HoyolabGame,
    response::{
        ApiEnvelope, CheckinInfoData, CheckinState, Reward, RewardListData, SignState,
        is_cookie_invalid,
    },
};

const HOYOLAB_ORIGIN: &str = "https://act.hoyolab.com";
const HOYOLAB_REFERER: &str = "https://act.hoyolab.com/";
const DEFAULT_LANGUAGE: &str = "en-us";
const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Linux; Android 12) AppleWebKit/537.36 Mobile Safari/537.36";

#[derive(Debug, Error)]
pub enum HoyolabCheckinError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("HoYoLAB 请求头 {0} 包含无效字符")]
    InvalidHeader(&'static str),
    #[error("HoYoLAB 签到接口返回无效数据：{0}")]
    InvalidResponse(String),
    #[error("HoYoLAB 认证信息无效或已过期")]
    CookieInvalid,
    #[error("HoYoLAB 签到接口错误 {retcode}：{message}")]
    Api { retcode: i64, message: String },
}

#[derive(Clone)]
pub struct HoyolabCheckinClient {
    http: HttpClient,
    cookie: SecretString,
    language: String,
    user_agent: String,
    endpoint_override: Option<Url>,
}

impl HoyolabCheckinClient {
    pub fn new(http: HttpClient, cookie: SecretString) -> Self {
        Self {
            http,
            cookie,
            language: DEFAULT_LANGUAGE.to_owned(),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            endpoint_override: None,
        }
    }

    pub fn language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    #[doc(hidden)]
    pub fn endpoint_override(mut self, base_url: Url) -> Self {
        self.endpoint_override = Some(base_url);
        self
    }

    pub async fn info(&self, game: HoyolabGame) -> Result<CheckinState, HoyolabCheckinError> {
        let spec = game.spec();
        let response: ApiEnvelope = self
            .http
            .get_json_with(
                self.url(spec.event_base, "/info")?,
                self.headers(game)?,
                &[
                    ("lang", self.language.clone()),
                    ("act_id", spec.act_id.to_owned()),
                ],
            )
            .await?;
        self.ensure_success(&response)?;
        let data: CheckinInfoData = response
            .decode()
            .map_err(|error| HoyolabCheckinError::InvalidResponse(error.to_string()))?;
        Ok(data.into())
    }

    pub async fn home(&self, game: HoyolabGame) -> Result<Vec<Reward>, HoyolabCheckinError> {
        let spec = game.spec();
        let response: ApiEnvelope = self
            .http
            .get_json_with(
                self.url(spec.event_base, "/home")?,
                self.headers(game)?,
                &[
                    ("lang", self.language.clone()),
                    ("act_id", spec.act_id.to_owned()),
                ],
            )
            .await?;
        self.ensure_success(&response)?;
        let data: RewardListData = response
            .decode()
            .map_err(|error| HoyolabCheckinError::InvalidResponse(error.to_string()))?;
        Ok(data.awards)
    }

    /// HoYoLAB 签到 POST 只发送一次，不使用 GET 的自动重试策略。
    pub async fn sign_once(&self, game: HoyolabGame) -> Result<SignState, HoyolabCheckinError> {
        let spec = game.spec();
        let mut url = self.url(spec.event_base, "/sign")?;
        url.query_pairs_mut().append_pair("lang", &self.language);
        let response: ApiEnvelope = self
            .http
            .post_json_once(
                url,
                self.headers(game)?,
                &SignRequest {
                    act_id: spec.act_id,
                },
            )
            .await?;
        match response.retcode {
            0 => Ok(SignState::Success),
            -5003 => Ok(SignState::AlreadySigned),
            _ => {
                self.ensure_success(&response)?;
                Err(HoyolabCheckinError::InvalidResponse(
                    "成功响应未映射为签到状态".to_owned(),
                ))
            }
        }
    }

    fn headers(&self, game: HoyolabGame) -> Result<HeaderMap, HoyolabCheckinError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.8"));
        insert_typed_header(&mut headers, COOKIE, self.cookie.expose_secret(), "Cookie")?;
        insert_typed_header(&mut headers, USER_AGENT, &self.user_agent, "User-Agent")?;
        insert_typed_header(&mut headers, ORIGIN, HOYOLAB_ORIGIN, "Origin")?;
        insert_typed_header(&mut headers, REFERER, HOYOLAB_REFERER, "Referer")?;
        if let Some(sign_game) = game.spec().sign_game {
            insert_header(&mut headers, "x-rpc-signgame", sign_game)?;
        }
        Ok(headers)
    }

    fn ensure_success(&self, response: &ApiEnvelope) -> Result<(), HoyolabCheckinError> {
        if response.retcode == 0 {
            Ok(())
        } else if is_cookie_invalid(response.retcode, &response.message) {
            Err(HoyolabCheckinError::CookieInvalid)
        } else {
            Err(HoyolabCheckinError::Api {
                retcode: response.retcode,
                message: response.message.clone(),
            })
        }
    }

    fn url(&self, event_base: &str, suffix: &str) -> Result<Url, HoyolabCheckinError> {
        if let Some(base) = &self.endpoint_override {
            let mut url = base.clone();
            url.set_path(suffix);
            Ok(url)
        } else {
            Url::parse(&format!("{event_base}{suffix}"))
                .map_err(HttpError::from)
                .map_err(HoyolabCheckinError::from)
        }
    }
}

#[derive(Serialize)]
struct SignRequest<'a> {
    act_id: &'a str,
}

fn insert_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), HoyolabCheckinError> {
    insert_typed_header(headers, HeaderName::from_static(name), value, name)
}

fn insert_typed_header(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: &str,
    display_name: &'static str,
) -> Result<(), HoyolabCheckinError> {
    let value = HeaderValue::from_str(value)
        .map_err(|_| HoyolabCheckinError::InvalidHeader(display_name))?;
    headers.insert(name, value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path, query_param},
    };

    use crate::http::{HttpClient, RetryPolicy};

    use super::*;

    fn client(server: &MockServer) -> HoyolabCheckinClient {
        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 3,
                base_delay: std::time::Duration::ZERO,
            })
            .build()
            .unwrap();
        HoyolabCheckinClient::new(http, SecretString::new("ltoken=secret; ltuid=10001"))
            .endpoint_override(Url::parse(&server.uri()).unwrap())
    }

    #[tokio::test]
    async fn info_maps_already_signed_and_first_bind() {
        let signed_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/info"))
            .and(query_param("lang", "en-us"))
            .and(query_param("act_id", HoyolabGame::Genshin.spec().act_id))
            .and(header("referer", HOYOLAB_REFERER))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "total_sign_day": 6, "is_sign": true, "first_bind": false }
            })))
            .mount(&signed_server)
            .await;
        assert_eq!(
            client(&signed_server)
                .info(HoyolabGame::Genshin)
                .await
                .unwrap(),
            CheckinState::AlreadySigned { total_sign_day: 6 }
        );

        let first_bind_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "total_sign_day": 0, "is_sign": false, "first_bind": true }
            })))
            .mount(&first_bind_server)
            .await;
        assert_eq!(
            client(&first_bind_server)
                .info(HoyolabGame::StarRail)
                .await
                .unwrap(),
            CheckinState::FirstBind
        );
    }

    #[tokio::test]
    async fn home_returns_typed_rewards() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/home"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "awards": [{ "icon": "https://example.invalid/item.png", "name": "Primogem", "cnt": "20" }] }
            })))
            .mount(&server)
            .await;

        assert_eq!(
            client(&server).home(HoyolabGame::Genshin).await.unwrap(),
            vec![Reward {
                icon: "https://example.invalid/item.png".to_owned(),
                name: "Primogem".to_owned(),
                cnt: 20,
            }]
        );
    }

    #[tokio::test]
    async fn sign_maps_success_already_signed_and_api_error() {
        let success_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sign"))
            .and(query_param("lang", "en-us"))
            .and(body_json(json!({
                "act_id": HoyolabGame::ZenlessZoneZero.spec().act_id
            })))
            .and(header("x-rpc-signgame", "zzz"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": null
            })))
            .mount(&success_server)
            .await;
        assert_eq!(
            client(&success_server)
                .sign_once(HoyolabGame::ZenlessZoneZero)
                .await
                .unwrap(),
            SignState::Success
        );

        let signed_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sign"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": -5003,
                "message": "Already checked in",
                "data": null
            })))
            .mount(&signed_server)
            .await;
        assert_eq!(
            client(&signed_server)
                .sign_once(HoyolabGame::Honkai3rd)
                .await
                .unwrap(),
            SignState::AlreadySigned
        );

        let error_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sign"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": -1,
                "message": "activity unavailable",
                "data": null
            })))
            .mount(&error_server)
            .await;
        assert!(matches!(
            client(&error_server)
                .sign_once(HoyolabGame::TearsOfThemis)
                .await,
            Err(HoyolabCheckinError::Api { retcode: -1, .. })
        ));
    }

    #[tokio::test]
    async fn sign_http_failure_is_sent_once() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sign"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        assert!(matches!(
            client(&server).sign_once(HoyolabGame::Genshin).await,
            Err(HoyolabCheckinError::Http(HttpError::Status(
                StatusCode::INTERNAL_SERVER_ERROR
            )))
        ));
    }

    #[tokio::test]
    async fn cookie_error_is_not_reported_as_generic_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": -100,
                "message": "Please log in",
                "data": null
            })))
            .mount(&server)
            .await;

        assert!(matches!(
            client(&server).info(HoyolabGame::Genshin).await,
            Err(HoyolabCheckinError::CookieInvalid)
        ));
    }
}
