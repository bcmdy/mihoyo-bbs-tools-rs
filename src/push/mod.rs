use std::{future::Future, pin::Pin, time::Duration};

use reqwest::{Url, header::HeaderMap};
use serde::{Deserialize, Serialize};

use crate::{
    auth::SecretString,
    config::{Config, NotificationProvider},
    http::{HttpClient, HttpError, RetryPolicy},
    service::RunReport,
};

mod smtp;
use smtp::SmtpProvider;
mod windows_toast;
use windows_toast::WindowsToastProvider;

const PUSHPLUS_API_URL: &str = "https://www.pushplus.plus/send";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notification {
    pub title: String,
    pub body: String,
}

impl Notification {
    pub fn from_report(report: &RunReport) -> Self {
        let title = match report.exit_code() {
            0 => "「米游社工具」执行成功",
            3 => "「米游社工具」认证失败",
            4 => "「米游社工具」需要验证码",
            5 => "「米游社工具」网络请求失败",
            _ => "「米游社工具」执行失败",
        };
        let body = report.render_text();
        Self {
            title: title.to_owned(),
            body: if body.is_empty() {
                "本次运行没有任务记录。".to_owned()
            } else {
                body
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Telegram,
    Webhook,
    Pushplus,
    Ftqq,
    Pushme,
    Cqhttp,
    Wecom,
    Wecomrobot,
    Pushdeer,
    Dingrobot,
    Feishubot,
    Bark,
    Gotify,
    Ifttt,
    Qmsg,
    Discord,
    Wxpusher,
    Serverchan3,
    Smtp,
    WindowsToast,
}

impl ProviderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Webhook => "webhook",
            Self::Pushplus => "pushplus",
            Self::Ftqq => "ftqq",
            Self::Pushme => "pushme",
            Self::Cqhttp => "cqhttp",
            Self::Wecom => "wecom",
            Self::Wecomrobot => "wecomrobot",
            Self::Pushdeer => "pushdeer",
            Self::Dingrobot => "dingrobot",
            Self::Feishubot => "feishubot",
            Self::Bark => "bark",
            Self::Gotify => "gotify",
            Self::Ifttt => "ifttt",
            Self::Qmsg => "qmsg",
            Self::Discord => "discord",
            Self::Wxpusher => "wxpusher",
            Self::Serverchan3 => "serverchan3",
            Self::Smtp => "smtp",
            Self::WindowsToast => "windows_toast",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Telegram => "Telegram 机器人",
            Self::Webhook => "通用 Webhook",
            Self::Pushplus => "PushPlus",
            Self::Ftqq => "Server 酱 Turbo",
            Self::Pushme => "PushMe",
            Self::Cqhttp => "CQHTTP QQ 机器人",
            Self::Wecom => "企业微信应用",
            Self::Wecomrobot => "企业微信群机器人",
            Self::Pushdeer => "PushDeer",
            Self::Dingrobot => "钉钉群机器人",
            Self::Feishubot => "飞书群机器人",
            Self::Bark => "Bark",
            Self::Gotify => "Gotify",
            Self::Ifttt => "IFTTT Webhooks",
            Self::Qmsg => "Qmsg 酱",
            Self::Discord => "Discord Webhook",
            Self::Wxpusher => "WxPusher",
            Self::Serverchan3 => "Server 酱 3",
            Self::Smtp => "SMTP 邮件",
            Self::WindowsToast => "Windows 本地通知",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSummary {
    pub index: usize,
    pub kind: ProviderKind,
    pub target: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Sent,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeliveryResult {
    pub provider: ProviderKind,
    pub status: DeliveryStatus,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PushReport {
    pub deliveries: Vec<DeliveryResult>,
}

impl PushReport {
    pub fn all_succeeded(&self) -> bool {
        self.deliveries
            .iter()
            .all(|delivery| delivery.status == DeliveryStatus::Sent)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PushError {
    #[error("推送接口地址无效")]
    InvalidEndpoint,
    #[error("推送请求超时")]
    Timeout,
    #[error("推送网络请求失败")]
    Network,
    #[error("推送服务返回 HTTP {0}")]
    HttpStatus(u16),
    #[error("推送服务响应无效")]
    InvalidResponse,
    #[error("推送服务拒绝请求（代码 {0}）")]
    ServiceRejected(i64),
    #[error("邮件地址或正文无效")]
    InvalidMessage,
    #[error("SMTP 邮件发送失败")]
    SmtpDelivery,
    #[error("Windows 本地通知仅支持 Windows 桌面会话")]
    UnsupportedWindowsToast,
    #[error("Windows 本地通知提交失败")]
    WindowsToastDelivery,
}

pub type SendFuture<'a> = Pin<Box<dyn Future<Output = Result<(), PushError>> + Send + 'a>>;

pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;

    fn send<'a>(&'a self, notification: &'a Notification) -> SendFuture<'a>;
}

pub struct PushDispatcher {
    providers: Vec<Box<dyn Provider>>,
}

impl std::fmt::Debug for PushDispatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PushDispatcher")
            .field("provider_count", &self.providers.len())
            .finish()
    }
}

impl PushDispatcher {
    pub fn new(providers: Vec<Box<dyn Provider>>) -> Self {
        Self { providers }
    }

    pub fn from_config(config: &Config) -> Self {
        let providers = config
            .notifications
            .providers
            .iter()
            .map(|provider| configured_provider(config, provider))
            .collect();
        Self::new(providers)
    }

    pub async fn dispatch(&self, notification: &Notification) -> PushReport {
        let mut report = PushReport::default();
        for provider in &self.providers {
            let kind = provider.kind();
            let result = provider.send(notification).await;
            report.deliveries.push(match result {
                Ok(()) => DeliveryResult {
                    provider: kind,
                    status: DeliveryStatus::Sent,
                    message: "推送成功".to_owned(),
                },
                Err(error) => DeliveryResult {
                    provider: kind,
                    status: DeliveryStatus::Failed,
                    message: error.to_string(),
                },
            });
        }
        report
    }
}

/// 将任务报告发送到全部已配置渠道。未启用通知时返回空报告。
///
/// 单个渠道失败不会中断后续渠道；每个渠道的结果均记录在返回值中。
pub async fn send_report(config: &Config, report: &RunReport) -> PushReport {
    if !config.notifications.enabled || (config.notifications.error_only && report.exit_code() == 0)
    {
        return PushReport::default();
    }

    let mut notification = Notification::from_report(report);
    for keyword in &config.notifications.block_keywords {
        if !keyword.is_empty() {
            notification.body = notification
                .body
                .replace(keyword, &"*".repeat(keyword.chars().count()));
        }
    }
    PushDispatcher::from_config(config)
        .dispatch(&notification)
        .await
}

pub fn provider_summaries(config: &Config) -> Vec<ProviderSummary> {
    config
        .notifications
        .providers
        .iter()
        .enumerate()
        .map(|(index, provider)| ProviderSummary {
            index: index + 1,
            kind: configured_kind(provider),
            target: provider_target(provider),
        })
        .collect()
}

pub async fn test_providers(
    config: &Config,
    selected: Option<usize>,
) -> Result<PushReport, String> {
    if config.notifications.providers.is_empty() {
        return Err("尚未配置通知渠道".to_owned());
    }
    if selected.is_some_and(|index| index == 0 || index > config.notifications.providers.len()) {
        return Err(format!(
            "通知渠道编号超出范围，应为 1 到 {}",
            config.notifications.providers.len()
        ));
    }
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let timestamp = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let mut report = PushReport::default();
    for (index, configured) in config.notifications.providers.iter().enumerate() {
        let number = index + 1;
        if selected.is_some_and(|selected| selected != number) {
            continue;
        }
        let provider = configured_provider(config, configured);
        let kind = provider.kind();
        let notification = Notification {
            title: "「米游社工具」测试通知".to_owned(),
            body: format!(
                "这是一条测试通知。\n程序版本：{}\n通知渠道：{}（{}）\n发送时间：{}",
                crate::VERSION,
                kind.display_name(),
                kind.as_str(),
                timestamp
            ),
        };
        report.deliveries.push(match provider.send(&notification).await {
            Ok(()) => DeliveryResult {
                provider: kind,
                status: DeliveryStatus::Sent,
                message: "测试通知发送成功".to_owned(),
            },
            Err(error) => DeliveryResult {
                provider: kind,
                status: DeliveryStatus::Failed,
                message: error.to_string(),
            },
        });
    }
    Ok(report)
}

fn provider_target(provider: &NotificationProvider) -> String {
    match provider {
        NotificationProvider::Telegram { chat_id, .. } => mask_tail(chat_id),
        NotificationProvider::Smtp { to, .. } => mask_email(to),
        NotificationProvider::WindowsToast { .. } => "当前 Windows 桌面".to_owned(),
        _ => "已配置（敏感地址不显示）".to_owned(),
    }
}

fn mask_tail(value: &str) -> String {
    let tail = value.chars().rev().take(4).collect::<Vec<_>>();
    format!("***{}", tail.into_iter().rev().collect::<String>())
}

fn mask_email(value: &str) -> String {
    let Some((name, domain)) = value.split_once('@') else {
        return "***".to_owned();
    };
    format!("{}***@{domain}", name.chars().next().unwrap_or('*'))
}

fn configured_http(
    config: &Config,
    provider: &NotificationProvider,
) -> Result<HttpClient, HttpError> {
    let builder = HttpClient::builder()
        .timeout(Duration::from_secs(config.runtime.request_timeout_seconds))
        .retry(RetryPolicy {
            attempts: config.runtime.retry_count as usize + 1,
            base_delay: Duration::from_millis(500),
        });
    let proxy = match provider {
        NotificationProvider::Telegram { proxy, .. } => {
            proxy.as_ref().map(|value| value.expose_secret())
        }
        _ => None,
    };
    builder.proxy(proxy)?.build()
}

fn configured_provider(config: &Config, provider: &NotificationProvider) -> Box<dyn Provider> {
    if let NotificationProvider::Smtp {
        host,
        port,
        from,
        to,
        username,
        password,
        subject,
        tls,
        timeout_seconds,
    } = provider
    {
        return Box::new(SmtpProvider::new(
            host.clone(),
            *port,
            from.clone(),
            to.clone(),
            username.clone(),
            password.clone(),
            subject.clone(),
            *tls,
            Duration::from_secs(timeout_seconds.unwrap_or(config.runtime.request_timeout_seconds)),
        ));
    }
    if let NotificationProvider::WindowsToast { title_prefix } = provider {
        return Box::new(WindowsToastProvider::new(title_prefix.clone()));
    }
    let http = configured_http(config, provider);
    let http = match http {
        Ok(http) => http,
        Err(error) => {
            return Box::new(FailedProvider {
                kind: configured_kind(provider),
                error: redact_http_error(error),
            });
        }
    };
    match provider {
        NotificationProvider::Telegram {
            bot_token,
            chat_id,
            api_url,
            ..
        } => Box::new(TelegramProvider::new(
            http,
            bot_token.clone(),
            chat_id.clone(),
            api_url.clone(),
        )),
        NotificationProvider::Webhook { url } => Box::new(WebhookProvider::new(http, url.clone())),
        NotificationProvider::Pushplus { token, topic } => {
            Box::new(PushplusProvider::new(http, token.clone(), topic.clone()))
        }
        other => Box::new(CompatProvider {
            http,
            config: other.clone(),
        }),
    }
}

struct FailedProvider {
    kind: ProviderKind,
    error: PushError,
}

impl Provider for FailedProvider {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn send<'a>(&'a self, _notification: &'a Notification) -> SendFuture<'a> {
        Box::pin(async move { Err(self.error.clone()) })
    }
}

fn configured_kind(provider: &NotificationProvider) -> ProviderKind {
    match provider {
        NotificationProvider::Telegram { .. } => ProviderKind::Telegram,
        NotificationProvider::Webhook { .. } => ProviderKind::Webhook,
        NotificationProvider::Pushplus { .. } => ProviderKind::Pushplus,
        NotificationProvider::Ftqq { .. } => ProviderKind::Ftqq,
        NotificationProvider::Pushme { .. } => ProviderKind::Pushme,
        NotificationProvider::Cqhttp { .. } => ProviderKind::Cqhttp,
        NotificationProvider::Wecom { .. } => ProviderKind::Wecom,
        NotificationProvider::Wecomrobot { .. } => ProviderKind::Wecomrobot,
        NotificationProvider::Pushdeer { .. } => ProviderKind::Pushdeer,
        NotificationProvider::Dingrobot { .. } => ProviderKind::Dingrobot,
        NotificationProvider::Feishubot { .. } => ProviderKind::Feishubot,
        NotificationProvider::Bark { .. } => ProviderKind::Bark,
        NotificationProvider::Gotify { .. } => ProviderKind::Gotify,
        NotificationProvider::Ifttt { .. } => ProviderKind::Ifttt,
        NotificationProvider::Qmsg { .. } => ProviderKind::Qmsg,
        NotificationProvider::Discord { .. } => ProviderKind::Discord,
        NotificationProvider::Wxpusher { .. } => ProviderKind::Wxpusher,
        NotificationProvider::Serverchan3 { .. } => ProviderKind::Serverchan3,
        NotificationProvider::Smtp { .. } => ProviderKind::Smtp,
        NotificationProvider::WindowsToast { .. } => ProviderKind::WindowsToast,
    }
}

#[derive(Clone)]
struct CompatProvider {
    http: HttpClient,
    config: NotificationProvider,
}

impl std::fmt::Debug for CompatProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompatProvider").finish_non_exhaustive()
    }
}

impl Provider for CompatProvider {
    fn kind(&self) -> ProviderKind {
        configured_kind(&self.config)
    }
    fn send<'a>(&'a self, n: &'a Notification) -> SendFuture<'a> {
        Box::pin(async move { self.send_inner(n).await })
    }
}

impl CompatProvider {
    async fn send_inner(&self, n: &Notification) -> Result<(), PushError> {
        use NotificationProvider::*;
        let text = format!("{}\n{}", n.title, n.body);
        match &self.config {
            Ftqq { sendkey, api_url } => {
                let base = api_url.clone().unwrap_or_else(|| Url::parse("https://sctapi.ftqq.com/").unwrap());
                let url = base.join(&format!("{}.send", sendkey.expose_secret())).map_err(|_| PushError::InvalidEndpoint)?;
                self.form(url, &[("title", n.title.clone()), ("desp", n.body.clone())]).await
            }
            Pushme { token, api_url } => {
                let url = api_url.clone().unwrap_or_else(|| Url::parse("https://push.i-i.me/").unwrap());
                self.form(url, &[("push_key", token.expose_secret().to_owned()), ("title", n.title.clone()), ("content", n.body.clone()), ("type", "text".into())]).await
            }
            Cqhttp { url, qq, group } => {
                let mut body = serde_json::json!({"message": text});
                if let Some(v)=qq { body["user_id"] = serde_json::Value::String(v.expose_secret().into()); }
                if let Some(v)=group { body["group_id"] = serde_json::Value::String(v.expose_secret().into()); }
                self.json(secret_url(url)?, &body).await
            }
            Wecomrobot { url, mobile } => self.json(secret_url(url)?, &serde_json::json!({"msgtype":"text","text":{"content":text,"mentioned_mobile_list":mobile.as_ref().map(|v| vec![v.expose_secret()]).unwrap_or_default()}})).await,
            Pushdeer { token, api_url } => {
                let base=api_url.clone().unwrap_or_else(||Url::parse("https://api2.pushdeer.com/").unwrap());
                let url=base.join("message/push").map_err(|_|PushError::InvalidEndpoint)?;
                self.http.get_once_without_response(url,&[("pushkey",token.expose_secret().into()),("text",n.title.clone()),("desp",n.body.clone()),("type","markdown".into())]).await.map_err(redact_http_error)
            }
            Dingrobot { webhook, secret } => {
                let mut url=secret_url(webhook)?;
                if let Some(secret)=secret { sign_ding(&mut url, secret.expose_secret())?; }
                self.json(url,&serde_json::json!({"msgtype":"text","text":{"content":text}})).await
            }
            Feishubot { webhook } => self.json(secret_url(webhook)?,&serde_json::json!({"msg_type":"text","content":{"text":text}})).await,
            Bark { token, api_url, icon } => {
                let base=api_url.clone().unwrap_or_else(||Url::parse("https://api.day.app/").unwrap());
                let mut url=base.join(&format!("{}/{}/{}",token.expose_secret(),encode_path(&n.title),encode_path(&n.body))).map_err(|_|PushError::InvalidEndpoint)?;
                if let Some(icon)=icon { url.query_pairs_mut().append_pair("icon",icon); }
                self.http.get_once_without_response(url,&[]).await.map_err(redact_http_error)
            }
            Gotify { token, api_url, priority } => {
                let mut url=api_url.join("message").map_err(|_|PushError::InvalidEndpoint)?; url.query_pairs_mut().append_pair("token",token.expose_secret());
                self.json(url,&serde_json::json!({"title":n.title,"message":n.body,"priority":priority})).await
            }
            Ifttt { event, key, api_url } => {
                let base=api_url.clone().unwrap_or_else(||Url::parse("https://maker.ifttt.com/").unwrap());
                let url=base.join(&format!("trigger/{event}/with/key/{}",key.expose_secret())).map_err(|_|PushError::InvalidEndpoint)?;
                self.json(url,&serde_json::json!({"value1":n.title,"value2":n.body})).await
            }
            Qmsg { key, api_url } => {
                let base=api_url.clone().unwrap_or_else(||Url::parse("https://qmsg.zendee.cn/").unwrap());
                let url=base.join(&format!("send/{}",key.expose_secret())).map_err(|_|PushError::InvalidEndpoint)?;
                self.form(url,&[("msg",text)]).await
            }
            Discord { webhook } => self.json(secret_url(webhook)?,&serde_json::json!({"username":"MihoyoBBSTools","embeds":[{"title":n.title,"description":n.body,"color":5763719}]})).await,
            Wxpusher { app_token, uids, topic_ids, api_url } => {
                let url=api_url.clone().unwrap_or_else(||Url::parse("https://wxpusher.zjiecode.com/api/send/message").unwrap());
                self.json(url,&serde_json::json!({"appToken":app_token.expose_secret(),"content":text,"contentType":1,"uids":uids,"topicIds":topic_ids})).await
            }
            Serverchan3 { sendkey, tags } => {
                let raw=sendkey.expose_secret(); let number=raw.strip_prefix("sctp").and_then(|v|v.split('t').next()).filter(|v|v.bytes().all(|b|b.is_ascii_digit())).ok_or(PushError::InvalidEndpoint)?;
                let url=Url::parse(&format!("https://{number}.push.ft07.com/send/{raw}.send")).map_err(|_|PushError::InvalidEndpoint)?;
                self.json(url,&serde_json::json!({"title":n.title,"desp":n.body,"tags":tags})).await
            }
            Wecom { corp_id, agent_id, secret, to_user, api_url } => {
                #[derive(Deserialize)] struct Token { access_token:String, #[serde(default)] errcode:i64 }
                let base=api_url.clone().unwrap_or_else(||Url::parse("https://qyapi.weixin.qq.com/").unwrap());
                let token_url=base.join("cgi-bin/gettoken").map_err(|_|PushError::InvalidEndpoint)?;
                let token:Token=self.http.get_json_with(token_url,HeaderMap::new(),&[("corpid",corp_id.expose_secret().into()),("corpsecret",secret.expose_secret().into())]).await.map_err(redact_http_error)?;
                if token.errcode!=0 { return Err(PushError::ServiceRejected(token.errcode)); }
                let mut send=base.join("cgi-bin/message/send").map_err(|_|PushError::InvalidEndpoint)?; send.query_pairs_mut().append_pair("access_token",&token.access_token);
                self.json(send,&serde_json::json!({"touser":to_user,"msgtype":"text","agentid":agent_id,"text":{"content":text},"safe":0})).await
            }
            Telegram{..}|Webhook{..}|Pushplus{..}|Smtp{..}|WindowsToast{..} => unreachable!(),
        }
    }
    async fn json(&self, url: Url, body: &serde_json::Value) -> Result<(), PushError> {
        self.http
            .post_json_once_without_response(url, HeaderMap::new(), body)
            .await
            .map_err(redact_http_error)
    }
    async fn form(&self, url: Url, body: &[(&str, String)]) -> Result<(), PushError> {
        self.http
            .post_form_once_without_response(url, body)
            .await
            .map_err(redact_http_error)
    }
}

fn secret_url(value: &SecretString) -> Result<Url, PushError> {
    Url::parse(value.expose_secret()).map_err(|_| PushError::InvalidEndpoint)
}
fn encode_path(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
fn sign_ding(url: &mut Url, secret: &str) -> Result<(), PushError> {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| PushError::InvalidEndpoint)?
        .as_millis()
        .to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| PushError::InvalidEndpoint)?;
    mac.update(format!("{timestamp}\n{secret}").as_bytes());
    let sign = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    url.query_pairs_mut()
        .append_pair("timestamp", &timestamp)
        .append_pair("sign", &sign);
    Ok(())
}

#[derive(Clone)]
pub struct TelegramProvider {
    http: HttpClient,
    bot_token: SecretString,
    chat_id: String,
    api_url: Url,
}

impl std::fmt::Debug for TelegramProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TelegramProvider")
            .field("bot_token", &"[REDACTED]")
            .field("chat_id", &"[REDACTED]")
            .field("api_url", &self.api_url)
            .finish_non_exhaustive()
    }
}

impl TelegramProvider {
    pub fn new(http: HttpClient, bot_token: SecretString, chat_id: String, api_url: Url) -> Self {
        Self {
            http,
            bot_token,
            chat_id,
            api_url,
        }
    }

    fn endpoint(&self) -> Result<Url, PushError> {
        let raw = format!(
            "{}/bot{}/sendMessage",
            self.api_url.as_str().trim_end_matches('/'),
            self.bot_token.expose_secret()
        );
        Url::parse(&raw).map_err(|_| PushError::InvalidEndpoint)
    }
}

#[derive(Serialize)]
struct TelegramRequest<'a> {
    chat_id: &'a str,
    text: String,
}

#[derive(Deserialize)]
struct TelegramResponse {
    ok: bool,
    #[serde(default)]
    error_code: Option<i64>,
}

impl Provider for TelegramProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Telegram
    }

    fn send<'a>(&'a self, notification: &'a Notification) -> SendFuture<'a> {
        Box::pin(async move {
            let response: TelegramResponse = self
                .http
                .post_json_once(
                    self.endpoint()?,
                    HeaderMap::new(),
                    &TelegramRequest {
                        chat_id: &self.chat_id,
                        text: format!("{}\n{}", notification.title, notification.body),
                    },
                )
                .await
                .map_err(redact_http_error)?;
            if response.ok {
                Ok(())
            } else {
                Err(PushError::ServiceRejected(
                    response.error_code.unwrap_or(-1),
                ))
            }
        })
    }
}

#[derive(Clone)]
pub struct WebhookProvider {
    http: HttpClient,
    url: SecretString,
}

impl std::fmt::Debug for WebhookProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebhookProvider")
            .field("url", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl WebhookProvider {
    pub fn new(http: HttpClient, url: SecretString) -> Self {
        Self { http, url }
    }
}

#[derive(Serialize)]
struct WebhookRequest<'a> {
    title: &'a str,
    message: &'a str,
}

impl Provider for WebhookProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Webhook
    }

    fn send<'a>(&'a self, notification: &'a Notification) -> SendFuture<'a> {
        Box::pin(async move {
            let url =
                Url::parse(self.url.expose_secret()).map_err(|_| PushError::InvalidEndpoint)?;
            self.http
                .post_json_once_without_response(
                    url,
                    HeaderMap::new(),
                    &WebhookRequest {
                        title: &notification.title,
                        message: &notification.body,
                    },
                )
                .await
                .map_err(redact_http_error)
        })
    }
}

#[derive(Clone)]
pub struct PushplusProvider {
    http: HttpClient,
    token: SecretString,
    topic: Option<String>,
    api_url: Url,
}

impl std::fmt::Debug for PushplusProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PushplusProvider")
            .field("token", &"[REDACTED]")
            .field("topic", &self.topic)
            .field("api_url", &self.api_url)
            .finish_non_exhaustive()
    }
}

impl PushplusProvider {
    pub fn new(http: HttpClient, token: SecretString, topic: Option<String>) -> Self {
        Self::with_endpoint(
            http,
            token,
            topic,
            Url::parse(PUSHPLUS_API_URL).expect("valid PushPlus URL"),
        )
    }

    pub fn with_endpoint(
        http: HttpClient,
        token: SecretString,
        topic: Option<String>,
        api_url: Url,
    ) -> Self {
        Self {
            http,
            token,
            topic,
            api_url,
        }
    }
}

#[derive(Serialize)]
struct PushplusRequest<'a> {
    token: &'a str,
    title: &'a str,
    content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    topic: Option<&'a str>,
}

#[derive(Deserialize)]
struct PushplusResponse {
    code: i64,
}

impl Provider for PushplusProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Pushplus
    }

    fn send<'a>(&'a self, notification: &'a Notification) -> SendFuture<'a> {
        Box::pin(async move {
            let response: PushplusResponse = self
                .http
                .post_json_once(
                    self.api_url.clone(),
                    HeaderMap::new(),
                    &PushplusRequest {
                        token: self.token.expose_secret(),
                        title: &notification.title,
                        content: &notification.body,
                        topic: self.topic.as_deref(),
                    },
                )
                .await
                .map_err(redact_http_error)?;
            if response.code == 200 {
                Ok(())
            } else {
                Err(PushError::ServiceRejected(response.code))
            }
        })
    }
}

fn redact_http_error(error: HttpError) -> PushError {
    match error {
        HttpError::InvalidUrl(_) => PushError::InvalidEndpoint,
        HttpError::Timeout => PushError::Timeout,
        HttpError::Status(status) => PushError::HttpStatus(status.as_u16()),
        HttpError::Decode(_) => PushError::InvalidResponse,
        HttpError::InvalidProxy(_) | HttpError::Connect(_) | HttpError::Build(_) => {
            PushError::Network
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path},
    };

    fn http() -> HttpClient {
        HttpClient::builder()
            .retry(RetryPolicy {
                attempts: 1,
                base_delay: Duration::ZERO,
            })
            .build()
            .unwrap()
    }

    fn notification() -> Notification {
        Notification {
            title: "执行结果".to_owned(),
            body: "签到成功".to_owned(),
        }
    }

    #[tokio::test]
    async fn telegram_uses_configured_url_and_expected_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/bot123:secret/sendMessage"))
            .and(header("content-type", "application/json"))
            .and(body_json(json!({
                "chat_id": "987654",
                "text": "执行结果\n签到成功"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = TelegramProvider::new(
            http(),
            SecretString::new("123:secret"),
            "987654".to_owned(),
            Url::parse(&server.uri()).unwrap(),
        );
        assert_eq!(provider.send(&notification()).await, Ok(()));
    }

    #[tokio::test]
    async fn webhook_accepts_empty_success_response_and_expected_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/private-hook"))
            .and(header("content-type", "application/json"))
            .and(body_json(json!({
                "title": "执行结果",
                "message": "签到成功"
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let provider = WebhookProvider::new(
            http(),
            SecretString::new(format!("{}/private-hook", server.uri())),
        );
        assert_eq!(provider.send(&notification()).await, Ok(()));
    }

    #[tokio::test]
    async fn pushplus_uses_expected_url_headers_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/send"))
            .and(header("content-type", "application/json"))
            .and(body_json(json!({
                "token": "pushplus-secret",
                "title": "执行结果",
                "content": "签到成功",
                "topic": "daily"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 200 })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = PushplusProvider::with_endpoint(
            http(),
            SecretString::new("pushplus-secret"),
            Some("daily".to_owned()),
            Url::parse(&format!("{}/send", server.uri())).unwrap(),
        );
        assert_eq!(provider.send(&notification()).await, Ok(()));
    }

    #[tokio::test]
    async fn dispatcher_continues_after_provider_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/failed"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/succeeded"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let dispatcher = PushDispatcher::new(vec![
            Box::new(WebhookProvider::new(
                http(),
                SecretString::new(format!("{}/failed", server.uri())),
            )),
            Box::new(WebhookProvider::new(
                http(),
                SecretString::new(format!("{}/succeeded", server.uri())),
            )),
        ]);
        let report = dispatcher.dispatch(&notification()).await;

        assert_eq!(report.deliveries.len(), 2);
        assert_eq!(report.deliveries[0].status, DeliveryStatus::Failed);
        assert_eq!(report.deliveries[1].status, DeliveryStatus::Sent);
        assert!(!report.all_succeeded());
    }

    #[tokio::test]
    async fn telegram_proxy_initialization_failure_does_not_block_other_providers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/succeeded"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let proxy_secret = "proxy-user:proxy-password";
        let source = format!(
            "version: 1\naccounts: []\nnotifications:\n  enabled: true\n  providers:\n    - type: telegram\n      bot_token: telegram-secret\n      chat_id: '123456'\n      proxy: file://{proxy_secret}@localhost/private\n    - type: webhook\n      url: '{}/succeeded'\n",
            server.uri()
        );
        let config: Config = serde_yaml_ng::from_str(&source).unwrap();

        let report = send_report(&config, &RunReport::default()).await;

        assert_eq!(report.deliveries.len(), 2);
        assert_eq!(report.deliveries[0].provider, ProviderKind::Telegram);
        assert_eq!(report.deliveries[0].status, DeliveryStatus::Failed);
        assert!(!report.deliveries[0].message.contains(proxy_secret));
        assert_eq!(report.deliveries[1].provider, ProviderKind::Webhook);
        assert_eq!(report.deliveries[1].status, DeliveryStatus::Sent);
    }

    #[tokio::test]
    async fn errors_and_debug_output_do_not_expose_secrets() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let token = "telegram-super-secret";
        let provider = TelegramProvider::new(
            http(),
            SecretString::new(token),
            "sensitive-chat-id".to_owned(),
            Url::parse(&server.uri()).unwrap(),
        );

        let error = provider.send(&notification()).await.unwrap_err();
        assert!(!format!("{error}").contains(token));
        let debug = format!("{provider:?}");
        assert!(!debug.contains(token));
        assert!(!debug.contains("sensitive-chat-id"));
    }

    #[test]
    fn structured_delivery_uses_specific_provider_name() {
        let report = PushReport {
            deliveries: vec![DeliveryResult {
                provider: ProviderKind::Ftqq,
                status: DeliveryStatus::Sent,
                message: "推送成功".to_owned(),
            }],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains(r#""provider":"ftqq""#));
        assert!(json.contains(r#""status":"sent""#));
    }
}
