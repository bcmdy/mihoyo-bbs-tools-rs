//! 米游社客户端协议数据。
//!
//! 版本号、DS Salt、校验 Key 与客户端类型集中在此处维护，避免米游社升级时
//! 在多个模块中分散修改。客户端版本变化时，通常只需要更新 [`APP_VERSION`]、
//! [`APP_SALT`] 与 [`WEB_SALT`]；默认 User-Agent 由 [`user_agent`] 按版本号生成。
//!
//! `config/config.example.yaml` 与 DaCapo 模板属于随程序分发的数据文件，无法
//! 直接引用本模块常量，测试会校验两者的示例 User-Agent 与 [`user_agent`] 一致。

/// 米游社客户端版本，与 Salt 相互对应。
pub const APP_VERSION: &str = "2.117.0";

/// 米游社 App（客户端类型 [`CLIENT_TYPE_APP`]）使用的 DS Salt。
pub const APP_SALT: &str = "9c7b896d7483e9070b27c55f1959c4d5";

/// 米游社 Web（客户端类型 [`CLIENT_TYPE_WEB`]）使用的 DS Salt。
pub const WEB_SALT: &str = "4751cb5557bfad5e249e58462c39fb0a";

/// 部分接口使用的 Body DS Salt（从 so 提取，一般不变）。
pub const BODY_SALT: &str = "t0qEgfub6cvueAPgR5m9aQWWVciEer7v";

/// 部分接口要求携带的 `x-rpc-verify_key`。
pub const VERIFY_KEY: &str = "bll8iq97cem8";

/// 通行证接口使用的 `x-rpc-app_id`；与 [`VERIFY_KEY`] 取值相同，但语义不同。
pub const APP_ID: &str = VERIFY_KEY;

/// App 客户端类型（`x-rpc-client_type`）。
pub const CLIENT_TYPE_APP: &str = "2";

/// Web 客户端类型（`x-rpc-client_type`）。
pub const CLIENT_TYPE_WEB: &str = "5";

/// 云游戏客户端类型（`x-rpc-client_type`）。
pub const CLIENT_TYPE_CLOUD_GAME: &str = "3";

/// 通行证（passport）接口使用的 DS Salt，一般不变。
pub const PASSPORT_SALT: &str = "JwYDpKvLj6MrMqqYU6jTKF17KNO2PXoS";

/// 通行证 SDK 版本，用于 `x-rpc-sdk_version` 与 `x-rpc-account_version`。
pub const PASSPORT_VERSION: &str = "2.44.0";

/// 通行证接口使用的游戏业务标识（`x-rpc-game_biz`）。
pub const PASSPORT_GAME_BIZ: &str = "bbs_cn";

/// 米游社 App 的 `x-rpc-sys_version`（Android 12）。
pub const SYS_VERSION: &str = "12";

/// 通行证换票的目标 Token 类型：LToken。
pub const TOKEN_TYPE_LTOKEN: u32 = 2;

/// 通行证换票的目标 Token 类型：CookieToken。
pub const TOKEN_TYPE_COOKIE_TOKEN: u32 = 4;

/// 换票请求中 `src_token.token_type` 的取值：SToken。
pub const SRC_TOKEN_TYPE_STOKEN: u32 = 1;

/// 国内米游社 User-Agent 中 `miHoYoBBS/` 之前的前缀。
const USER_AGENT_PREFIX: &str =
    "Mozilla/5.0 (Linux; Android 12) AppleWebKit/537.36 Mobile Safari/537.36 miHoYoBBS/";

/// 返回带当前客户端版本的国内米游社 User-Agent。
pub fn user_agent() -> String {
    format!("{USER_AGENT_PREFIX}{APP_VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salts_are_valid_md5_sized_values() {
        for salt in [APP_SALT, WEB_SALT] {
            assert_eq!(salt.len(), 32, "Salt 长度应为 32：{salt}");
            assert!(
                salt.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "App/Web Salt 应为小写十六进制：{salt}"
            );
        }
        assert_eq!(BODY_SALT.len(), 32, "Body Salt 长度应为 32");
        assert!(BODY_SALT.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(APP_SALT, WEB_SALT);
    }

    #[test]
    fn user_agent_tracks_client_version() {
        assert_eq!(
            user_agent(),
            format!(
                "Mozilla/5.0 (Linux; Android 12) AppleWebKit/537.36 Mobile Safari/537.36 miHoYoBBS/{APP_VERSION}"
            )
        );
    }

    #[test]
    fn distributed_templates_track_user_agent() {
        let agent = user_agent();
        assert!(
            crate::config::EXAMPLE_CONFIG.contains(agent.as_str()),
            "config/config.example.yaml 的示例 User-Agent 未与 APP_VERSION 同步"
        );
        assert!(
            include_str!("../integrations/dacapo/template.yml").contains(agent.as_str()),
            "integrations/dacapo/template.yml 的示例 User-Agent 未与 APP_VERSION 同步"
        );
    }
}
