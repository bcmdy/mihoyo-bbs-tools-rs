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
    games::ChinaGame,
    response::{
        ApiEnvelope, CheckinInfoData, CheckinState, RoleListData, RoleState, SignData, SignState,
        is_cookie_invalid,
    },
};

const ROLE_BASE: &str = "https://api-takumi.mihoyo.com";
const ROLE_PATH: &str = "/binding/api/getUserGameRolesByCookie";
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 12) AppleWebKit/537.36 Mobile Safari/537.36 miHoYoBBS/2.109.0";

#[derive(Debug, Error)]
pub enum CheckinError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("请求头 {0} 包含无效字符")]
    InvalidHeader(&'static str),
    #[error("签到接口返回无效数据：{0}")]
    InvalidResponse(String),
    #[error("认证信息无效或已过期")]
    CookieInvalid,
    #[error("签到接口错误 {retcode}：{message}")]
    Api { retcode: i64, message: String },
}

#[derive(Clone)]
pub struct ChinaCheckinClient {
    http: HttpClient,
    cookie: SecretString,
    device_id: String,
    user_agent: String,
    endpoint_override: Option<Url>,
}

impl ChinaCheckinClient {
    pub fn new(http: HttpClient, cookie: SecretString, device_id: impl Into<String>) -> Self {
        Self {
            http,
            cookie,
            device_id: device_id.into(),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            endpoint_override: None,
        }
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

    pub async fn roles(&self, game: ChinaGame, ds: &str) -> Result<RoleState, CheckinError> {
        let spec = game.spec();
        let response: ApiEnvelope = self
            .http
            .get_json_with(
                self.url(ROLE_BASE, ROLE_PATH)?,
                self.headers(game, ds)?,
                &[("game_biz", spec.game_biz.to_owned())],
            )
            .await?;
        self.ensure_success(&response)?;
        let data: RoleListData = response
            .decode()
            .map_err(|error| CheckinError::InvalidResponse(error.to_string()))?;
        Ok(if data.list.is_empty() {
            RoleState::NoRole
        } else {
            RoleState::Available(data.list)
        })
    }

    pub async fn status(
        &self,
        game: ChinaGame,
        region: &str,
        uid: &str,
        ds: &str,
    ) -> Result<CheckinState, CheckinError> {
        let spec = game.spec();
        let response: ApiEnvelope = self
            .http
            .get_json_with(
                self.url(spec.api_base, spec.info_path)?,
                self.headers(game, ds)?,
                &[
                    ("lang", "zh-cn".to_owned()),
                    ("act_id", spec.act_id.to_owned()),
                    ("region", region.to_owned()),
                    ("uid", uid.to_owned()),
                ],
            )
            .await?;
        self.ensure_success(&response)?;
        let data: CheckinInfoData = response
            .decode()
            .map_err(|error| CheckinError::InvalidResponse(error.to_string()))?;
        Ok(data.into())
    }

    /// 只发送一次签到 POST。验证码处理完成后是否再次发送必须由上层显式决定。
    pub async fn sign_once(
        &self,
        game: ChinaGame,
        region: &str,
        uid: &str,
        ds: &str,
        captcha: Option<&CaptchaHeaders<'_>>,
    ) -> Result<SignState, CheckinError> {
        let spec = game.spec();
        let mut headers = self.headers(game, ds)?;
        if let Some(captcha) = captcha {
            insert_header(&mut headers, "x-rpc-challenge", captcha.challenge)?;
            insert_header(&mut headers, "x-rpc-validate", captcha.validate)?;
            let seccode = format!("{}|jordan", captcha.validate);
            insert_header(&mut headers, "x-rpc-seccode", &seccode)?;
        }
        let response: ApiEnvelope = self
            .http
            .post_json_once(
                self.url(spec.api_base, spec.sign_path)?,
                headers,
                &SignRequest {
                    act_id: spec.act_id,
                    region,
                    uid,
                },
            )
            .await?;

        if response.retcode == -5003 {
            return Ok(SignState::AlreadySigned);
        }
        self.ensure_success(&response)?;
        let data: SignData = response
            .decode()
            .map_err(|error| CheckinError::InvalidResponse(error.to_string()))?;
        match data.success {
            0 => Ok(SignState::Success),
            1 => Ok(SignState::CaptchaRequired {
                gt: data.gt,
                challenge: data.challenge,
            }),
            value => Err(CheckinError::InvalidResponse(format!(
                "未知签到状态 success={value}"
            ))),
        }
    }

    fn headers(&self, game: ChinaGame, ds: &str) -> Result<HeaderMap, CheckinError> {
        let spec = game.spec();
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        headers.insert(
            ACCEPT_LANGUAGE,
            HeaderValue::from_static("zh-CN,en-US;q=0.8"),
        );
        insert_typed_header(&mut headers, COOKIE, self.cookie.expose_secret(), "Cookie")?;
        insert_typed_header(&mut headers, USER_AGENT, &self.user_agent, "User-Agent")?;
        insert_typed_header(&mut headers, ORIGIN, spec.origin, "Origin")?;
        insert_typed_header(&mut headers, REFERER, spec.referer, "Referer")?;
        insert_header(&mut headers, "ds", ds)?;
        insert_header(&mut headers, "x-rpc-channel", "miyousheluodi")?;
        insert_header(&mut headers, "x-rpc-app_version", "2.109.0")?;
        insert_header(&mut headers, "x-rpc-client_type", "5")?;
        insert_header(&mut headers, "x-rpc-device_id", &self.device_id)?;
        insert_header(
            &mut headers,
            "x-requested-with",
            "com.mihoyo.hyperion",
        )?;
        if let Some(sign_game) = spec.sign_game {
            insert_header(&mut headers, "x-rpc-signgame", sign_game)?;
        }
        Ok(headers)
    }

    fn ensure_success(&self, response: &ApiEnvelope) -> Result<(), CheckinError> {
        if response.retcode == 0 {
            Ok(())
        } else if is_cookie_invalid(response.retcode, &response.message) {
            Err(CheckinError::CookieInvalid)
        } else {
            Err(CheckinError::Api {
                retcode: response.retcode,
                message: response.message.clone(),
            })
        }
    }

    fn url(&self, production_base: &str, path: &str) -> Result<Url, CheckinError> {
        let mut url = match &self.endpoint_override {
            Some(base) => base.clone(),
            None => Url::parse(production_base).map_err(HttpError::from)?,
        };
        url.set_path(path);
        Ok(url)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CaptchaHeaders<'a> {
    pub challenge: &'a str,
    pub validate: &'a str,
}

#[derive(Serialize)]
struct SignRequest<'a> {
    act_id: &'a str,
    region: &'a str,
    uid: &'a str,
}

fn insert_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), CheckinError> {
    insert_typed_header(headers, HeaderName::from_static(name), value, name)
}

fn insert_typed_header(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: &str,
    display_name: &'static str,
) -> Result<(), CheckinError> {
    let value =
        HeaderValue::from_str(value).map_err(|_| CheckinError::InvalidHeader(display_name))?;
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

    async fn client(server: &MockServer) -> ChinaCheckinClient {
        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 3,
                base_delay: std::time::Duration::ZERO,
            })
            .build()
            .unwrap();
        ChinaCheckinClient::new(
            http,
            SecretString::new("cookie_token=secret"),
            "device-id",
        )
        .endpoint_override(Url::parse(&server.uri()).unwrap())
    }

    #[tokio::test]
    async fn empty_role_list_is_classified_without_real_api() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ROLE_PATH))
            .and(query_param("game_biz", "hk4e_cn"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "list": [] }
            })))
            .mount(&server)
            .await;

        assert_eq!(
            client(&server)
                .await
                .roles(ChinaGame::Genshin, "ds")
                .await
                .unwrap(),
            RoleState::NoRole
        );
    }

    #[tokio::test]
    async fn status_classifies_already_signed_and_sends_required_context() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/event/luna/info"))
            .and(query_param("act_id", ChinaGame::Genshin.spec().act_id))
            .and(query_param("region", "cn_gf01"))
            .and(query_param("uid", "10001"))
            .and(header("cookie", "cookie_token=secret"))
            .and(header("ds", "fixed-ds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "total_sign_day": 8, "is_sign": true, "first_bind": false }
            })))
            .mount(&server)
            .await;

        assert_eq!(
            client(&server)
                .await
                .status(ChinaGame::Genshin, "cn_gf01", "10001", "fixed-ds")
                .await
                .unwrap(),
            CheckinState::AlreadySigned { total_sign_day: 8 }
        );
    }

    #[tokio::test]
    async fn captcha_and_expired_cookie_are_distinct() {
        let captcha_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/event/luna/sign"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": { "success": 1, "gt": "gt-value", "challenge": "challenge-value" }
            })))
            .mount(&captcha_server)
            .await;
        assert_eq!(
            client(&captcha_server)
                .await
                .sign_once(ChinaGame::Genshin, "cn_gf01", "10001", "ds", None)
                .await
                .unwrap(),
            SignState::CaptchaRequired {
                gt: "gt-value".to_owned(),
                challenge: "challenge-value".to_owned()
            }
        );

        let cookie_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/event/luna/info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": -100,
                "message": "登录失效，请重新登录",
                "data": null
            })))
            .mount(&cookie_server)
            .await;
        assert!(matches!(
            client(&cookie_server)
                .await
                .status(ChinaGame::Genshin, "cn_gf01", "10001", "ds")
                .await,
            Err(CheckinError::CookieInvalid)
        ));
    }

    #[tokio::test]
    async fn sign_post_is_not_retried_on_server_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/event/luna/sign"))
            .and(body_json(json!({
                "act_id": ChinaGame::Genshin.spec().act_id,
                "region": "cn_gf01",
                "uid": "10001"
            })))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        assert!(matches!(
            client(&server)
                .await
                .sign_once(ChinaGame::Genshin, "cn_gf01", "10001", "ds", None)
                .await,
            Err(CheckinError::Http(HttpError::Status(
                StatusCode::INTERNAL_SERVER_ERROR
            )))
        ));
    }
}
