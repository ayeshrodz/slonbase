//! Encryption at rest of what slonbase keeps secret in `_slonbase.params`: the settings (SMTP
//! password, S3 secrets) and the signing secret of every token. With an encryption key
//! (`server.encryption_key`), they're stored sealed, so a database dump, a read-only role or a
//! backup doesn't give them away without the key.
//!
//! The format is PocketBase's (`PB_ENCRYPTION`): AES-256-GCM with the key's 32 bytes as they
//! are, a random 12-byte nonce before the ciphertext, all in standard base64. Stored values are
//! `{"sealed": "<base64>"}`; anything else is a plain value, sealed on the next start with a key.

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use deadpool_postgres::GenericClient;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Value, json};

/// The key's length, in bytes (as PocketBase's).
pub const KEY_LENGTH: usize = 32;
/// The `_slonbase.params` keys stored sealed.
pub const SEALED_PARAMS: &[&str] = &["settings", "auth_secret"];

const KEY_HINT: &str =
    "set the encryption key (--encryption-key, SLONBASE_ENCRYPTION_KEY or server.encryption_key)";

pub struct SecretsKey(LessSafeKey);

/// The stored values need a key that wasn't given, or another one: a configuration error,
/// which retrying won't fix.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct KeyError(pub String);

impl std::fmt::Debug for SecretsKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretsKey(..)")
    }
}

/// Checks a key: exactly [`KEY_LENGTH`] bytes.
pub fn check_key(key: &str) -> Result<()> {
    if key.len() != KEY_LENGTH {
        bail!(
            "the encryption key must be exactly {KEY_LENGTH} characters (it's {}); `openssl rand -hex 16` makes one",
            key.len()
        );
    }
    Ok(())
}

impl SecretsKey {
    pub fn new(key: &str) -> Result<Self> {
        check_key(key)?;
        let key = UnboundKey::new(&AES_256_GCM, key.as_bytes()).expect("a 32-byte key");
        Ok(Self(LessSafeKey::new(key)))
    }

    /// Seals `plain` (base64 of nonce, ciphertext and tag).
    pub fn seal(&self, plain: &[u8]) -> String {
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce)
            .expect("the system's random number generator");
        let mut data = plain.to_vec();
        self.0
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut data)
            .expect("AES-GCM seals any length we store");
        let mut out = nonce.to_vec();
        out.extend_from_slice(&data);
        B64.encode(out)
    }

    /// Opens what [`Self::seal`] (or PocketBase) sealed; `None` when the key is wrong or the
    /// value was changed.
    pub fn open(&self, sealed: &str) -> Option<Vec<u8>> {
        let data = B64.decode(sealed.trim()).ok()?;
        if data.len() < NONCE_LEN {
            return None;
        }
        let (nonce, rest) = data.split_at(NONCE_LEN);
        let mut rest = rest.to_vec();
        let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
        let plain = self.0.open_in_place(nonce, Aad::empty(), &mut rest).ok()?;
        Some(plain.to_vec())
    }
}

/// The stored form of a param's value: sealed with the key, or as it is without one.
pub fn to_stored(value: &Value, key: Option<&SecretsKey>) -> Value {
    match key {
        Some(key) => json!({ "sealed": key.seal(value.to_string().as_bytes()) }),
        None => value.clone(),
    }
}

/// Whether a stored value is sealed.
pub fn is_sealed(stored: &Value) -> bool {
    stored
        .as_object()
        .is_some_and(|o| o.len() == 1 && o.get("sealed").is_some_and(Value::is_string))
}

/// A param's value from its stored form; `what` names it in errors.
pub fn from_stored(stored: Value, key: Option<&SecretsKey>, what: &str) -> Result<Value> {
    if !is_sealed(&stored) {
        return Ok(stored);
    }
    let Some(key) = key else {
        return Err(KeyError(format!("the {what} are encrypted: {KEY_HINT}")).into());
    };
    let sealed = stored["sealed"].as_str().expect("checked");
    let plain = key.open(sealed).ok_or_else(|| {
        KeyError(format!(
            "the {what} can't be decrypted: the encryption key is wrong"
        ))
    })?;
    serde_json::from_slice(&plain).with_context(|| format!("the decrypted {what} aren't JSON"))
}

/// Seals the params of [`SEALED_PARAMS`] that are still plain (a new key, or a restored backup
/// made without one). Values sealed meanwhile by another instance are left alone.
pub async fn seal_plain(client: &impl GenericClient, key: &SecretsKey) -> Result<usize> {
    let mut sealed = 0;
    for param in SEALED_PARAMS {
        let Some(row) = client
            .query_opt(
                "SELECT value FROM _slonbase.params WHERE key = $1",
                &[param],
            )
            .await?
        else {
            continue;
        };
        let value: Value = row.get(0);
        if is_sealed(&value) {
            continue;
        }
        sealed += client
            .execute(
                "UPDATE _slonbase.params SET value = $2 WHERE key = $1 AND value = $3",
                &[param, &to_stored(&value, Some(key)), &value],
            )
            .await? as usize;
    }
    Ok(sealed)
}

/// Re-stores the params of [`SEALED_PARAMS`] under another key (`to: None` stores them plain),
/// in one transaction. Every instance must be stopped, and started again with the new key.
pub async fn rekey(
    client: &mut deadpool_postgres::Client,
    from: Option<&SecretsKey>,
    to: Option<&SecretsKey>,
) -> Result<usize> {
    let tx = client.transaction().await?;
    let mut changed = 0;
    for param in SEALED_PARAMS {
        let Some(row) = tx
            .query_opt(
                "SELECT value FROM _slonbase.params WHERE key = $1 FOR UPDATE",
                &[param],
            )
            .await?
        else {
            continue;
        };
        let value = from_stored(row.get(0), from, param_name(param))?;
        changed += tx
            .execute(
                "UPDATE _slonbase.params SET value = $2 WHERE key = $1",
                &[param, &to_stored(&value, to)],
            )
            .await? as usize;
    }
    tx.commit().await?;
    Ok(changed)
}

/// How a param is named in errors.
pub fn param_name(param: &str) -> &'static str {
    match param {
        "auth_secret" => "token signing secret",
        _ => "settings",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn seals_and_opens() {
        let key = SecretsKey::new(KEY).unwrap();
        let sealed = key.seal(b"hello");
        assert_ne!(sealed, key.seal(b"hello"), "a fresh nonce each time");
        assert_eq!(key.open(&sealed).unwrap(), b"hello");
        let other = SecretsKey::new("fedcba9876543210fedcba9876543210").unwrap();
        assert!(other.open(&sealed).is_none());
        // a changed byte fails
        let mut bytes = B64.decode(&sealed).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        assert!(key.open(&B64.encode(bytes)).is_none());
        assert!(key.open("not base64!").is_none());
        assert!(key.open("").is_none());
    }

    #[test]
    fn keys_must_be_32_bytes() {
        assert!(SecretsKey::new("short").is_err());
        assert!(SecretsKey::new(&format!("{KEY}x")).is_err());
        assert!(check_key(KEY).is_ok());
    }

    // the layout PocketBase's `security.Encrypt` writes (`gcm.Seal(nonce, nonce, data, nil)`
    // in standard base64: the nonce, then ciphertext and tag), so its settings can be imported
    #[test]
    fn opens_the_pocketbase_layout() {
        let key = SecretsKey::new(KEY).unwrap();
        let nonce = [7u8; NONCE_LEN];
        let mut data = br#"{"a":1}"#.to_vec();
        key.0
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut data)
            .unwrap();
        let mut layout = nonce.to_vec();
        layout.extend_from_slice(&data);
        assert_eq!(key.open(&B64.encode(layout)).unwrap(), br#"{"a":1}"#);
    }

    #[test]
    fn stored_values() {
        let key = SecretsKey::new(KEY).unwrap();
        let value = json!({ "smtp": { "password": "p" } });
        let stored = to_stored(&value, Some(&key));
        assert!(is_sealed(&stored));
        assert!(!stored.to_string().contains("\"p\""));
        assert_eq!(
            from_stored(stored.clone(), Some(&key), "settings").unwrap(),
            value
        );
        let err = from_stored(stored, None, "settings")
            .unwrap_err()
            .to_string();
        assert!(err.contains("encrypted"), "{err}");
        // plain values pass through, with or without a key
        assert_eq!(to_stored(&value, None), value);
        assert_eq!(
            from_stored(value.clone(), Some(&key), "settings").unwrap(),
            value
        );
        assert!(!is_sealed(&json!({ "sealed": "x", "other": 1 })));
    }
}
