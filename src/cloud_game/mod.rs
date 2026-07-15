use reqwest::{
    Url,
    header::{ACCEPT, HeaderMap, HeaderName, HeaderValue, USER_AGENT},
};
use serde_json::Value;
use thiserror::Error;

use crate::{
    auth::SecretString,
    http::{HttpClient, HttpError},
};

const CHINA_GENSHIN_URL: &str = "https://api-cloudgame.mihoyo.com/hk4e_cg_cn/wallet/wallet/get";
const CHINA_ZZZ_URL: &str = "https://cg-nap-api.mihoyo.com/nap_cn/cg/wallet/wallet/get";
const OVERSEAS_GENSHIN_URL: &str =
    "https://sg-cg-api.hoyoverse.com/hk4e_global/cg/wallet/wallet/get";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloudGame {
    ChinaGenshin,
    ChinaZenlessZoneZero,
    OverseasGenshin,
}

impl CloudGame {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::ChinaGenshin | Self::OverseasGenshin => "云原神",
            Self::ChinaZenlessZoneZero => "云绝区零",
        }
    }

    pub const fn coin_name(self) -> &'static str {
        match self {
            Self::ChinaZenlessZoneZero => "邦邦点",
            Self::ChinaGenshin | Self::OverseasGenshin => "米云币",
        }
    }

    const fn production_url(self) -> &'static str {
        match self {
            Self::ChinaGenshin => CHINA_GENSHIN_URL,
            Self::ChinaZenlessZoneZero => CHINA_ZZZ_URL,
            Self::OverseasGenshin => OVERSEAS_GENSHIN_URL,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloudWallet {
    pub sent_minutes: u64,
    pub total_minutes: u64,
    pub play_card: String,
    pub coins: u64,
}

#[derive(Debug, Error)]
pub enum CloudGameError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("云游戏请求头 {0} 包含无效字符")]
    InvalidHeader(&'static str),
    #[error("云游戏 Token 无效或账号受防沉迷限制")]
    TokenInvalid,
    #[error("云游戏接口错误 {retcode}：{message}")]
    Api { retcode: i64, message: String },
    #[error("云游戏接口响应缺少字段 {0}")]
    InvalidResponse(&'static str),
}

#[derive(Clone)]
pub struct CloudGameClient {
    http: HttpClient,
    endpoint_override: Option<Url>,
}

impl CloudGameClient {
    pub fn new(http: HttpClient) -> Self {
        Self {
            http,
            endpoint_override: None,
        }
    }

    #[doc(hidden)]
    pub fn endpoint_override(mut self, endpoint: Url) -> Self {
        self.endpoint_override = Some(endpoint);
        self
    }

    pub async fn wallet(
        &self,
        game: CloudGame,
        token: &SecretString,
        language: &str,
    ) -> Result<CloudWallet, CloudGameError> {
        let endpoint = match &self.endpoint_override {
            Some(endpoint) => endpoint.clone(),
            None => Url::parse(game.production_url()).expect("固定云游戏接口 URL 应当始终有效"),
        };
        let response: Value = self
            .http
            .get_json_with(endpoint, headers(game, token, language)?, &[])
            .await?;
        parse_wallet(&response)
    }
}

fn headers(
    game: CloudGame,
    token: &SecretString,
    language: &str,
) -> Result<HeaderMap, CloudGameError> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
    insert(&mut headers, "x-rpc-combo_token", token.expose_secret())?;
    match game {
        CloudGame::ChinaGenshin => {
            headers.insert(
                USER_AGENT,
                HeaderValue::from_static(
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/99.0.4844.84 Safari/537.36",
                ),
            );
            headers.insert(
                "referer",
                HeaderValue::from_static("https://app.mihoyo.com"),
            );
        }
        CloudGame::ChinaZenlessZoneZero => {
            headers.insert(
                USER_AGENT,
                HeaderValue::from_static(
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/99.0.4844.84 Safari/537.36",
                ),
            );
        }
        CloudGame::OverseasGenshin => {
            headers.insert(USER_AGENT, HeaderValue::from_static("okhttp/4.10.0"));
            headers.insert("x-rpc-client_type", HeaderValue::from_static("3"));
            headers.insert("x-rpc-cg_game_biz", HeaderValue::from_static("hk4e_global"));
            headers.insert("x-rpc-channel_id", HeaderValue::from_static("1"));
            insert(&mut headers, "x-rpc-language", language)?;
        }
    }
    Ok(headers)
}

fn insert(headers: &mut HeaderMap, name: &'static str, value: &str) -> Result<(), CloudGameError> {
    let value = HeaderValue::from_str(value).map_err(|_| CloudGameError::InvalidHeader(name))?;
    headers.insert(HeaderName::from_static(name), value);
    Ok(())
}

fn parse_wallet(value: &Value) -> Result<CloudWallet, CloudGameError> {
    let retcode = value
        .get("retcode")
        .and_then(Value::as_i64)
        .ok_or(CloudGameError::InvalidResponse("retcode"))?;
    if retcode == -100 {
        return Err(CloudGameError::TokenInvalid);
    }
    if retcode != 0 {
        return Err(CloudGameError::Api {
            retcode,
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未知错误")
                .to_owned(),
        });
    }
    let data = value
        .get("data")
        .ok_or(CloudGameError::InvalidResponse("data"))?;
    Ok(CloudWallet {
        sent_minutes: number(data.pointer("/free_time/send_freetime"), "send_freetime")?,
        total_minutes: number(data.pointer("/free_time/free_time"), "free_time")?,
        play_card: data
            .pointer("/play_card/short_msg")
            .and_then(Value::as_str)
            .unwrap_or("未知")
            .to_owned(),
        coins: number(data.pointer("/coin/coin_num"), "coin_num")?,
    })
}

fn number(value: Option<&Value>, field: &'static str) -> Result<u64, CloudGameError> {
    value
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|raw| raw.parse().ok()))
        })
        .ok_or(CloudGameError::InvalidResponse(field))
}

#[cfg(test)]
mod tests {
    use reqwest::Url;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    use crate::http::{HttpClient, RetryPolicy};

    use super::*;

    fn client(server: &MockServer) -> CloudGameClient {
        let http = HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: std::time::Duration::ZERO,
            })
            .build()
            .unwrap();
        CloudGameClient::new(http).endpoint_override(Url::parse(&server.uri()).unwrap())
    }

    #[tokio::test]
    async fn parses_wallet_and_sends_overseas_headers() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(header("x-rpc-combo_token", "combo-secret"))
            .and(header("x-rpc-language", "zh-cn"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "retcode": 0,
                "data": {
                    "free_time": {"send_freetime": "15", "free_time": 120},
                    "play_card": {"short_msg": "未开通"},
                    "coin": {"coin_num": "7"}
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let wallet = client(&server)
            .wallet(
                CloudGame::OverseasGenshin,
                &SecretString::new("combo-secret"),
                "zh-cn",
            )
            .await
            .unwrap();
        assert_eq!(wallet.sent_minutes, 15);
        assert_eq!(wallet.total_minutes, 120);
        assert_eq!(wallet.coins, 7);
    }

    #[test]
    fn token_invalid_is_classified() {
        assert!(matches!(
            parse_wallet(&json!({"retcode": -100, "message": "invalid"})),
            Err(CloudGameError::TokenInvalid)
        ));
    }

    #[test]
    fn production_urls_and_domestic_headers_match_cloud_services() {
        assert_eq!(
            CloudGame::ChinaGenshin.production_url(),
            "https://api-cloudgame.mihoyo.com/hk4e_cg_cn/wallet/wallet/get"
        );
        assert_eq!(
            CloudGame::ChinaZenlessZoneZero.production_url(),
            "https://cg-nap-api.mihoyo.com/nap_cn/cg/wallet/wallet/get"
        );
        assert_eq!(
            CloudGame::OverseasGenshin.production_url(),
            "https://sg-cg-api.hoyoverse.com/hk4e_global/cg/wallet/wallet/get"
        );

        let token = SecretString::new("domestic-token");
        let genshin = headers(CloudGame::ChinaGenshin, &token, "zh-cn").unwrap();
        assert_eq!(genshin["x-rpc-combo_token"], "domestic-token");
        assert_eq!(genshin["referer"], "https://app.mihoyo.com");
        assert!(
            genshin["user-agent"]
                .to_str()
                .unwrap()
                .contains("Chrome/99")
        );

        let zzz = headers(CloudGame::ChinaZenlessZoneZero, &token, "zh-cn").unwrap();
        assert_eq!(zzz["x-rpc-combo_token"], "domestic-token");
        assert!(!zzz.contains_key("referer"));
    }
}
