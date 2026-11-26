use std::fmt;

const UID_KEYS: &[&str] = &[
    "account_id",
    "ltuid",
    "login_uid",
    "ltuid_v2",
    "account_id_v2",
];
const MID_KEYS: &[&str] = &["account_mid_v2", "ltmid_v2", "mid"];

/// V2 字段存在时，旧名字段应与其保持同值（米游社 App 注入 WebView 的做法）。
const V2_ALIASES: &[(&str, &str)] = &[
    ("ltoken_v2", "ltoken"),
    ("ltuid_v2", "ltuid"),
    ("account_id_v2", "account_id"),
    ("cookie_token_v2", "cookie_token"),
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CookieError {
    MissingEquals { segment: String },
    EmptyName,
}

impl fmt::Display for CookieError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEquals { segment } => {
                write!(formatter, "Cookie 片段缺少等号：{segment}")
            }
            Self::EmptyName => formatter.write_str("Cookie 名称不能为空"),
        }
    }
}

impl std::error::Error for CookieError {}

/// 保留 Cookie 首次出现的顺序；同名项再次出现时用最后一个值覆盖。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CookieJar {
    entries: Vec<(String, String)>,
}

impl CookieJar {
    pub fn parse(header: &str) -> Result<Self, CookieError> {
        let mut jar = Self::default();
        for raw_segment in header.split(';') {
            let segment = raw_segment.trim();
            if segment.is_empty() {
                continue;
            }
            let (name, value) =
                segment
                    .split_once('=')
                    .ok_or_else(|| CookieError::MissingEquals {
                        segment: segment.to_owned(),
                    })?;
            jar.insert(name.trim(), value.trim())?;
        }
        Ok(jar)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(entry_name, _)| entry_name == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn insert(
        &mut self,
        name: impl AsRef<str>,
        value: impl Into<String>,
    ) -> Result<Option<String>, CookieError> {
        let name = name.as_ref().trim();
        if name.is_empty() {
            return Err(CookieError::EmptyName);
        }

        let value = value.into();
        if let Some((_, old_value)) = self
            .entries
            .iter_mut()
            .find(|(entry_name, _)| entry_name == name)
        {
            return Ok(Some(std::mem::replace(old_value, value)));
        }
        self.entries.push((name.to_owned(), value));
        Ok(None)
    }

    pub fn uid(&self) -> Option<&str> {
        self.entries.iter().find_map(|(name, value)| {
            (UID_KEYS.contains(&name.as_str())
                && !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then_some(value.as_str())
        })
    }

    pub fn mid(&self) -> Option<&str> {
        self.entries.iter().find_map(|(name, value)| {
            (MID_KEYS.contains(&name.as_str()) && !value.is_empty()).then_some(value.as_str())
        })
    }

    pub fn login_ticket(&self) -> Option<&str> {
        self.get("login_ticket").filter(|value| !value.is_empty())
    }

    pub fn cookie_token(&self) -> Option<&str> {
        self.get("cookie_token").filter(|value| !value.is_empty())
    }

    /// 返回 `cookie_token_v2`，缺失时回退到 `cookie_token`。
    pub fn any_cookie_token(&self) -> Option<&str> {
        self.get("cookie_token_v2")
            .filter(|value| !value.is_empty())
            .or_else(|| self.cookie_token())
    }

    /// 返回 `ltoken_v2`，缺失时回退到 `ltoken`。
    pub fn any_ltoken(&self) -> Option<&str> {
        ["ltoken_v2", "ltoken"]
            .into_iter()
            .find_map(|name| self.get(name).filter(|value| !value.is_empty()))
    }

    /// 是否使用 V2 命名的凭据：`cookie_token_v2` 或 `ltoken_v2` 的存在即为 V2。
    pub fn uses_v2_tokens(&self) -> bool {
        ["cookie_token_v2", "ltoken_v2", "account_id_v2", "ltuid_v2"]
            .into_iter()
            .any(|name| self.get(name).is_some())
    }

    /// 写入 V1 `cookie_token`，不改动其它字段。
    ///
    /// 保留该入口用于「明确写入 V1 键」的场景；V2 账号应使用
    /// [`Self::replace_cookie_token_value`]。
    pub fn replace_cookie_token(
        &mut self,
        token: impl Into<String>,
    ) -> Result<Option<String>, CookieError> {
        self.insert("cookie_token", token)
    }

    /// 写入刷新后的 CookieToken，并同步 V1/V2 别名。
    ///
    /// 当 Cookie 使用 V2 字段（或调用方要求 `prefer_v2`）时写入 `cookie_token_v2`，
    /// 否则沿用旧键 `cookie_token`。写入后按 [`V2_ALIASES`] 让 V2 字段的旧名保持同值，
    /// 避免残留失效的 V1 旧值。
    pub fn replace_cookie_token_value(
        &mut self,
        token: impl Into<String>,
        prefer_v2: bool,
    ) -> Result<Option<String>, CookieError> {
        let token = token.into();
        let key = if prefer_v2 || self.uses_v2_tokens() {
            "cookie_token_v2"
        } else {
            "cookie_token"
        };
        let previous = self.insert(key, token)?;
        self.sync_aliases();
        Ok(previous)
    }

    /// 写入换票得到的 LToken，并同步 `ltuid_v2` / `ltmid_v2` 与 V1 别名。
    pub fn replace_ltoken(
        &mut self,
        token: impl Into<String>,
        uid: &str,
        mid: Option<&str>,
    ) -> Result<Option<String>, CookieError> {
        let previous = self.insert("ltoken_v2", token)?;
        if !uid.is_empty() {
            self.insert("ltuid_v2", uid)?;
        }
        if let Some(mid) = mid.filter(|value| !value.is_empty()) {
            self.insert("ltmid_v2", mid)?;
        }
        self.sync_aliases();
        Ok(previous)
    }

    /// 按 [`V2_ALIASES`] 把 V1 旧名同步为对应的 V2 值；返回是否有字段被改动。
    pub fn sync_aliases(&mut self) -> bool {
        let mut changed = false;
        for (v2_key, v1_key) in V2_ALIASES {
            let Some(value) = self
                .get(v2_key)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
            else {
                continue;
            };
            if self.get(v1_key) != Some(value.as_str()) {
                // 直接替换值，不改变既有键的位置，也不新增重复键。
                if let Some((_, slot)) = self.entries.iter_mut().find(|(name, _)| name == v1_key) {
                    *slot = value;
                } else {
                    self.entries.push(((*v1_key).to_owned(), value));
                }
                changed = true;
            }
        }
        changed
    }

    pub fn to_header(&self) -> String {
        self.entries
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl fmt::Display for CookieJar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_header())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_cookie_header() {
        let jar = CookieJar::parse(" a=1;; b=two=parts ; c=3; ").unwrap();

        assert_eq!(jar.get("a"), Some("1"));
        assert_eq!(jar.get("b"), Some("two=parts"));
        assert_eq!(jar.to_header(), "a=1; b=two=parts; c=3");
    }

    #[test]
    fn duplicate_names_keep_position_and_last_value() {
        let jar = CookieJar::parse("a=old; b=2; a=new").unwrap();

        assert_eq!(jar.to_header(), "a=new; b=2");
    }

    #[test]
    fn extracts_uid_mid_and_login_ticket_in_cookie_order() {
        let jar = CookieJar::parse(
            "ltuid_v2=123456; account_id=999; account_mid_v2=mid-v2; mid=old; login_ticket=ticket",
        )
        .unwrap();

        assert_eq!(jar.uid(), Some("123456"));
        assert_eq!(jar.mid(), Some("mid-v2"));
        assert_eq!(jar.login_ticket(), Some("ticket"));
    }

    #[test]
    fn rejects_non_numeric_uid_and_malformed_segments() {
        let jar = CookieJar::parse("ltuid=12x; account_id_v2=123").unwrap();
        assert_eq!(jar.uid(), Some("123"));

        assert_eq!(
            CookieJar::parse("valid=1; broken"),
            Err(CookieError::MissingEquals {
                segment: "broken".to_owned()
            })
        );
        assert_eq!(CookieJar::parse("=value"), Err(CookieError::EmptyName));
    }

    #[test]
    fn replaces_or_adds_cookie_token_without_touching_other_values() {
        let mut jar = CookieJar::parse("cookie_token=old; token_copy=old").unwrap();
        assert_eq!(
            jar.replace_cookie_token("new").unwrap(),
            Some("old".to_owned())
        );
        assert_eq!(jar.to_header(), "cookie_token=new; token_copy=old");

        let mut jar = CookieJar::parse("a=1").unwrap();
        assert_eq!(jar.replace_cookie_token("new").unwrap(), None);
        assert_eq!(jar.to_header(), "a=1; cookie_token=new");
    }
}
