//! Sending emails over SMTP (or to the log when SMTP is disabled).

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use anyhow::{Context, Result};
use lettre::message::{Mailbox, header::ContentType};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::settings::Settings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub to: String,
    pub subject: String,
    pub html: String,
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Mailer: Send + Sync {
    fn send<'a>(&'a self, settings: &'a Settings, message: Message) -> BoxFuture<'a, Result<()>>;
}

/// Sends through the SMTP server from the settings, or logs when SMTP is disabled.
pub struct SmtpMailer;

impl Mailer for SmtpMailer {
    fn send<'a>(&'a self, settings: &'a Settings, message: Message) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if !settings.smtp.enabled {
                tracing::info!(
                    to = %message.to,
                    subject = %message.subject,
                    "SMTP is disabled, the email was not sent (configure it in the settings)"
                );
                tracing::debug!(body = %message.html, "unsent email body");
                return Ok(());
            }

            let from = Mailbox::new(
                Some(settings.meta.sender_name.clone()),
                settings
                    .meta
                    .sender_address
                    .parse()
                    .context("invalid sender address")?,
            );
            let email = lettre::Message::builder()
                .from(from)
                .to(message.to.parse().context("invalid recipient address")?)
                .subject(message.subject)
                .header(ContentType::TEXT_HTML)
                .body(message.html)?;

            let smtp = &settings.smtp;
            let tls_parameters = TlsParameters::new(smtp.host.clone())?;
            let mut transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&smtp.host)
                .port(smtp.port)
                .tls(if smtp.tls {
                    Tls::Wrapper(tls_parameters)
                } else {
                    Tls::Opportunistic(tls_parameters)
                });
            if !smtp.username.is_empty() {
                transport = transport.credentials(Credentials::new(
                    smtp.username.clone(),
                    smtp.password.clone(),
                ));
            }
            match smtp.auth_method.as_str() {
                "PLAIN" => transport = transport.authentication(vec![Mechanism::Plain]),
                "LOGIN" => transport = transport.authentication(vec![Mechanism::Login]),
                _ => {}
            }
            if !smtp.local_name.trim().is_empty() {
                transport =
                    transport.hello_name(ClientId::Domain(smtp.local_name.trim().to_string()));
            }
            transport
                .build()
                .send(email)
                .await
                .context("failed to send the email")?;
            Ok(())
        })
    }
}

/// Keeps sent emails in memory (for tests).
#[derive(Default)]
pub struct MemoryMailer {
    pub sent: Mutex<Vec<Message>>,
}

impl Mailer for MemoryMailer {
    fn send<'a>(&'a self, _settings: &'a Settings, message: Message) -> BoxFuture<'a, Result<()>> {
        self.sent.lock().expect("mailer lock").push(message);
        Box::pin(async { Ok(()) })
    }
}

/// Fills a template's `{APP_NAME}`, `{APP_URL}` and `{TOKEN}` placeholders.
pub fn render(template: &str, settings: &Settings, token: &str) -> String {
    template
        .replace("{APP_NAME}", &html_escape(&settings.meta.app_name))
        .replace("{APP_URL}", settings.meta.app_url.trim_end_matches('/'))
        .replace("{TOKEN}", token)
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_placeholders() {
        let mut settings = Settings::default();
        settings.meta.app_name = "My <App>".into();
        settings.meta.app_url = "https://example.com/".into();
        assert_eq!(
            render("{APP_NAME}: {APP_URL}/x/{TOKEN}", &settings, "abc"),
            "My &lt;App&gt;: https://example.com/x/abc"
        );
    }
}
