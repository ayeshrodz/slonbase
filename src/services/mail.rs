//! Email to addresses a user typed in (a job's recipients, the people told about a failure).
//!
//! The checks live here once: how many addresses, what an address may look like, that nothing
//! typed can add a header, and that text is sent as text.

use std::sync::Arc;

use serde_json::Value;

use crate::core::Core;

use super::Failure;

/// The most addresses one email can go to.
pub const MAX_RECIPIENTS: usize = 10;
/// What is said when email isn't set up.
pub const DISABLED: &str = "Email isn't set up: turn on SMTP in Settings → Mail first.";

/// Addresses as typed: a list, or one text with commas, semicolons, spaces or lines between.
pub fn recipients(value: Option<&Value>) -> Vec<String> {
    let raw: Vec<String> = match value {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => vec![],
    };
    raw.iter()
        .flat_map(|s| s.split([',', ';', '\n', ' ']))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Whether a text is one plain line (no control characters), as a subject has to be.
pub fn plain_line(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

/// One to [`MAX_RECIPIENTS`] plausible addresses, none with a control character (no header
/// injection).
pub fn check_addresses(to: &[String]) -> Result<(), String> {
    if to.is_empty() || to.len() > MAX_RECIPIENTS {
        return Err(format!("One to {MAX_RECIPIENTS} email addresses."));
    }
    match to.iter().find(|a| {
        a.len() > 254
            || !plain_line(a)
            || a.matches('@').count() != 1
            || a.starts_with('@')
            || a.ends_with('@')
            || !a.rsplit('@').next().is_some_and(|d| d.contains('.'))
    }) {
        Some(bad) => Err(format!("{bad:?} is not an email address.")),
        None => Ok(()),
    }
}

/// Whether email can be sent (SMTP is on in the settings).
pub fn enabled(core: &Core) -> bool {
    core.settings().smtp.enabled
}

/// A text as the HTML of an email: markup is shown as typed, lines are kept.
pub fn text_to_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "<br>")
}

/// Sends a plain-text email to one address and waits for the result.
pub async fn send_text_to(
    core: &Arc<Core>,
    to: &str,
    subject: &str,
    text: &str,
) -> Result<(), Failure> {
    if !enabled(core) {
        return Err(Failure::permanent(
            "Email isn't set up: turn on SMTP in Settings → Mail.",
        ));
    }
    core.send_mail_now(crate::mail::Message {
        to: to.to_string(),
        subject: subject.to_string(),
        html: text_to_html(text),
    })
    .await
    .map_err(|e| Failure::transient(format!("Sending to {to} failed: {e}")))
}

/// Sends a plain-text email to each address in turn, stopping at the first that fails. Returns
/// how many were sent.
pub async fn send_text(
    core: &Arc<Core>,
    to: &[String],
    subject: &str,
    text: &str,
) -> Result<usize, Failure> {
    if !enabled(core) {
        return Err(Failure::permanent(
            "Email isn't set up: turn on SMTP in Settings → Mail.",
        ));
    }
    let mut sent = 0;
    for address in to {
        send_text_to(core, address, subject, text).await?;
        sent += 1;
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn addresses_are_read_and_checked() {
        let list = recipients(Some(&json!("a@b.co, c@d.co;e@f.co\ng@h.co")));
        assert_eq!(list, ["a@b.co", "c@d.co", "e@f.co", "g@h.co"]);
        assert_eq!(recipients(Some(&json!(["a@b.co", 3]))), ["a@b.co"]);
        assert!(recipients(None).is_empty());

        assert!(check_addresses(&list).is_ok());
        for bad in [
            "nope",
            "@b.co",
            "a@",
            "a@b",
            "a@@b.co",
            "a@b.co\r\nBcc: x@y.co",
        ] {
            assert!(check_addresses(&[bad.to_string()]).is_err(), "{bad:?}");
        }
        assert!(check_addresses(&[]).is_err());
        let many: Vec<String> = (0..=MAX_RECIPIENTS).map(|i| format!("u{i}@b.co")).collect();
        assert!(check_addresses(&many).is_err());
    }

    #[test]
    fn text_is_sent_as_text() {
        assert_eq!(
            text_to_html("Hello <b>team</b> & co\nsecond line"),
            "Hello &lt;b&gt;team&lt;/b&gt; &amp; co<br>second line"
        );
        assert!(plain_line("Weekly report") && !plain_line("Hi\nBcc: x@y.co"));
    }
}
