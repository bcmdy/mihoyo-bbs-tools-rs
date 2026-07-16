use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::Duration,
};

use reqwest::{
    Url,
    header::{COOKIE, HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};

use crate::{
    auth::CookieJar,
    config::{self, Config, NotificationProvider},
    http::{HttpClient, HttpError, RetryPolicy},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStatus {
    Passed,
    Warning,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticItem {
    pub category: String,
    pub name: String,
    pub status: DiagnosticStatus,
    pub message: String,
    pub impact: String,
    pub suggestion: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    pub schema_version: u8,
    pub online: bool,
    pub items: Vec<DiagnosticItem>,
}

impl DoctorReport {
    pub fn exit_code(&self) -> u8 {
        u8::from(
            self.items
                .iter()
                .any(|item| item.status == DiagnosticStatus::Failed),
        )
    }

    pub fn render_text(&self) -> String {
        let mut output = String::new();
        output.push_str(if self.online {
            "诊断模式：在线只读检查（会发起网络请求，不执行任何任务）\n"
        } else {
            "诊断模式：离线检查（不访问网络）\n"
        });
        for item in &self.items {
            output.push_str(&format!(
                "[{}] {} / {}：{}\n",
                status_name(item.status),
                item.category,
                item.name,
                item.message
            ));
            if item.status != DiagnosticStatus::Passed {
                output.push_str(&format!("  影响：{}\n", item.impact));
                output.push_str(&format!("  建议：{}\n", item.suggestion));
            }
        }
        let passed = self
            .items
            .iter()
            .filter(|item| item.status == DiagnosticStatus::Passed)
            .count();
        let warnings = self
            .items
            .iter()
            .filter(|item| item.status == DiagnosticStatus::Warning)
            .count();
        let failed = self
            .items
            .iter()
            .filter(|item| item.status == DiagnosticStatus::Failed)
            .count();
        output.push_str(&format!(
            "诊断完成：通过 {passed}，警告 {warnings}，失败 {failed}\n"
        ));
        output
    }
}

pub async fn run(path: &Path, online: bool) -> DoctorReport {
    let mut report = DoctorReport {
        schema_version: 1,
        online,
        items: Vec::new(),
    };
    if !path.exists() {
        report.push(
            "配置",
            "配置文件",
            DiagnosticStatus::Failed,
            "配置文件不存在",
            "无法加载账号和任务设置",
            "运行 `config init` 创建配置，或通过 --config 指定正确路径",
        );
        return report;
    }
    report.push(
        "配置",
        "配置文件",
        DiagnosticStatus::Passed,
        "文件存在且可读取",
        "无",
        "无需处理",
    );
    let loaded = match config::load(path) {
        Ok(loaded) => loaded,
        Err(error) => {
            report.push(
                "配置",
                "结构与版本",
                DiagnosticStatus::Failed,
                &error.to_string(),
                "程序无法安全运行",
                "按错误中的字段路径修改配置，或运行 `config setup`",
            );
            return report;
        }
    };
    report.push(
        "配置",
        "结构与版本",
        DiagnosticStatus::Passed,
        &format!(
            "version 1，{} 个账号，结构有效",
            loaded.config.accounts.len()
        ),
        "无",
        "无需处理",
    );
    report.push(
        "配置",
        "环境变量占位符",
        DiagnosticStatus::Passed,
        "所有已引用环境变量均已解析",
        "无",
        "无需处理",
    );
    let config_directory = path.parent().unwrap_or_else(|| Path::new("."));
    report.push_directory_check("配置目录", config_directory);
    if loaded.config.runtime.logging.enabled {
        report.push_directory_check("日志目录", &loaded.config.runtime.logging.directory);
    } else {
        report.push(
            "文件系统",
            "日志目录",
            DiagnosticStatus::Skipped,
            "文件日志已关闭",
            "不生成本地日志文件",
            "如需保留日志，可在 `config setup` 中启用",
        );
    }
    let local_toast = loaded
        .config
        .notifications
        .providers
        .iter()
        .any(|provider| matches!(provider, NotificationProvider::WindowsToast { .. }));
    report.push(
        "平台",
        "Windows 本地通知",
        if local_toast && !cfg!(windows) {
            DiagnosticStatus::Warning
        } else {
            DiagnosticStatus::Passed
        },
        if local_toast && !cfg!(windows) {
            "当前系统不支持已配置的 Windows 本地通知"
        } else {
            "当前平台与本地通知配置兼容"
        },
        "不兼容时该通知渠道会失败，但不影响签到结果",
        "删除不适用的 windows_toast 渠道，或改用网络通知",
    );

    if online {
        run_online(&loaded.config, &mut report).await;
    }
    report
}

impl DoctorReport {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        category: &str,
        name: &str,
        status: DiagnosticStatus,
        message: &str,
        impact: &str,
        suggestion: &str,
    ) {
        self.items.push(DiagnosticItem {
            category: category.to_owned(),
            name: name.to_owned(),
            status,
            message: message.to_owned(),
            impact: impact.to_owned(),
            suggestion: suggestion.to_owned(),
        });
    }

    fn push_directory_check(&mut self, name: &str, directory: &Path) {
        match directory_writable(directory) {
            Ok(()) => self.push(
                "文件系统",
                name,
                DiagnosticStatus::Passed,
                "目录存在且可写",
                "无",
                "无需处理",
            ),
            Err(message) => self.push(
                "文件系统",
                name,
                DiagnosticStatus::Failed,
                &message,
                "配置保存、备份或日志写入可能失败",
                "检查目录是否存在，以及当前用户是否具有写入权限",
            ),
        }
    }
}

async fn run_online(config: &Config, report: &mut DoctorReport) {
    let default_client = match client(config, None) {
        Ok(client) => client,
        Err(_) => {
            report.push(
                "网络",
                "HTTP 客户端",
                DiagnosticStatus::Failed,
                "无法初始化网络客户端",
                "所有在线检查均无法执行",
                "检查 TLS 环境与运行配置",
            );
            return;
        }
    };
    check_connectivity(
        report,
        &default_client,
        "米游社基础网络",
        "https://bbs-api.miyoushe.com/",
    )
    .await;
    check_connectivity(
        report,
        &default_client,
        "HoYoLAB 基础网络",
        "https://bbs-api-os.hoyolab.com/",
    )
    .await;

    for account in config.accounts.iter().filter(|account| account.enabled) {
        let proxy = account
            .proxy
            .url
            .as_ref()
            .map(|value| value.expose_secret());
        let account_client = match client(config, proxy) {
            Ok(client) => client,
            Err(error) => {
                report.push(
                    "账号网络",
                    &account.name,
                    DiagnosticStatus::Failed,
                    safe_http_reason(&error),
                    "该账号无法通过当前代理访问远程接口",
                    "在 `config setup` 中检查该账号代理 URL",
                );
                continue;
            }
        };
        match connectivity(
            &account_client,
            Url::parse("https://bbs-api.miyoushe.com/").expect("valid URL"),
        )
        .await
        {
            Ok(()) => report.push(
                "账号网络",
                &account.name,
                DiagnosticStatus::Passed,
                if proxy.is_some() {
                    "账号代理可连接米游社"
                } else {
                    "直连可访问米游社"
                },
                "无",
                "无需处理",
            ),
            Err(error) => report.push(
                "账号网络",
                &account.name,
                DiagnosticStatus::Failed,
                safe_http_reason(&error),
                "该账号的签到和社区任务可能失败",
                "检查网络、DNS、TLS 与账号代理，然后重试",
            ),
        }
        check_identity(report, &account_client, account).await;
    }

    if let Some(endpoint) = config.captcha.endpoint.clone() {
        check_endpoint(report, &default_client, "验证码端点", origin(endpoint)).await;
    } else {
        report.push(
            "网络",
            "验证码端点",
            DiagnosticStatus::Skipped,
            "未配置验证码平台",
            "遇到风控验证码时无法自动求解",
            "按需在 `config setup` 中配置验证码端点",
        );
    }
    for (index, provider) in config.notifications.providers.iter().enumerate() {
        if let Some(url) = provider.diagnostic_url() {
            check_endpoint(
                report,
                &default_client,
                &format!("通知渠道 {}（{}）", index + 1, provider.kind_name()),
                url,
            )
            .await;
        }
    }
}

async fn check_identity(
    report: &mut DoctorReport,
    client: &HttpClient,
    account: &config::AccountConfig,
) {
    let cookie = account.credentials.cookie.expose_secret();
    let jar = match CookieJar::parse(cookie) {
        Ok(jar) => jar,
        Err(_) => {
            report.push(
                "凭据",
                &account.name,
                DiagnosticStatus::Failed,
                "Cookie 格式无效",
                "无法执行身份查询和账号任务",
                "在 `config setup` 中更新完整 Cookie",
            );
            return;
        }
    };
    let Some(uid) = jar.uid() else {
        report.push(
            "凭据",
            &account.name,
            DiagnosticStatus::Failed,
            "Cookie 中未识别到 UID",
            "无法执行只读身份查询",
            "重新获取包含 account_id 或 ltuid_v2 的完整 Cookie",
        );
        return;
    };
    let mut url = Url::parse("https://bbs-api.miyoushe.com/user/api/getUserFullInfo")
        .expect("valid profile URL");
    url.query_pairs_mut().append_pair("uid", uid);
    let mut headers = HeaderMap::new();
    let Ok(cookie_header) = HeaderValue::from_str(cookie) else {
        report.push(
            "凭据",
            &account.name,
            DiagnosticStatus::Failed,
            "Cookie 包含无效请求头字符",
            "远程接口会拒绝该 Cookie",
            "重新复制 Cookie，避免换行和不可见字符",
        );
        return;
    };
    headers.insert(COOKIE, cookie_header);
    match client
        .get_json_with::<ProfileEnvelope>(url, headers, &[])
        .await
    {
        Ok(response) if response.retcode == 0 && response.data.is_some() => report.push(
            "凭据",
            &account.name,
            DiagnosticStatus::Passed,
            "只读身份查询成功",
            "无",
            "无需处理",
        ),
        Ok(response) => report.push(
            "凭据",
            &account.name,
            DiagnosticStatus::Failed,
            &format!("身份接口拒绝请求（代码 {}）", response.retcode),
            "Cookie 可能已过期或不完整",
            "在 `config setup` 中更新该账号 Cookie",
        ),
        Err(error) => report.push(
            "凭据",
            &account.name,
            DiagnosticStatus::Failed,
            safe_http_reason(&error),
            "尚未确认凭据是否有效",
            "先修复网络或代理，再重新运行 `doctor --online`",
        ),
    }
}

async fn check_connectivity(report: &mut DoctorReport, client: &HttpClient, name: &str, url: &str) {
    check_endpoint(
        report,
        client,
        name,
        Url::parse(url).expect("valid diagnostic URL"),
    )
    .await;
}

async fn check_endpoint(report: &mut DoctorReport, client: &HttpClient, name: &str, url: Url) {
    match connectivity(client, url).await {
        Ok(()) => report.push(
            "网络",
            name,
            DiagnosticStatus::Passed,
            "服务地址可连接",
            "无",
            "无需处理",
        ),
        Err(error) => report.push(
            "网络",
            name,
            DiagnosticStatus::Failed,
            safe_http_reason(&error),
            "依赖该服务的功能可能失败",
            "检查网络、代理、DNS、TLS 和服务地址",
        ),
    }
}

async fn connectivity(client: &HttpClient, url: Url) -> Result<(), HttpError> {
    match client.get_once_without_response(url, &[]).await {
        Ok(()) | Err(HttpError::Status(_)) => Ok(()),
        Err(error) => Err(error),
    }
}

fn client(config: &Config, proxy: Option<&str>) -> Result<HttpClient, HttpError> {
    HttpClient::builder()
        .timeout(Duration::from_secs(config.runtime.request_timeout_seconds))
        .retry(RetryPolicy {
            attempts: 1,
            base_delay: Duration::ZERO,
        })
        .proxy(proxy)?
        .build()
}

fn origin(mut url: Url) -> Url {
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    url
}

fn directory_writable(directory: &Path) -> Result<(), String> {
    if !directory.is_dir() {
        return Err("目录不存在或不是文件夹".to_owned());
    }
    let probe = directory.join(format!(
        ".mihoyo-bbs-tools-doctor-{}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|_| "当前用户无法在该目录创建文件".to_owned())?;
    let result = file
        .write_all(b"diagnostic probe")
        .map_err(|_| "当前用户无法写入该目录".to_owned());
    drop(file);
    let _ = fs::remove_file(probe);
    result
}

fn safe_http_reason(error: &HttpError) -> &'static str {
    match error {
        HttpError::InvalidUrl(_) => "服务地址无效",
        HttpError::InvalidProxy(_) => "代理地址无效",
        HttpError::Timeout => "请求超时",
        HttpError::Connect(_) => "连接失败（可能是 DNS、TLS、网络或代理问题）",
        HttpError::Status(_) => "服务返回了 HTTP 错误状态",
        HttpError::Decode(_) => "服务响应格式无效",
        HttpError::Build(_) => "HTTP 客户端初始化失败",
    }
}

const fn status_name(status: DiagnosticStatus) -> &'static str {
    match status {
        DiagnosticStatus::Passed => "通过",
        DiagnosticStatus::Warning => "警告",
        DiagnosticStatus::Failed => "失败",
        DiagnosticStatus::Skipped => "跳过",
    }
}

#[derive(Deserialize)]
struct ProfileEnvelope {
    retcode: i64,
    data: Option<serde_json::Value>,
}
