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

/// Fills a template's placeholders: `{APP_NAME}`, `{APP_URL}`, `{TOKEN}`, and slonbase's
/// `{APP_LOGO_URL}` (the logo's address, or nothing), `{APP_LOGO}` (the logo, or the
/// application's name when there's none) and `{ACCENT}` (the accent, as `#rrggbb`).
pub fn render(template: &str, settings: &Settings, token: &str) -> String {
    let name = html_escape(&settings.meta.app_name);
    let base = settings.meta.app_url.trim_end_matches('/');
    let logo_url = if settings.meta.logo.is_empty() {
        String::new()
    } else {
        format!("{base}/api/branding/assets/{}", settings.meta.logo)
    };
    let logo = if logo_url.is_empty() {
        format!("<span style=\"font-size:18px;font-weight:700;color:#1f2328\">{name}</span>")
    } else {
        format!(
            "<img src=\"{}\" alt=\"{name}\" height=\"32\" style=\"display:block;height:32px;width:auto;border:0\">",
            html_escape(&logo_url)
        )
    };
    template
        .replace("{APP_NAME}", &name)
        .replace("{APP_URL}", base)
        .replace("{APP_LOGO_URL}", &html_escape(&logo_url))
        .replace("{APP_LOGO}", &logo)
        .replace(
            "{ACCENT}",
            crate::settings::accent_hex(&settings.meta.accent_color),
        )
        .replace("{TOKEN}", token)
}

/// The email's HTML: `body` in the layout (Settings → Mail), unless the layout is off or the
/// body is a whole document of its own.
pub fn wrap(settings: &Settings, body: &str) -> String {
    let layout = &settings.mail_layout;
    let whole = body
        .get(..body.len().min(512))
        .unwrap_or(body)
        .to_ascii_lowercase();
    if !layout.enabled
        || !layout.html.contains("{CONTENT}")
        || whole.contains("<html")
        || whole.contains("<!doctype")
    {
        return body.to_string();
    }
    layout.html.replacen("{CONTENT}", body, 1)
}

/// Fills `{RECORD:field}` with the record's fields (HTML-escaped). Hidden fields, the password
/// and the token key stay empty. Done last, in one pass, so a record's values can't bring in
/// other placeholders.
pub fn fill_record(template: &str, record: Option<&crate::auth::AuthRecord>) -> String {
    const OPEN: &str = "{RECORD:";
    if !template.contains(OPEN) {
        return template.to_string();
    }
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        let shown = record.and_then(|r| {
            let hidden = matches!(name, "password" | "tokenKey")
                || r.collection.field(name).is_none_or(|f| f.hidden);
            if hidden {
                return None;
            }
            r.values.get(name).map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            })
        });
        out.push_str(&html_escape(&shown.unwrap_or_default()));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A template made into a message's subject and HTML: placeholders filled, the body in the
/// layout, then the record's fields.
pub fn compose(
    settings: &Settings,
    subject: &str,
    body: &str,
    token: &str,
    record: Option<&crate::auth::AuthRecord>,
) -> (String, String) {
    (
        fill_record(&render(subject, settings, token), record),
        fill_record(&render(&wrap(settings, body), settings, token), record),
    )
}

/// The default layout: a light card on a grey page, the logo above it, the application's name
/// and address below; inline styles and tables, as email clients need.
pub const DEFAULT_LAYOUT: &str = r#"<!doctype html>
<html>
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="color-scheme" content="light"></head>
<body style="margin:0;padding:0;background:#f6f8fa;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#1f2328">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="background:#f6f8fa">
<tr><td align="center" style="padding:32px 16px">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="max-width:560px">
<tr><td style="padding:0 4px 18px">{APP_LOGO}</td></tr>
<tr><td style="background:#ffffff;border:1px solid #d0d7de;border-top:3px solid {ACCENT};border-radius:8px;padding:28px 28px 22px;font-size:15px;line-height:1.6">
{CONTENT}
</td></tr>
<tr><td style="padding:16px 4px;font-size:12px;line-height:1.5;color:#59636e">Sent by {APP_NAME} · <a href="{APP_URL}" style="color:#59636e">{APP_URL}</a></td></tr>
</table>
</td></tr>
</table>
</body>
</html>"#;

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
        // no logo: the name instead
        assert!(render("{APP_LOGO}", &settings, "").contains("My &lt;App&gt;</span>"));
        assert_eq!(render("{APP_LOGO_URL}", &settings, ""), "");
        settings.meta.logo = "abcdefghij12345".into();
        assert_eq!(
            render("{APP_LOGO_URL}", &settings, ""),
            "https://example.com/api/branding/assets/abcdefghij12345"
        );
        assert_eq!(render("{ACCENT}", &settings, ""), "#4f7a6c");
    }

    #[test]
    fn layout() {
        let mut settings = Settings::default();
        assert!(wrap(&settings, "<p>Hi</p>").contains("<p>Hi</p>\n</td>"));
        assert!(wrap(&settings, "<p>Hi</p>").starts_with("<!doctype html>"));
        // a document of its own isn't wrapped
        let doc = "<!DOCTYPE html><html><body>x</body></html>";
        assert_eq!(wrap(&settings, doc), doc);
        settings.mail_layout.enabled = false;
        assert_eq!(wrap(&settings, "<p>Hi</p>"), "<p>Hi</p>");
        settings.mail_layout.enabled = true;
        settings.mail_layout.html = "<div>{CONTENT}</div><div>{CONTENT}</div>".into();
        assert_eq!(wrap(&settings, "x"), "<div>x</div><div>{CONTENT}</div>");
    }
}
