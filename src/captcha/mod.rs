use reqwest::Url;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::http::{HttpClient, HttpError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptchaSolution {
    pub validate: String,
    pub challenge: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CaptchaRequest<'a> {
    pub gt: &'a str,
    pub challenge: &'a str,
    pub use_v3_model: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct CaptchaResponse {
    #[serde(default)]
    pub data: Option<CaptchaResponseData>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub validate: Option<String>,
    #[serde(default)]
    pub challenge: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct CaptchaResponseData {
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub validate: Option<String>,
    #[serde(default)]
    pub challenge: Option<String>,
}

#[derive(Debug, Error)]
pub enum CaptchaError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("验证码平台返回失败结果: {0}")]
    Rejected(String),
    #[error("验证码平台响应无效: {0}")]
    InvalidResponse(String),
}

#[derive(Clone)]
pub struct CaptchaClient {
    http: HttpClient,
    endpoint: Url,
}

impl CaptchaClient {
    pub fn new(http: HttpClient, endpoint: Url) -> Self {
        Self { http, endpoint }
    }

    /// 调用兼容 pass_nine 的验证码平台。
    ///
    /// 平台可以把 `validate` 和可选的 `challenge` 放在顶层或 `data` 对象内。
    /// 响应未提供新 challenge 时沿用米哈游接口返回的原始 challenge。
    pub async fn solve(
        &self,
        gt: &str,
        challenge: &str,
    ) -> Result<CaptchaSolution, CaptchaError> {
        let request = CaptchaRequest {
            gt,
            challenge,
            use_v3_model: true,
        };
        let response: CaptchaResponse = self
            .http
            .get_json_with(
                self.endpoint.clone(),
                Default::default(),
                &[
                    ("gt", request.gt.to_owned()),
                    ("challenge", request.challenge.to_owned()),
                    ("use_v3_model", request.use_v3_model.to_string()),
                ],
            )
            .await?;
        parse_solution(&response, challenge)
    }
}

fn parse_solution(
    response: &CaptchaResponse,
    original_challenge: &str,
) -> Result<CaptchaSolution, CaptchaError> {
    let result = response
        .data
        .as_ref()
        .and_then(|data| data.result.as_deref())
        .or(response.result.as_deref());
    if let Some(result) = result.filter(|result| !result.eq_ignore_ascii_case("success")) {
        return Err(CaptchaError::Rejected(result.to_owned()));
    }

    let validate = response
        .data
        .as_ref()
        .and_then(|data| data.validate.as_deref())
        .or(response.validate.as_deref())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CaptchaError::InvalidResponse("缺少非空 validate".to_owned()))?;
    let challenge = response
        .data
        .as_ref()
        .and_then(|data| data.challenge.as_deref())
        .or(response.challenge.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(original_challenge);

    Ok(CaptchaSolution {
        validate: validate.to_owned(),
        challenge: challenge.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    use crate::http::{HttpClient, RetryPolicy};

    use super::*;

    fn client(server: &MockServer) -> CaptchaClient {
        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: std::time::Duration::ZERO,
            })
            .build()
            .unwrap();
        CaptchaClient::new(
            http,
            Url::parse(&format!("{}/pass_nine", server.uri())).unwrap(),
        )
    }

    #[tokio::test]
    async fn sends_pass_nine_query_and_parses_nested_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/pass_nine"))
            .and(query_param("gt", "gt-value"))
            .and(query_param("challenge", "original-challenge"))
            .and(query_param("use_v3_model", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {
                    "result": "success",
                    "validate": "validate-value",
                    "challenge": "updated-challenge"
                }
            })))
            .mount(&server)
            .await;

        assert_eq!(
            client(&server)
                .solve("gt-value", "original-challenge")
                .await
                .unwrap(),
            CaptchaSolution {
                validate: "validate-value".to_owned(),
                challenge: "updated-challenge".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn accepts_top_level_json_and_keeps_original_challenge() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/pass_nine"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "validate": "validate-value"
            })))
            .mount(&server)
            .await;

        assert_eq!(
            client(&server)
                .solve("gt-value", "original-challenge")
                .await
                .unwrap()
                .challenge,
            "original-challenge"
        );
    }

    #[tokio::test]
    async fn explicit_failure_and_missing_validate_are_rejected() {
        let failure_server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "result": "failed" }
            })))
            .mount(&failure_server)
            .await;
        assert!(matches!(
            client(&failure_server)
                .solve("gt", "challenge")
                .await,
            Err(CaptchaError::Rejected(result)) if result == "failed"
        ));

        let invalid_server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "result": "success" }
            })))
            .mount(&invalid_server)
            .await;
        assert!(matches!(
            client(&invalid_server)
                .solve("gt", "challenge")
                .await,
            Err(CaptchaError::InvalidResponse(_))
        ));
    }
}
