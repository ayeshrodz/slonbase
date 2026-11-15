//! Signed webhooks: a JSON `POST` whose receiver can check that it came from here and was not
//! changed on the way.
//!
//! The signature follows the [Standard Webhooks](https://www.standardwebhooks.com/) scheme, so
//! a receiver verifies it with a library that exists for most languages, or in a few lines:
//!
//! - `webhook-id`: the delivery's id, the same on every retry (so a receiver can drop one it
//!   already handled);
//! - `webhook-timestamp`: when this attempt was sent, in seconds since 1970 (a receiver
//!   refuses one that is too old, which stops replays);
//! - `webhook-signature`: `v1,` and the base64 of HMAC-SHA256 over `{id}.{timestamp}.{body}`,
//!   keyed with the secret's bytes. While a secret is being replaced there are two, separated
//!   by a space: one for the new secret and one for the old.
//!
//! A secret is `whsec_` and the base64 of 32 random bytes. It is made here, never typed, and
//! the request goes through [`super::outbound`] like every other: public addresses only. It is
//! only sent over https, since what a webhook carries is the installation's data.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::{Failure, outbound};

const PREFIX: &str = "whsec_";
/// How long the secret that was replaced keeps signing, next to the new one.
pub const ROTATION_HOURS: i64 = 24;

/// A new signing secret.
pub fn new_secret() -> String {
    let mut key = [0u8; 32];
    rand::fill(&mut key[..]);
    format!("{PREFIX}{}", STANDARD.encode(key))
}

/// The key of a secret, when it is one: 24 to 64 bytes after the prefix.
fn key(secret: &str) -> Option<Vec<u8>> {
    let key = STANDARD.decode(secret.strip_prefix(PREFIX)?).ok()?;
    (24..=64).contains(&key.len()).then_some(key)
}

/// Whether `secret` is a signing secret.
pub fn valid_secret(secret: &str) -> bool {
    key(secret).is_some()
}

/// The `webhook-signature` header for a delivery: one signature per secret.
pub fn signature(secrets: &[&str], id: &str, timestamp: i64, body: &str) -> String {
    let signed = format!("{id}.{timestamp}.{body}");
    secrets
        .iter()
        .filter_map(|secret| key(secret))
        .map(|key| {
            let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &key);
            let tag = ring::hmac::sign(&key, signed.as_bytes());
            format!("v1,{}", STANDARD.encode(tag.as_ref()))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Checks the address of a webhook when it is saved: https, and public.
pub async fn check(url: &str) -> Result<(), String> {
    if !reqwest::Url::parse(url.trim()).is_ok_and(|u| u.scheme() == "https") {
        return Err("A signed webhook goes to an https:// address.".into());
    }
    outbound::check(url.trim()).await
}

/// Sends `body` to `url`, signed with `secrets` (the current one first). `id` names the
/// delivery: the same for every attempt at it.
pub async fn deliver(
    url: &str,
    secrets: &[&str],
    id: &str,
    body: &str,
    timeout: Duration,
) -> Result<outbound::Reply, Failure> {
    let url = url.trim();
    if !reqwest::Url::parse(url).is_ok_and(|u| u.scheme() == "https") {
        return Err(Failure::permanent(
            "A signed webhook goes to an https:// address.",
        ));
    }
    let timestamp = chrono::Utc::now().timestamp();
    let headers = [
        ("content-type".to_string(), "application/json".to_string()),
        ("webhook-id".to_string(), id.to_string()),
        ("webhook-timestamp".to_string(), timestamp.to_string()),
        (
            "webhook-signature".to_string(),
            signature(secrets, id, timestamp, body),
        ),
    ];
    outbound::send(outbound::Request {
        method: "POST",
        url,
        headers: &headers,
        body: Some(body),
        timeout,
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_is_the_standard_one() {
        // the example of the Standard Webhooks specification
        let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
        let body = r#"{"test": 2432232314}"#;
        assert_eq!(
            signature(&[secret], "msg_p5jXN8AQM9LWM0D4loKWxJek", 1614265330, body),
            "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="
        );
        // two secrets while one is being replaced
        let other = new_secret();
        let both = signature(
            &[&other, secret],
            "msg_p5jXN8AQM9LWM0D4loKWxJek",
            1614265330,
            body,
        );
        let parts: Vec<&str> = both.split(' ').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1], "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=");
        assert_ne!(parts[0], parts[1]);
    }

    #[test]
    fn a_secret_is_made_here() {
        let secret = new_secret();
        assert!(valid_secret(&secret));
        assert_ne!(secret, new_secret());
        for bad in ["", "whsec_", "whsec_!!!", "secret", "whsec_YQ=="] {
            assert!(!valid_secret(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn only_https() {
        assert!(check("http://93.184.216.34/hook").await.is_err());
        let sent = deliver(
            "http://93.184.216.34/hook",
            &[&new_secret()],
            "msg_1",
            "{}",
            Duration::from_secs(1),
        )
        .await;
        assert!(sent.is_err_and(|f| !f.retry));
    }
}
