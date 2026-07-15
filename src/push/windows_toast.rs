#[cfg(windows)]
use std::{
    os::windows::process::CommandExt,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use super::{Notification, Provider, ProviderKind, PushError, SendFuture};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const TOAST_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(windows)]
const TOAST_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
if ([System.Diagnostics.Process]::GetCurrentProcess().SessionId -eq 0) { throw 'interactive desktop session is required' }
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] > $null
$title = [System.Security.SecurityElement]::Escape($env:MIHOYO_BBS_TOAST_TITLE)
$body = [System.Security.SecurityElement]::Escape($env:MIHOYO_BBS_TOAST_BODY)
$payload = '<toast><visual><binding template="ToastGeneric"><text>{0}</text><text>{1}</text></binding></visual></toast>' -f $title, $body
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml($payload)
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('MihoyoBBSToolsRS').Show($toast)
"#;

#[derive(Clone, Debug)]
pub(super) struct WindowsToastProvider {
    title_prefix: String,
}

impl WindowsToastProvider {
    pub(super) fn new(title_prefix: String) -> Self {
        Self { title_prefix }
    }

    #[cfg(any(windows, test))]
    fn title(&self, notification: &Notification) -> String {
        let prefix = self.title_prefix.trim();
        let title = if prefix.is_empty() {
            notification.title.clone()
        } else {
            format!("{prefix} · {}", notification.title)
        };
        truncate(&title, 128)
    }

    #[cfg(windows)]
    fn send_blocking(&self, notification: &Notification) -> Result<(), PushError> {
        let mut child = Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                TOAST_SCRIPT,
            ])
            .env("MIHOYO_BBS_TOAST_TITLE", self.title(notification))
            .env("MIHOYO_BBS_TOAST_BODY", truncate(&notification.body, 4096))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|_| PushError::WindowsToastDelivery)?;
        let deadline = Instant::now() + TOAST_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(_)) | Err(_) => return Err(PushError::WindowsToastDelivery),
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(PushError::WindowsToastDelivery);
                }
            }
        }
    }
}

impl Provider for WindowsToastProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::WindowsToast
    }

    fn send<'a>(&'a self, notification: &'a Notification) -> SendFuture<'a> {
        let provider = self.clone();
        let notification = notification.clone();
        Box::pin(async move {
            #[cfg(windows)]
            {
                tokio::task::spawn_blocking(move || provider.send_blocking(&notification))
                    .await
                    .map_err(|_| PushError::WindowsToastDelivery)?
            }
            #[cfg(not(windows))]
            {
                let _ = (provider.title_prefix, notification);
                Err(PushError::UnsupportedWindowsToast)
            }
        })
    }
}

#[cfg(any(windows, test))]
fn truncate(value: &str, maximum: usize) -> String {
    let mut characters = value.chars();
    let mut output = characters.by_ref().take(maximum).collect::<String>();
    if characters.next().is_some() {
        output.push('…');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification() -> Notification {
        Notification {
            title: "执行成功".to_owned(),
            body: "签到完成".to_owned(),
        }
    }

    #[test]
    fn title_prefix_is_optional_and_long_text_is_truncated_safely() {
        let provider = WindowsToastProvider::new("MihoyoBBSTools RS".to_owned());
        assert_eq!(
            provider.title(&notification()),
            "MihoyoBBSTools RS · 执行成功"
        );
        let provider = WindowsToastProvider::new(String::new());
        assert_eq!(provider.title(&notification()), "执行成功");
        assert_eq!(truncate("测试内容", 2), "测试…");
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn unsupported_platform_returns_explicit_error() {
        let provider = WindowsToastProvider::new(String::new());
        let notification = notification();
        assert_eq!(
            provider.send(&notification).await,
            Err(PushError::UnsupportedWindowsToast)
        );
    }
}
