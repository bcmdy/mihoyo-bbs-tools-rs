use std::time::Duration;

use lettre::{Message, SmtpTransport, Transport, transport::smtp::authentication::Credentials};

use super::{Notification, Provider, ProviderKind, PushError, SendFuture};
use crate::{auth::SecretString, config::SmtpTlsMode};

#[derive(Clone)]
pub(super) struct SmtpProvider {
    host: String,
    port: u16,
    from: String,
    to: String,
    username: SecretString,
    password: SecretString,
    subject: String,
    tls: SmtpTlsMode,
    timeout: Duration,
}

impl std::fmt::Debug for SmtpProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SmtpProvider")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("from", &self.from)
            .field("to", &self.to)
            .field("username", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .field("subject", &self.subject)
            .field("tls", &self.tls)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl SmtpProvider {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        host: String,
        port: u16,
        from: String,
        to: String,
        username: SecretString,
        password: SecretString,
        subject: String,
        tls: SmtpTlsMode,
        timeout: Duration,
    ) -> Self {
        Self {
            host,
            port,
            from,
            to,
            username,
            password,
            subject,
            tls,
            timeout,
        }
    }

    fn send_blocking(self, notification: Notification) -> Result<(), PushError> {
        let message = Message::builder()
            .from(self.from.parse().map_err(|_| PushError::InvalidMessage)?)
            .to(self.to.parse().map_err(|_| PushError::InvalidMessage)?)
            .subject(self.subject)
            .body(format!("{}\n\n{}", notification.title, notification.body))
            .map_err(|_| PushError::InvalidMessage)?;
        let builder = match self.tls {
            SmtpTlsMode::None => SmtpTransport::builder_dangerous(&self.host),
            SmtpTlsMode::Starttls => {
                SmtpTransport::starttls_relay(&self.host).map_err(|_| PushError::InvalidEndpoint)?
            }
            SmtpTlsMode::Implicit => {
                SmtpTransport::relay(&self.host).map_err(|_| PushError::InvalidEndpoint)?
            }
        };
        let mailer = builder
            .port(self.port)
            .credentials(Credentials::new(
                self.username.expose_secret().to_owned(),
                self.password.expose_secret().to_owned(),
            ))
            .timeout(Some(self.timeout))
            .build();
        mailer
            .send(&message)
            .map(|_| ())
            .map_err(|_| PushError::SmtpDelivery)
    }
}

impl Provider for SmtpProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Smtp
    }

    fn send<'a>(&'a self, notification: &'a Notification) -> SendFuture<'a> {
        let provider = self.clone();
        let notification = notification.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || provider.send_blocking(notification))
                .await
                .map_err(|_| PushError::SmtpDelivery)?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_redacts_smtp_credentials() {
        let provider = SmtpProvider::new(
            "smtp.example.com".to_owned(),
            465,
            "sender@example.com".to_owned(),
            "receiver@example.com".to_owned(),
            SecretString::new("smtp-user-secret"),
            SecretString::new("smtp-password-secret"),
            "MihoyoBBSTools RS".to_owned(),
            SmtpTlsMode::Implicit,
            Duration::from_secs(30),
        );
        let debug = format!("{provider:?}");
        assert!(!debug.contains("smtp-user-secret"));
        assert!(!debug.contains("smtp-password-secret"));
    }

    #[test]
    fn invalid_mailbox_is_rejected_before_network_access() {
        let provider = SmtpProvider::new(
            "smtp.example.com".to_owned(),
            465,
            "not-an-email".to_owned(),
            "receiver@example.com".to_owned(),
            SecretString::new("user"),
            SecretString::new("password"),
            "MihoyoBBSTools RS".to_owned(),
            SmtpTlsMode::Implicit,
            Duration::from_secs(30),
        );
        assert_eq!(
            provider.send_blocking(Notification {
                title: "执行结果".to_owned(),
                body: "签到成功".to_owned(),
            }),
            Err(PushError::InvalidMessage)
        );
    }
}
