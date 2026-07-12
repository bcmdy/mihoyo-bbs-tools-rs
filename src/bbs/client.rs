use std::collections::HashSet;

use rand::Rng;
use reqwest::{
    Url,
    header::{
        ACCEPT, ACCEPT_ENCODING, CONNECTION, CONTENT_TYPE, COOKIE, HeaderMap, HeaderName,
        HeaderValue, ORIGIN, REFERER, USER_AGENT,
    },
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    auth::SecretString,
    http::{HttpClient, HttpError},
};

use super::model::{CoinSummary, MissionData, PostListData, PostRef};

const BBS_BASE: &str = "https://bbs-api.miyoushe.com";
const APP_VERSION: &str = "2.109.0";

#[derive(Debug, Error)]
pub enum BbsError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("请求头 {0} 包含无效字符")]
    InvalidHeader(&'static str),
    #[error("BBS 接口返回无效数据：{0}")]
    InvalidResponse(String),
    #[error("BBS 认证信息无效或已过期")]
    AuthExpired,
    #[error("BBS 操作触发验证码")]
    CaptchaRequired,
    #[error("BBS 接口错误 {retcode}：{message}")]
    Api { retcode: i64, message: String },
}

#[derive(Clone, Debug)]
pub struct BbsEndpoints {
    pub missions: Url,
    pub posts: Url,
    pub read: Url,
    pub like: Url,
    pub share: Url,
    pub sign: Url,
}

impl BbsEndpoints {
    pub fn production() -> Self {
        Self::from_base_url(&Url::parse(BBS_BASE).expect("固定 BBS URL 应当有效"))
            .expect("固定 BBS 端点应当有效")
    }

    pub fn from_base_url(base_url: &Url) -> Result<Self, url::ParseError> {
        Ok(Self {
            missions: base_url.join("apihub/wapi/getUserMissionsState")?,
            posts: base_url.join("post/api/getForumPostList")?,
            read: base_url.join("post/api/getPostFull")?,
            like: base_url.join("apihub/sapi/upvotePost")?,
            share: base_url.join("apihub/api/getShareConf")?,
            sign: base_url.join("apihub/app/api/signIn")?,
        })
    }
}

impl Default for BbsEndpoints {
    fn default() -> Self {
        Self::production()
    }
}

#[derive(Clone)]
pub struct BbsClient {
    http: HttpClient,
    app_cookie: SecretString,
    web_cookie: SecretString,
    device_id: String,
    device_name: String,
    device_model: String,
    endpoints: BbsEndpoints,
}

impl BbsClient {
    pub fn new(
        http: HttpClient,
        app_cookie: SecretString,
        web_cookie: SecretString,
        device_id: impl Into<String>,
    ) -> Self {
        Self {
            http,
            app_cookie,
            web_cookie,
            device_id: device_id.into(),
            device_name: "Xiaomi MI 6".to_owned(),
            device_model: "Mi 6".to_owned(),
            endpoints: BbsEndpoints::production(),
        }
    }

    pub fn device(mut self, name: impl Into<String>, model: impl Into<String>) -> Self {
        self.device_name = name.into();
        self.device_model = model.into();
        self
    }

    #[doc(hidden)]
    pub fn endpoints(mut self, endpoints: BbsEndpoints) -> Self {
        self.endpoints = endpoints;
        self
    }

    pub async fn missions(&self) -> Result<CoinSummary, BbsError> {
        let response: ApiEnvelope<MissionData> = self
            .http
            .get_json_with(
                self.endpoints.missions.clone(),
                self.web_headers()?,
                &[("point_sn", "myb".to_owned())],
            )
            .await?;
        Ok(self.data(response)?.into())
    }

    pub async fn posts(
        &self,
        forum_id: &str,
        page_size: u32,
        ds: &str,
    ) -> Result<Vec<PostRef>, BbsError> {
        let response: ApiEnvelope<PostListData> = self
            .http
            .get_json_with(
                self.endpoints.posts.clone(),
                self.app_headers(ds)?,
                &[
                    ("forum_id", forum_id.to_owned()),
                    ("is_good", "false".to_owned()),
                    ("is_hot", "false".to_owned()),
                    ("page_size", page_size.to_string()),
                    ("sort_type", "1".to_owned()),
                ],
            )
            .await?;
        let data = self.data(response)?;
        Ok(data
            .list
            .into_iter()
            .map(|entry| entry.post.into())
            .collect())
    }

    pub async fn read_post(&self, post_id: &str, ds: &str) -> Result<(), BbsError> {
        let response: ApiEnvelope<serde_json::Value> = self
            .http
            .get_json_with(
                self.endpoints.read.clone(),
                self.app_headers(ds)?,
                &[("post_id", post_id.to_owned())],
            )
            .await?;
        self.success(response)
    }

    /// 点赞与取消点赞均只发送一次 POST。验证码通过后的重试由上层显式发起。
    pub async fn set_like_once(
        &self,
        post_id: &str,
        cancel: bool,
        ds: &str,
        captcha_challenge: Option<&str>,
    ) -> Result<(), BbsError> {
        let response: ApiEnvelope<serde_json::Value> = self
            .http
            .post_json_once(
                self.endpoints.like.clone(),
                self.app_headers_with_challenge(ds, captcha_challenge)?,
                &LikeRequest {
                    post_id,
                    is_cancel: cancel,
                },
            )
            .await?;
        self.success(response)
    }

    pub async fn share_post(&self, post_id: &str, ds: &str) -> Result<(), BbsError> {
        let response: ApiEnvelope<serde_json::Value> = self
            .http
            .get_json_with(
                self.endpoints.share.clone(),
                self.app_headers(ds)?,
                &[
                    ("entity_id", post_id.to_owned()),
                    ("entity_type", "1".to_owned()),
                ],
            )
            .await?;
        self.success(response)
    }

    /// 社区签到只发送一次 POST。DS 必须由实际发送的 `{"gids":"..."}` JSON 生成。
    pub async fn sign_forum_once(
        &self,
        gids: &str,
        ds: &str,
        captcha_challenge: Option<&str>,
    ) -> Result<(), BbsError> {
        let response: ApiEnvelope<serde_json::Value> = self
            .http
            .post_json_once(
                self.endpoints.sign.clone(),
                self.app_headers_with_challenge(ds, captcha_challenge)?,
                &ForumSignRequest { gids },
            )
            .await?;
        self.success(response)
    }

    pub fn select_posts(&self, posts: &[PostRef], count: usize) -> Vec<PostRef> {
        select_posts_with_rng(posts, count, &mut rand::rng())
    }

    fn success<T>(&self, response: ApiEnvelope<T>) -> Result<(), BbsError> {
        self.ensure_success(response.retcode, &response.message)
    }

    fn data<T>(&self, response: ApiEnvelope<T>) -> Result<T, BbsError> {
        self.ensure_success(response.retcode, &response.message)?;
        response
            .data
            .ok_or_else(|| BbsError::InvalidResponse("响应缺少 data".to_owned()))
    }

    fn ensure_success(&self, retcode: i64, message: &str) -> Result<(), BbsError> {
        match retcode {
            0 => Ok(()),
            1034 => Err(BbsError::CaptchaRequired),
            -100 => Err(BbsError::AuthExpired),
            _ => Err(BbsError::Api {
                retcode,
                message: message.to_owned(),
            }),
        }
    }

    fn web_headers(&self) -> Result<HeaderMap, BbsError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        insert_typed_header(
            &mut headers,
            COOKIE,
            self.web_cookie.expose_secret(),
            "Cookie",
        )?;
        insert_typed_header(
            &mut headers,
            ORIGIN,
            "https://webstatic.mihoyo.com",
            "Origin",
        )?;
        insert_typed_header(
            &mut headers,
            REFERER,
            "https://webstatic.mihoyo.com",
            "Referer",
        )?;
        Ok(headers)
    }

    fn app_headers(&self, ds: &str) -> Result<HeaderMap, BbsError> {
        self.app_headers_with_challenge(ds, None)
    }

    fn app_headers_with_challenge(
        &self,
        ds: &str,
        captcha_challenge: Option<&str>,
    ) -> Result<HeaderMap, BbsError> {
        let mut headers = HeaderMap::new();
        insert_typed_header(
            &mut headers,
            COOKIE,
            self.app_cookie.expose_secret(),
            "Cookie",
        )?;
        insert_typed_header(
            &mut headers,
            CONTENT_TYPE,
            "application/json; charset=UTF-8",
            "Content-Type",
        )?;
        insert_typed_header(&mut headers, REFERER, "https://app.mihoyo.com", "Referer")?;
        insert_typed_header(&mut headers, USER_AGENT, "okhttp/4.9.3", "User-Agent")?;
        insert_typed_header(&mut headers, CONNECTION, "Keep-Alive", "Connection")?;
        insert_typed_header(&mut headers, ACCEPT_ENCODING, "gzip", "Accept-Encoding")?;
        insert_header(&mut headers, "ds", ds)?;
        insert_header(&mut headers, "x-rpc-client_type", "2")?;
        insert_header(&mut headers, "x-rpc-app_version", APP_VERSION)?;
        insert_header(&mut headers, "x-rpc-sys_version", "12")?;
        insert_header(&mut headers, "x-rpc-channel", "miyousheluodi")?;
        insert_header(&mut headers, "x-rpc-device_id", &self.device_id)?;
        insert_header(&mut headers, "x-rpc-device_name", &self.device_name)?;
        insert_header(&mut headers, "x-rpc-device_model", &self.device_model)?;
        insert_header(&mut headers, "x-rpc-h265_supported", "1")?;
        insert_header(&mut headers, "x-rpc-verify_key", "bll8iq97cem8")?;
        insert_header(&mut headers, "x-rpc-csm_source", "discussion")?;
        if let Some(challenge) = captcha_challenge {
            insert_header(&mut headers, "x-rpc-challenge", challenge)?;
        }
        Ok(headers)
    }
}

pub fn select_posts_with_rng<R>(posts: &[PostRef], count: usize, rng: &mut R) -> Vec<PostRef>
where
    R: Rng + ?Sized,
{
    let mut seen = HashSet::new();
    let mut selected = posts
        .iter()
        .filter(|post| seen.insert(post.post_id.clone()))
        .cloned()
        .collect::<Vec<_>>();
    for index in (1..selected.len()).rev() {
        let swap_with = rng.random_range(0..=index);
        selected.swap(index, swap_with);
    }
    selected.truncate(count.min(selected.len()));
    selected
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    retcode: i64,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

#[derive(Serialize)]
struct LikeRequest<'a> {
    post_id: &'a str,
    is_cancel: bool,
}

#[derive(Serialize)]
struct ForumSignRequest<'a> {
    gids: &'a str,
}

fn insert_header(headers: &mut HeaderMap, name: &'static str, value: &str) -> Result<(), BbsError> {
    insert_typed_header(headers, HeaderName::from_static(name), value, name)
}

fn insert_typed_header(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: &str,
    display_name: &'static str,
) -> Result<(), BbsError> {
    let value = HeaderValue::from_str(value).map_err(|_| BbsError::InvalidHeader(display_name))?;
    headers.insert(name, value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rand::{SeedableRng, rngs::StdRng};
    use reqwest::StatusCode;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path, query_param},
    };

    use super::*;
    use crate::http::RetryPolicy;

    fn client(server: &MockServer) -> BbsClient {
        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap();
        let base = Url::parse(&format!("{}/", server.uri())).unwrap();
        BbsClient::new(
            http,
            SecretString::new("stuid=123;stoken=secret"),
            SecretString::new("cookie_token=secret"),
            "device-id",
        )
        .endpoints(BbsEndpoints::from_base_url(&base).unwrap())
    }

    #[tokio::test]
    async fn mission_state_maps_coin_summary_and_auth_expiry() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apihub/wapi/getUserMissionsState"))
            .and(query_param("point_sn", "myb"))
            .and(header("cookie", "cookie_token=secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {
                    "can_get_points": 30,
                    "already_received_points": 70,
                    "total_points": 500,
                    "states": [
                        {"mission_id": 59, "is_get_award": false, "happened_times": 2},
                        {"mission_id": 60, "is_get_award": true, "happened_times": 5}
                    ]
                }
            })))
            .mount(&server)
            .await;

        let summary = client(&server).missions().await.unwrap();
        assert_eq!(summary.can_get_points, 30);
        assert_eq!(
            summary.mission(MissionKind::Read).unwrap().happened_times,
            2
        );
        assert!(summary.mission(MissionKind::Like).unwrap().award_received);

        let expired = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": -100, "message": "expired", "data": null
            })))
            .mount(&expired)
            .await;
        assert!(matches!(
            client(&expired).missions().await,
            Err(BbsError::AuthExpired)
        ));
    }

    #[tokio::test]
    async fn post_list_contract_and_finite_unique_selection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/post/api/getForumPostList"))
            .and(query_param("forum_id", "26"))
            .and(query_param("page_size", "20"))
            .and(header("ds", "fixed-ds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "message": "OK",
                "data": {"list": [
                    {"post": {"post_id": "1", "subject": "A"}},
                    {"post": {"post_id": "1", "subject": "duplicate"}},
                    {"post": {"post_id": "2", "subject": "B"}}
                ]}
            })))
            .mount(&server)
            .await;

        let api_posts = client(&server).posts("26", 20, "fixed-ds").await.unwrap();
        let mut rng = StdRng::seed_from_u64(7);
        let selected = select_posts_with_rng(&api_posts, 10, &mut rng);
        assert_eq!(selected.len(), 2);
        assert_ne!(selected[0].post_id, selected[1].post_id);
    }

    #[tokio::test]
    async fn read_and_share_send_expected_queries() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/post/api/getPostFull"))
            .and(query_param("post_id", "42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0, "message": "OK", "data": {}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/apihub/api/getShareConf"))
            .and(query_param("entity_id", "42"))
            .and(query_param("entity_type", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0, "message": "OK", "data": {}
            })))
            .mount(&server)
            .await;

        let client = client(&server);
        client.read_post("42", "ds").await.unwrap();
        client.share_post("42", "ds").await.unwrap();
    }

    #[tokio::test]
    async fn like_and_cancel_are_single_post_operations() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/apihub/sapi/upvotePost"))
            .and(body_json(json!({"post_id": "42", "is_cancel": false})))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        assert!(matches!(
            client(&server).set_like_once("42", false, "ds", None).await,
            Err(BbsError::Http(HttpError::Status(
                StatusCode::INTERNAL_SERVER_ERROR
            )))
        ));
    }

    #[tokio::test]
    async fn captcha_is_classified_and_challenge_can_be_submitted() {
        let captcha = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 1034, "message": "captcha", "data": null
            })))
            .mount(&captcha)
            .await;
        assert!(matches!(
            client(&captcha)
                .set_like_once("42", false, "ds", None)
                .await,
            Err(BbsError::CaptchaRequired)
        ));

        let passed = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/apihub/sapi/upvotePost"))
            .and(header("x-rpc-challenge", "passed-challenge"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0, "message": "OK", "data": {}
            })))
            .mount(&passed)
            .await;
        client(&passed)
            .set_like_once("42", false, "ds", Some("passed-challenge"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn forum_sign_is_single_post_with_exact_contract() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/apihub/app/api/signIn"))
            .and(header("ds", "body-ds"))
            .and(body_json(json!({"gids": "2"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0, "message": "OK", "data": {}
            })))
            .expect(1)
            .mount(&server)
            .await;

        client(&server)
            .sign_forum_once("2", "body-ds", None)
            .await
            .unwrap();
    }
}
