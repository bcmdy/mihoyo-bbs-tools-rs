use std::fmt;

use super::{CookieError, CookieJar};

const REDACTED: &str = "[REDACTED]";

#[derive(Clone, Default, Eq, PartialEq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REDACTED)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialError {
    Cookie(CookieError),
    MissingCookie,
    MissingStoken,
    MissingUid,
    MidRequired,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cookie(error) => write!(formatter, "Cookie 无效：{error}"),
            Self::MissingCookie => formatter.write_str("缺少 Cookie"),
            Self::MissingStoken => formatter.write_str("缺少 SToken"),
            Self::MissingUid => formatter.write_str("Cookie 中缺少 UID"),
            Self::MidRequired => formatter.write_str("v2 SToken 必须同时提供 MID"),
        }
    }
}

impl std::error::Error for CredentialError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cookie(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CookieError> for CredentialError {
    fn from(value: CookieError) -> Self {
        Self::Cookie(value)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Credentials {
    pub cookie: SecretString,
    pub stoken: SecretString,
    pub stuid: Option<String>,
    pub mid: Option<String>,
}

impl Credentials {
    pub fn new(cookie: impl Into<SecretString>, stoken: impl Into<SecretString>) -> Self {
        Self {
            cookie: cookie.into(),
            stoken: stoken.into(),
            stuid: None,
            mid: None,
        }
    }

    pub fn hydrate_from_cookie(&mut self) -> Result<(), CredentialError> {
        if self.cookie.is_empty() {
            return Err(CredentialError::MissingCookie);
        }
        if self.stoken.is_empty() {
            return Err(CredentialError::MissingStoken);
        }

        let jar = CookieJar::parse(self.cookie.expose_secret())?;
        self.stuid = Some(jar.uid().ok_or(CredentialError::MissingUid)?.to_owned());
        if self.requires_mid() {
            self.mid = self
                .mid
                .take()
                .or_else(|| jar.mid().map(str::to_owned))
                .ok_or(CredentialError::MidRequired)
                .map(Some)?;
        }
        Ok(())
    }

    pub fn requires_mid(&self) -> bool {
        self.stoken.expose_secret().starts_with("v2_")
    }

    pub fn stoken_cookie(&self) -> Result<SecretString, CredentialError> {
        if self.stoken.is_empty() {
            return Err(CredentialError::MissingStoken);
        }
        let stuid = self.stuid.as_deref().ok_or(CredentialError::MissingUid)?;
        let mut value = format!("stuid={stuid};stoken={}", self.stoken.expose_secret());
        if self.requires_mid() {
            let mid = self.mid.as_deref().ok_or(CredentialError::MidRequired)?;
            value.push_str(";mid=");
            value.push_str(mid);
        }
        Ok(SecretString::new(value))
    }

    pub fn replace_cookie_token(&mut self, token: &str) -> Result<(), CredentialError> {
        let mut jar = CookieJar::parse(self.cookie.expose_secret())?;
        jar.replace_cookie_token(token)?;
        self.cookie = SecretString::new(jar.to_header());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_in_debug_and_display() {
        let secret = SecretString::new("do-not-log-me");
        assert_eq!(format!("{secret}"), "[REDACTED]");
        assert_eq!(format!("{secret:?}"), "SecretString([REDACTED])");

        let credentials = Credentials::new("cookie=secret", "stoken-secret");
        let debug = format!("{credentials:?}");
        assert!(!debug.contains("cookie=secret"));
        assert!(!debug.contains("stoken-secret"));
    }

    #[test]
    fn hydrates_v2_credentials_and_builds_stoken_cookie() {
        let mut credentials = Credentials::new(
            "account_id_v2=123; account_mid_v2=mid-value; cookie_token=old",
            "v2_token",
        );
        credentials.hydrate_from_cookie().unwrap();

        assert_eq!(credentials.stuid.as_deref(), Some("123"));
        assert_eq!(credentials.mid.as_deref(), Some("mid-value"));
        assert_eq!(
            credentials.stoken_cookie().unwrap().expose_secret(),
            "stuid=123;stoken=v2_token;mid=mid-value"
        );
    }

    #[test]
    fn legacy_stoken_does_not_require_mid() {
        let mut credentials = Credentials::new("ltuid=456", "legacy-token");
        credentials.hydrate_from_cookie().unwrap();

        assert_eq!(credentials.mid, None);
        assert_eq!(
            credentials.stoken_cookie().unwrap().expose_secret(),
            "stuid=456;stoken=legacy-token"
        );
    }

    #[test]
    fn v2_stoken_rejects_missing_mid() {
        let mut credentials = Credentials::new("ltuid_v2=123", "v2_token");
        assert_eq!(
            credentials.hydrate_from_cookie(),
            Err(CredentialError::MidRequired)
        );
    }

    #[test]
    fn v2_stoken_accepts_explicit_mid() {
        let mut credentials = Credentials::new("ltuid_v2=123", "v2_token");
        credentials.mid = Some("configured-mid".to_owned());
        credentials.hydrate_from_cookie().unwrap();

        assert_eq!(credentials.mid.as_deref(), Some("configured-mid"));
    }

    #[test]
    fn token_replacement_is_cookie_aware() {
        let mut credentials = Credentials::new(
            "cookie_token=old; token_copy=old; account_id=123",
            "token",
        );
        credentials.replace_cookie_token("new").unwrap();

        assert_eq!(
            credentials.cookie.expose_secret(),
            "cookie_token=new; token_copy=old; account_id=123"
        );
    }
}
