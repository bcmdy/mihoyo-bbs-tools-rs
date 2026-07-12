use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ApiEnvelope {
    pub retcode: i64,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub data: Value,
}

impl ApiEnvelope {
    pub(crate) fn decode<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.data.clone())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct GameRole {
    #[serde(default)]
    pub nickname: String,
    #[serde(rename = "game_uid")]
    pub uid: String,
    pub region: String,
    #[serde(default)]
    pub level: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoleState {
    NoRole,
    Available(Vec<GameRole>),
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RoleListData {
    #[serde(default)]
    pub list: Vec<GameRole>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CheckinInfoData {
    #[serde(default)]
    pub total_sign_day: u32,
    #[serde(default)]
    pub is_sign: bool,
    #[serde(default)]
    pub first_bind: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckinState {
    FirstBind,
    Pending { total_sign_day: u32 },
    AlreadySigned { total_sign_day: u32 },
}

impl From<CheckinInfoData> for CheckinState {
    fn from(value: CheckinInfoData) -> Self {
        if value.first_bind {
            Self::FirstBind
        } else if value.is_sign {
            Self::AlreadySigned {
                total_sign_day: value.total_sign_day,
            }
        } else {
            Self::Pending {
                total_sign_day: value.total_sign_day,
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SignData {
    #[serde(default)]
    pub success: u8,
    #[serde(default)]
    pub gt: String,
    #[serde(default)]
    pub challenge: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignState {
    Success,
    AlreadySigned,
    CaptchaRequired { gt: String, challenge: String },
}

pub(crate) fn is_cookie_invalid(retcode: i64, message: &str) -> bool {
    matches!(retcode, -100 | -101 | -10001 | -10002)
        || message.to_ascii_lowercase().contains("cookie")
        || message.contains("登录失效")
        || message.contains("尚未登录")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_first_bind_before_other_flags() {
        let state = CheckinState::from(CheckinInfoData {
            total_sign_day: 1,
            is_sign: true,
            first_bind: true,
        });
        assert_eq!(state, CheckinState::FirstBind);
    }

    #[test]
    fn recognizes_cookie_errors_without_exposing_payloads() {
        assert!(is_cookie_invalid(-100, ""));
        assert!(is_cookie_invalid(1, "Cookie not found"));
        assert!(is_cookie_invalid(1, "登录失效，请重新登录"));
        assert!(!is_cookie_invalid(-5003, "already signed"));
    }
}
