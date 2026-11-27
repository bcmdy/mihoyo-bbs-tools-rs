//! QQ 机器人单聊 OpenID 获取工具。
//!
//! 通过 QQ 开放平台的 `getAppAccessToken` 换取访问令牌，再连接其 WebSocket
//! 网关，监听 `C2C_MESSAGE_CREATE` 事件取出 `author.user_openid`，最后写回配置。
//!
//! 设计取舍：
//!
//! - 复用 [`HttpClient`] 完成 HTTP 部分，沿用项目既有的代理、TLS 与超时策略；
//!   需要 HTTP upgrade 时用 `allow_upgrade` 手工构造请求，不引入 WebSocket 生态。
//! - 不新增依赖：SHA-1 只为校验 `Sec-WebSocket-Accept`，这里内置最小实现，
//!   避免为单个握手校验改变 `Cargo.lock`（CI 使用 `--locked` 构建）。
//! - 网关会周期性要求心跳，且可能主动断开要求重连；本模块按截止时间重连，
//!   与上游脚本行为一致。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::STANDARD};
use rand::Rng;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    auth::SecretString,
    config::default_qbot_api_url,
    http::{HttpClient, HttpError},
};

/// 单聊消息事件的 intent 位。`1 << 25` 表示只订阅 C2C 单聊消息。
const C2C_INTENT: u64 = 1 << 25;
/// WebSocket 握手固定 GUID（RFC 6455）。
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// 单条 WebSocket 帧允许的最大载荷，避免异常连接耗尽内存。
const MAX_FRAME_PAYLOAD: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub enum QbotOpenIdError {
    Http(HttpError),
    /// 开放平台返回的业务错误。
    ServiceRejected {
        stage: &'static str,
        code: i64,
    },
    /// 响应缺少必要字段。
    MissingField(&'static str),
    /// WebSocket 握手或帧处理失败。
    WebSocket(String),
    /// 等待用户在截止时间前发送单聊消息超时。
    Timeout,
    /// 网关要求重连，由外层循环处理。
    ReconnectRequested,
}

impl std::fmt::Display for QbotOpenIdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(error) => write!(formatter, "网络请求失败：{error}"),
            Self::ServiceRejected { stage, code } => {
                write!(formatter, "{stage}被拒绝（代码 {code}）")
            }
            Self::MissingField(field) => write!(formatter, "响应缺少字段 {field}"),
            Self::WebSocket(detail) => write!(formatter, "WebSocket 通信失败：{detail}"),
            Self::Timeout => formatter.write_str("等待单聊消息超时"),
            Self::ReconnectRequested => formatter.write_str("网关要求重新连接"),
        }
    }
}

impl std::error::Error for QbotOpenIdError {}

impl From<HttpError> for QbotOpenIdError {
    fn from(value: HttpError) -> Self {
        Self::Http(value)
    }
}

/// 备用 access_token 端点：部分实现使用独立域名。
const FALLBACK_TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";

/// 发送一次 access_token 请求。
async fn request_access_token(
    http: &HttpClient,
    url: reqwest::Url,
    body: &serde_json::Value,
) -> Result<AccessTokenResponse, QbotOpenIdError> {
    Ok(http
        .post_json_once(url, reqwest::header::HeaderMap::new(), body)
        .await?)
}

#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    /// 官方示例返回字符串 `"7200"`，实际也可能是数字，两种都要接受。
    #[serde(default, deserialize_with = "deserialize_optional_seconds")]
    expires_in: Option<i64>,
    #[serde(default)]
    code: Option<i64>,
}

/// 接受字符串或数字形式的秒数。
fn deserialize_optional_seconds<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::Number(number)) => number.as_i64(),
        Some(serde_json::Value::String(text)) => text.trim().parse::<i64>().ok(),
        _ => None,
    })
}

#[derive(Debug, Deserialize)]
struct GatewayResponse {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    code: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct GatewayPayload {
    #[serde(default)]
    op: i64,
    #[serde(default)]
    s: Option<i64>,
    #[serde(default)]
    t: Option<String>,
    #[serde(default)]
    d: Option<serde_json::Value>,
}

/// 获取结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QbotOpenId {
    pub openid: String,
    /// 令牌剩余有效秒数；仅用于提示用户，不做缓存。
    pub expires_in: i64,
}

/// 用 AppID/AppSecret 换取 access_token。
/// 换取 access_token。
///
/// 官方文档把该接口列在 `api.bot.qq.com` 下，社区实现（如 `HCHogan/qqbot`）则使用
/// 独立的 `bots.qq.com`。两者都被官方环境接受：这里以配置地址为主，网络层失败时
/// 再试备用域名，避免单一域名不可达让命令完全不可用；业务错误（`code` 非 0）不重试，
/// 因为换域名不会改变鉴权结果。
async fn fetch_access_token(
    http: &HttpClient,
    api_url: &reqwest::Url,
    appid: &str,
    secret: &SecretString,
) -> Result<(String, i64), QbotOpenIdError> {
    let body = serde_json::json!({
        "appId": appid,
        "clientSecret": secret.expose_secret(),
    });
    let primary = api_url
        .join("app/getAppAccessToken")
        .map_err(HttpError::InvalidUrl)?;
    let response: AccessTokenResponse = match request_access_token(http, primary, &body).await {
        Ok(response) => response,
        Err(primary_error) => {
            let fallback =
                reqwest::Url::parse(FALLBACK_TOKEN_URL).map_err(HttpError::InvalidUrl)?;
            match request_access_token(http, fallback, &body).await {
                Ok(response) => response,
                // 两个域名都不可达时，报告主域名的错误更有助排查。
                Err(_) => return Err(primary_error),
            }
        }
    };
    if let Some(code) = response.code.filter(|code| *code != 0) {
        return Err(QbotOpenIdError::ServiceRejected {
            stage: "获取 access_token ",
            code,
        });
    }
    let token = response
        .access_token
        .filter(|token| !token.is_empty())
        .ok_or(QbotOpenIdError::MissingField("access_token"))?;
    Ok((token, response.expires_in.unwrap_or(0)))
}

/// 用 access_token 换取 WebSocket 网关地址。
async fn fetch_gateway(
    http: &HttpClient,
    api_url: &reqwest::Url,
    access_token: &str,
) -> Result<String, QbotOpenIdError> {
    let url = api_url.join("gateway").map_err(HttpError::InvalidUrl)?;
    // 网关请求需要 Authorization 头；HttpClient 的 GET 封装不带自定义头，
    // 这里复用 `get_json_once_with` 显式传入。
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&format!("QQBot {access_token}"))
            .map_err(|_| QbotOpenIdError::WebSocket("Authorization 头无效".to_owned()))?,
    );
    let response: GatewayResponse = http.get_json_once_with(url, headers, &[]).await?;
    if let Some(code) = response.code.filter(|code| *code != 0) {
        return Err(QbotOpenIdError::ServiceRejected {
            stage: "获取 WebSocket 网关",
            code,
        });
    }
    response
        .url
        .filter(|url| !url.is_empty())
        .ok_or(QbotOpenIdError::MissingField("url"))
}

/// 当前 Unix 秒。
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 计算 `Sec-WebSocket-Accept`。
///
/// 只需要 SHA-1，这里内置最小实现：为一次握手校验引入 `sha1` 依赖会改变
/// `Cargo.lock`，而 CI 使用 `--locked` 构建。
fn websocket_accept(sec_key: &str) -> String {
    let mut input = sec_key.as_bytes().to_vec();
    input.extend_from_slice(WEBSOCKET_GUID.as_bytes());
    STANDARD.encode(sha1(&input))
}

/// 最小 SHA-1 实现（FIPS 180-4），只用于 WebSocket 握手校验。
///
/// 消息扩展天然是「按下标读写同一数组」，改成迭代器会显著降低可读性，
/// 因此这里显式允许 `needless_range_loop`。
#[allow(clippy::needless_range_loop)]
fn sha1(input: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut message = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    // 逐块处理：用索引而不是 `chunks_exact`，既避开 clippy 的
    // `chunks_exact_to_as_chunks` 建议（其替代 `as_chunks` 在部分版本不可用），
    // 也避免为常量分块尺寸引入额外 lint 抑制。
    let block_count = message.len() / 64;
    for block in 0..block_count {
        let mut w = [0u32; 80];
        for index in 0..16 {
            let offset = block * 64 + index * 4;
            w[index] = u32::from_be_bytes([
                message[offset],
                message[offset + 1],
                message[offset + 2],
                message[offset + 3],
            ]);
        }
        for index in 16..80 {
            w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (index, word) in w.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut output = [0u8; 20];
    for (index, value) in h.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    output
}

/// 一次帧解析的结果。
struct ParsedFrame {
    opcode: u8,
    payload: Vec<u8>,
    /// 本帧在缓冲区中占用的字节数。
    consumed: usize,
}

/// 从字节缓冲解析一个 WebSocket 帧，返回帧与已消费的字节数。
///
/// 数据不完整时返回 `Ok(None)` 且不消费；抽出为纯函数便于在不建立连接的情况下
/// 覆盖长度编码、掩码与分片边界。
fn parse_frame_bytes(buffer: &[u8]) -> Result<Option<ParsedFrame>, QbotOpenIdError> {
    if buffer.len() < 2 {
        return Ok(None);
    }
    let opcode = buffer[0] & 0x0F;
    let masked = buffer[1] & 0x80 != 0;
    let mut length = (buffer[1] & 0x7F) as usize;
    let mut offset = 2;

    if length == 126 {
        if buffer.len() < offset + 2 {
            return Ok(None);
        }
        length = u16::from_be_bytes([buffer[offset], buffer[offset + 1]]) as usize;
        offset += 2;
    } else if length == 127 {
        if buffer.len() < offset + 8 {
            return Ok(None);
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&buffer[offset..offset + 8]);
        length = usize::try_from(u64::from_be_bytes(bytes))
            .map_err(|_| QbotOpenIdError::WebSocket("帧长度溢出".to_owned()))?;
        offset += 8;
    }
    if length > MAX_FRAME_PAYLOAD {
        return Err(QbotOpenIdError::WebSocket("帧过大".to_owned()));
    }

    let mask = if masked {
        if buffer.len() < offset + 4 {
            return Ok(None);
        }
        let mut mask = [0u8; 4];
        mask.copy_from_slice(&buffer[offset..offset + 4]);
        offset += 4;
        Some(mask)
    } else {
        None
    };
    if buffer.len() < offset + length {
        return Ok(None);
    }

    let mut payload = buffer[offset..offset + length].to_vec();
    if let Some(mask) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok(Some(ParsedFrame {
        opcode,
        payload,
        consumed: offset + length,
    }))
}

/// 一次 WebSocket 会话：握手后等待 `C2C_MESSAGE_CREATE` 事件。
struct GatewaySession {
    stream: reqwest::Upgraded,
    buffer: Vec<u8>,
}

impl GatewaySession {
    /// 建立到网关的 WebSocket 连接。
    ///
    /// 握手通过 HTTP upgrade 完成：`Connection: Upgrade` + `Upgrade: websocket`
    /// 等头由本方法显式设置，因此可以复用项目既有的代理与 TLS 配置。
    async fn connect(http: &HttpClient, gateway_url: &str) -> Result<Self, QbotOpenIdError> {
        let mut key_bytes = [0u8; 16];
        rand::rng().fill(&mut key_bytes);
        let sec_key = STANDARD.encode(key_bytes);

        let url = reqwest::Url::parse(gateway_url)
            .map_err(|_| QbotOpenIdError::WebSocket("网关地址无效".to_owned()))?;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONNECTION,
            reqwest::header::HeaderValue::from_static("Upgrade"),
        );
        headers.insert(
            reqwest::header::UPGRADE,
            reqwest::header::HeaderValue::from_static("websocket"),
        );
        headers.insert(
            reqwest::header::SEC_WEBSOCKET_VERSION,
            reqwest::header::HeaderValue::from_static("13"),
        );
        headers.insert(
            reqwest::header::SEC_WEBSOCKET_KEY,
            reqwest::header::HeaderValue::from_str(&sec_key)
                .map_err(|_| QbotOpenIdError::WebSocket("Sec-WebSocket-Key 无效".to_owned()))?,
        );

        let response = http.get_upgrade_once(url, headers).await?;
        // 协议要求校验服务端回显的 accept 值，避免连到非 WebSocket 端点。
        let accept = response
            .headers()
            .get("sec-websocket-accept")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        if accept != websocket_accept(&sec_key) {
            return Err(QbotOpenIdError::WebSocket(
                "WebSocket 握手校验失败".to_owned(),
            ));
        }
        let upgraded = response
            .upgrade()
            .await
            .map_err(|_| QbotOpenIdError::WebSocket("HTTP upgrade 失败".to_owned()))?;
        Ok(Self {
            stream: upgraded,
            buffer: Vec::new(),
        })
    }

    /// 读取一个完整文本帧。
    async fn read_text(&mut self) -> Result<String, QbotOpenIdError> {
        loop {
            let (opcode, payload) = self.read_frame().await?;
            match opcode {
                // 文本帧；网关事件都是 JSON 文本。
                0x1 => {
                    return String::from_utf8(payload)
                        .map_err(|_| QbotOpenIdError::WebSocket("帧不是有效 UTF-8".to_owned()));
                }
                // ping 要求回 pong。
                0x9 => {
                    self.write_frame(0xA, &payload).await?;
                }
                // pong 或其它控制帧忽略。
                0xA => {}
                0x8 => return Err(QbotOpenIdError::ReconnectRequested),
                _ => {}
            }
        }
    }

    /// 读取一帧（处理 126/127 长度与服务端掩码）。
    async fn read_frame(&mut self) -> Result<(u8, Vec<u8>), QbotOpenIdError> {
        loop {
            if let Some(frame) = self.parse_frame()? {
                return Ok(frame);
            }
            let mut chunk = [0u8; 4096];
            let read = self
                .stream
                .read(&mut chunk)
                .await
                .map_err(|error| QbotOpenIdError::WebSocket(format!("读取失败：{error}")))?;
            if read == 0 {
                return Err(QbotOpenIdError::ReconnectRequested);
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }

    /// 尝试从缓冲区解析出一帧；数据不足时返回 `None`。
    fn parse_frame(&mut self) -> Result<Option<(u8, Vec<u8>)>, QbotOpenIdError> {
        let Some(frame) = parse_frame_bytes(&self.buffer)? else {
            return Ok(None);
        };
        self.buffer.drain(..frame.consumed);
        Ok(Some((frame.opcode, frame.payload)))
    }

    /// 发送一个客户端帧（必须加掩码）。
    async fn write_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), QbotOpenIdError> {
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(0x80 | opcode);
        if payload.len() < 126 {
            frame.push(0x80 | payload.len() as u8);
        } else if payload.len() < 65_536 {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
        let mut mask = [0u8; 4];
        rand::rng().fill(&mut mask);
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % 4]),
        );
        self.stream
            .write_all(&frame)
            .await
            .map_err(|error| QbotOpenIdError::WebSocket(format!("写入失败：{error}")))?;
        self.stream
            .flush()
            .await
            .map_err(|error| QbotOpenIdError::WebSocket(format!("刷新失败：{error}")))?;
        Ok(())
    }

    /// 发送 JSON 文本。
    async fn send_json(&mut self, value: &serde_json::Value) -> Result<(), QbotOpenIdError> {
        self.write_frame(0x1, value.to_string().as_bytes()).await
    }
}

/// 单个连接的等待结果。
enum SessionOutcome {
    /// 收到单聊消息，取得 OpenID。
    OpenId(String),
    /// 截止时间内未收到消息。
    TimedOut,
}

/// 在单个连接上完成鉴权并等待单聊消息。
///
/// 返回 `Err` 表示连接级失败（由外层重连），`Ok` 表示本次会话正常结束。
async fn wait_on_session(
    session: &mut GatewaySession,
    access_token: &str,
    deadline: u64,
) -> Result<SessionOutcome, QbotOpenIdError> {
    let mut heartbeat = 45u64;
    let mut last_heartbeat = unix_now();
    let mut last_seq: Option<i64> = None;

    while unix_now() < deadline {
        let payload = match tokio::time::timeout(Duration::from_secs(5), session.read_text()).await
        {
            Ok(Ok(text)) => text,
            // 读超时只是为了周期检查心跳与截止时间，不是错误。
            Err(_) => String::new(),
            Ok(Err(QbotOpenIdError::ReconnectRequested)) => {
                return Err(QbotOpenIdError::ReconnectRequested);
            }
            Ok(Err(error)) => return Err(error),
        };

        if !payload.is_empty() {
            let frame: GatewayPayload = serde_json::from_str(&payload)
                .map_err(|_| QbotOpenIdError::WebSocket("网关事件不是有效 JSON".to_owned()))?;
            if let Some(seq) = frame.s {
                last_seq = Some(seq);
            }
            match frame.op {
                // HELLO：带心跳间隔，要求立即鉴权。
                10 => {
                    heartbeat = frame
                        .d
                        .as_ref()
                        .and_then(|data| data.get("heartbeat_interval"))
                        .and_then(serde_json::Value::as_u64)
                        .map(|millis| (millis / 1000).max(1))
                        .unwrap_or(45);
                    session
                        .send_json(&serde_json::json!({
                            "op": 2,
                            "d": {
                                "token": format!("QQBot {access_token}"),
                                "intents": C2C_INTENT,
                                "shard": [0, 1],
                                "properties": {},
                            }
                        }))
                        .await?;
                    last_heartbeat = unix_now();
                }
                // 服务端事件派发。
                0 => {
                    if frame.t.as_deref() == Some("C2C_MESSAGE_CREATE")
                        && let Some(openid) = frame
                            .d
                            .as_ref()
                            .and_then(|data| data.get("author"))
                            .and_then(|author| author.get("user_openid"))
                            .and_then(serde_json::Value::as_str)
                            .filter(|openid| !openid.is_empty())
                    {
                        return Ok(SessionOutcome::OpenId(openid.to_owned()));
                    }
                }
                // 要求重连。
                7 | 9 => return Err(QbotOpenIdError::ReconnectRequested),
                _ => {}
            }
        }

        if unix_now().saturating_sub(last_heartbeat) >= heartbeat {
            session
                .send_json(&serde_json::json!({ "op": 1, "d": last_seq }))
                .await?;
            last_heartbeat = unix_now();
        }
    }
    Ok(SessionOutcome::TimedOut)
}

/// 获取单聊 OpenID：换取令牌、连接网关并等待用户发送消息。
///
/// `timeout_seconds` 是等待用户发送单聊消息的总时长；网关异常断开会在截止时间前
/// 自动重连。返回 [`QbotOpenId`] 时用户已经成功与机器人建立过单聊会话。
pub async fn fetch_open_id(
    http: &HttpClient,
    appid: &str,
    secret: &SecretString,
    api_url: Option<reqwest::Url>,
    timeout_seconds: u64,
) -> Result<QbotOpenId, QbotOpenIdError> {
    if appid.trim().is_empty() || secret.is_empty() {
        return Err(QbotOpenIdError::MissingField("appid/secret"));
    }
    let api_url = api_url.unwrap_or_else(default_qbot_api_url);
    let (access_token, expires_in) = fetch_access_token(http, &api_url, appid, secret).await?;
    let gateway = fetch_gateway(http, &api_url, &access_token).await?;

    let deadline = unix_now().saturating_add(timeout_seconds.max(1));
    let mut last_error = None;
    while unix_now() < deadline {
        match GatewaySession::connect(http, &gateway).await {
            Ok(mut session) => match wait_on_session(&mut session, &access_token, deadline).await {
                Ok(SessionOutcome::OpenId(openid)) => {
                    return Ok(QbotOpenId { openid, expires_in });
                }
                Ok(SessionOutcome::TimedOut) => last_error = Some(QbotOpenIdError::Timeout),
                Err(error) => last_error = Some(error),
            },
            Err(error) => last_error = Some(error),
        }
        // 重连前短暂等待，避免服务端拒绝时忙循环。
        let remaining = deadline.saturating_sub(unix_now());
        if remaining == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2).min(Duration::from_secs(remaining))).await;
    }

    Err(last_error.unwrap_or(QbotOpenIdError::Timeout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_known_vectors() {
        // FIPS 180-4 与经典测试向量。
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(b"The quick brown fox jumps over the lazy dog")),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
        // 跨 64 字节块边界，覆盖填充与长度编码。
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn access_token_accepts_string_and_numeric_expiry() {
        let from_string: AccessTokenResponse =
            serde_json::from_str(r#"{"access_token":"t","expires_in":"7200"}"#).unwrap();
        assert_eq!(from_string.expires_in, Some(7200));

        let from_number: AccessTokenResponse =
            serde_json::from_str(r#"{"access_token":"t","expires_in":7200}"#).unwrap();
        assert_eq!(from_number.expires_in, Some(7200));

        let missing: AccessTokenResponse = serde_json::from_str(r#"{"access_token":"t"}"#).unwrap();
        assert_eq!(missing.expires_in, None);
    }

    #[test]
    fn websocket_accept_matches_rfc_example() {
        // RFC 6455 1.3 示例。
        assert_eq!(
            websocket_accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn frame_parsing_handles_masked_short_frames() {
        // 带掩码的短帧："hello"。
        let mask = [0x37u8, 0xfa, 0x21, 0x3d];
        let payload = b"hello";
        let mut frame = vec![0x81, 0x80 | payload.len() as u8];
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % 4]),
        );

        let parsed = parse_frame_bytes(&frame).unwrap().unwrap();
        assert_eq!(parsed.opcode, 0x1);
        assert_eq!(parsed.payload, b"hello");
        assert_eq!(parsed.consumed, frame.len());

        // 不足一帧时返回 None。
        assert!(parse_frame_bytes(&[0x81]).unwrap().is_none());
        assert!(parse_frame_bytes(&frame[..4]).unwrap().is_none());
    }

    #[test]
    fn frame_parsing_handles_extended_lengths_and_trailing_bytes() {
        // 126 长度编码：200 字节载荷。
        let payload = vec![b'x'; 200];
        let mut frame = vec![0x81, 126];
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        frame.extend_from_slice(&payload);
        let parsed = parse_frame_bytes(&frame).unwrap().unwrap();
        assert_eq!(parsed.opcode, 0x1);
        assert_eq!(parsed.payload, payload);
        assert_eq!(parsed.consumed, frame.len());

        // 同缓冲区内两帧：只消费第一帧。
        let mut two = frame.clone();
        two.extend_from_slice(&frame);
        let first = parse_frame_bytes(&two).unwrap().unwrap();
        assert_eq!(first.payload, payload);
        assert_eq!(first.consumed, frame.len());
    }
}
