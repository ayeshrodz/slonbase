//! Encryption at rest of what slonbase keeps secret in `_slonbase.params`: the settings (SMTP
//! password, S3 secrets) and the signing secret of every token. With an encryption key
//! (`server.encryption_key`), they're stored sealed, so a database dump, a read-only role or a
//! backup doesn't give them away without the key.
//!
//! The format is PocketBase's (`PB_ENCRYPTION`) for a key of exactly 32 characters:
//! AES-256-GCM with the key's 32 bytes as they are, a random 12-byte nonce before the
//! ciphertext, all in standard base64. Stored values are `{"sealed": "<base64>"}`; anything else
//! is a plain value, sealed on the next start with a key.
//!
//! A key **longer** than 32 characters (a random one, such as `openssl rand -base64 48`) is
//! not used as the AES key: each value is sealed under its own key, derived from it with
//! HKDF-SHA256 and a random salt, and stored as `v2.<base64 of salt, nonce and ciphertext>`. The
//! scheme follows from the key's length, so existing installations (32 characters) are
//! unchanged; `slonbase rekey --to <longer key>` moves one to it, and back.

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
/// Keys that start with this are sealed too: the sections of the settings.
pub const SEALED_PREFIX: &str = "settings:";

const KEY_HINT: &str =
    "set the encryption key (--encryption-key, SLONBASE_ENCRYPTION_KEY or server.encryption_key)";

pub struct SecretsKey {
    /// The key itself, for a key of exactly [`KEY_LENGTH`] characters (PocketBase's scheme).
    direct: Option<LessSafeKey>,
    /// A longer key, from which each value's own key is derived.
    master: Option<Vec<u8>>,
}

/// The prefix of a value sealed with a derived key.
const DERIVED_PREFIX: &str = "v2.";
const DERIVED_SALT_LEN: usize = 16;
const DERIVED_INFO: &[u8] = b"slonbase secrets v2";
/// The least a longer key may be (it is one: more than [`KEY_LENGTH`]).
const MAX_KEY_LENGTH: usize = 1024;

struct KeyLen(usize);

impl ring::hkdf::KeyType for KeyLen {
    fn len(&self) -> usize {
        self.0
    }
}

/// The AES key of one value: HKDF-SHA256 of the master key with the value's salt.
fn derive(master: &[u8], salt: &[u8]) -> LessSafeKey {
    let mut out = [0u8; KEY_LENGTH];
    ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, salt)
        .extract(master)
        .expand(&[DERIVED_INFO], KeyLen(KEY_LENGTH))
        .expect("HKDF gives 32 bytes")
        .fill(&mut out)
        .expect("HKDF gives 32 bytes");
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &out).expect("a 32-byte key"))
}

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

/// Checks a key: [`KEY_LENGTH`] characters (PocketBase's scheme), or more (a derived key).
pub fn check_key(key: &str) -> Result<()> {
    if key.len() < KEY_LENGTH {
        bail!(
            "the encryption key must be at least {KEY_LENGTH} characters (it's {}); `openssl rand -hex 16` makes one of 32, `openssl rand -base64 48` a longer one, from which every value's key is derived",
            key.len()
        );
    }
    if key.len() > MAX_KEY_LENGTH {
        bail!("the encryption key must be at most {MAX_KEY_LENGTH} characters");
    }
    Ok(())
}

/// The configuration's key, if it has one.
pub fn from_config(config: &crate::config::Config) -> Result<Option<SecretsKey>> {
    config
        .server
        .encryption_key
        .as_deref()
        .map(SecretsKey::new)
        .transpose()
}

impl SecretsKey {
    pub fn new(key: &str) -> Result<Self> {
        check_key(key)?;
        if key.len() == KEY_LENGTH {
            let key = UnboundKey::new(&AES_256_GCM, key.as_bytes()).expect("a 32-byte key");
            return Ok(Self {
                direct: Some(LessSafeKey::new(key)),
                master: None,
            });
        }
        Ok(Self {
            direct: None,
            master: Some(key.as_bytes().to_vec()),
        })
    }

    /// Seals `plain`: base64 of nonce, ciphertext and tag, or, with a derived key,
    /// `v2.` and base64 of salt, nonce, ciphertext and tag.
    pub fn seal(&self, plain: &[u8]) -> String {
        let random = SystemRandom::new();
        let mut nonce = [0u8; NONCE_LEN];
        random
            .fill(&mut nonce)
            .expect("the system's random number generator");
        let mut data = plain.to_vec();
        let mut out = Vec::new();
        let derived;
        let key = match (&self.direct, &self.master) {
            (Some(key), _) => key,
            (None, Some(master)) => {
                let mut salt = [0u8; DERIVED_SALT_LEN];
                random
                    .fill(&mut salt)
                    .expect("the system's random number generator");
                out.extend_from_slice(&salt);
                derived = derive(master, &salt);
                &derived
            }
            (None, None) => unreachable!("a key has a scheme"),
        };
        key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut data)
            .expect("AES-GCM seals any length we store");
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&data);
        match self.master {
            Some(_) => format!("{DERIVED_PREFIX}{}", B64.encode(out)),
            None => B64.encode(out),
        }
    }

    /// Opens what [`Self::seal`] (or PocketBase) sealed; `None` when the key is wrong, the value
    /// was changed, or it was sealed under the other scheme (see the module's documentation).
    pub fn open(&self, sealed: &str) -> Option<Vec<u8>> {
        let sealed = sealed.trim();
        let (derived, encoded) = match sealed.strip_prefix(DERIVED_PREFIX) {
            Some(rest) => (true, rest),
            None => (false, sealed),
        };
        let data = B64.decode(encoded).ok()?;
        let (key, rest) = match (derived, &self.direct, &self.master) {
            (false, Some(key), _) => (key, data.as_slice()),
            (true, _, Some(master)) => {
                if data.len() < DERIVED_SALT_LEN {
                    return None;
                }
                let (salt, rest) = data.split_at(DERIVED_SALT_LEN);
                return open_with(&derive(master, salt), rest);
            }
            _ => return None,
        };
        open_with(key, rest)
    }
}

/// Opens `nonce ‖ ciphertext ‖ tag` with `key`.
fn open_with(key: &LessSafeKey, data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < NONCE_LEN {
        return None;
    }
    let (nonce, rest) = data.split_at(NONCE_LEN);
    let mut rest = rest.to_vec();
    let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
    let plain = key.open_in_place(nonce, Aad::empty(), &mut rest).ok()?;
    Some(plain.to_vec())
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

/// The OAuth2 client secrets in a collection definition (stored or not).
fn client_secrets(data: &mut Value) -> impl Iterator<Item = &mut Value> {
    data.get_mut("oauth2")
        .and_then(|o| o.get_mut("providers"))
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(|p| p.get_mut("clientSecret"))
}

/// A collection definition's stored form: its OAuth2 client secrets sealed with `key` (as
/// `{"sealed": ...}`, which a definition can't be read as, so nothing uses one unopened).
pub fn seal_collection(data: &mut Value, key: Option<&SecretsKey>) {
    let Some(key) = key else { return };
    for secret in client_secrets(data) {
        if secret.as_str().is_some_and(|s| !s.is_empty()) {
            *secret = to_stored(secret, Some(key));
        }
    }
}

/// Opens a stored collection definition's sealed client secrets.
pub fn open_collection(data: &mut Value, key: Option<&SecretsKey>) -> Result<()> {
    for secret in client_secrets(data) {
        if is_sealed(secret) {
            *secret = from_stored(std::mem::take(secret), key, "OAuth2 client secrets")?;
        }
    }
    Ok(())
}

/// Whether a stored collection definition has a client secret that isn't sealed.
fn has_plain_secret(data: &mut Value) -> bool {
    client_secrets(data).any(|s| s.as_str().is_some_and(|s| !s.is_empty()))
}

/// The prefix of a sealed text column (a project's role password).
const TEXT_PREFIX: &str = "sealed:";

/// A secret text's stored form: sealed with `key`, or as it is without one.
pub fn seal_text(plain: &str, key: Option<&SecretsKey>) -> String {
    match key {
        Some(key) if !plain.is_empty() => format!("{TEXT_PREFIX}{}", key.seal(plain.as_bytes())),
        _ => plain.to_string(),
    }
}

/// A secret text from its stored form; `what` names it in errors.
pub fn open_text(stored: &str, key: Option<&SecretsKey>, what: &str) -> Result<String> {
    let Some(sealed) = stored.strip_prefix(TEXT_PREFIX) else {
        return Ok(stored.to_string());
    };
    let Some(key) = key else {
        return Err(KeyError(format!("the {what} are encrypted: {KEY_HINT}")).into());
    };
    let plain = key.open(sealed).ok_or_else(|| {
        KeyError(format!(
            "the {what} can't be decrypted: the encryption key is wrong"
        ))
    })?;
    String::from_utf8(plain).with_context(|| format!("the decrypted {what} aren't text"))
}

/// Seals what's still plain in the main tables: the collections' client secrets and the
/// projects' role passwords (each row compare-and-set, as [`seal_plain`]).
pub async fn seal_plain_rows(client: &impl GenericClient, key: &SecretsKey) -> Result<usize> {
    let mut sealed = 0;
    for row in client
        .query("SELECT id, data FROM _slonbase.collections", &[])
        .await?
    {
        let (id, old): (String, Value) = (row.get(0), row.get(1));
        let mut data = old.clone();
        if !has_plain_secret(&mut data) {
            continue;
        }
        seal_collection(&mut data, Some(key));
        sealed += client
            .execute(
                "UPDATE _slonbase.collections SET data = $2 WHERE id = $1 AND data = $3",
                &[&id, &data, &old],
            )
            .await? as usize;
    }
    // projects exist in the main installation only
    let projects = client
        .query_opt("SELECT to_regclass('_slonbase.projects') IS NOT NULL", &[])
        .await?
        .is_some_and(|r| r.get::<_, bool>(0));
    if projects {
        for column in project_secret_columns(client).await? {
            for row in client
                .query(
                    &format!(
                        "SELECT name, {column} FROM _slonbase.projects
                         WHERE {column} IS NOT NULL AND {column} <> '' AND {column} NOT LIKE 'sealed:%'"
                    ),
                    &[],
                )
                .await?
            {
                let (name, plain): (String, String) = (row.get(0), row.get(1));
                sealed += client
                    .execute(
                        &format!(
                            "UPDATE _slonbase.projects SET {column} = $2 WHERE name = $1 AND {column} = $3"
                        ),
                        &[&name, &seal_text(&plain, Some(key)), &plain],
                    )
                    .await? as usize;
            }
        }
    }
    Ok(sealed)
}

/// Seals the params of [`SEALED_PARAMS`] that are still plain (a new key, or a restored backup
/// made without one). Values sealed meanwhile by another instance are left alone.
pub async fn seal_plain(client: &impl GenericClient, key: &SecretsKey) -> Result<usize> {
    let mut sealed = 0;
    let rows = client
        .query(
            "SELECT key, value FROM _slonbase.params WHERE key = ANY($1) OR starts_with(key, $2)",
            &[&SEALED_PARAMS, &SEALED_PREFIX],
        )
        .await?;
    for row in rows {
        let (param, value): (String, Value) = (row.get(0), row.get(1));
        if is_sealed(&value) {
            continue;
        }
        sealed += client
            .execute(
                "UPDATE _slonbase.params SET value = $2 WHERE key = $1 AND value = $3",
                &[&param, &to_stored(&value, Some(key)), &value],
            )
            .await? as usize;
    }
    Ok(sealed)
}

/// Re-stores everything sealed (the params of [`SEALED_PARAMS`], the collections' client
/// secrets, the projects' role passwords) under another key (`to: None` stores them plain), in
/// one transaction. Every instance must be stopped, and started again with the new key.
pub async fn rekey(
    client: &mut deadpool_postgres::Client,
    from: Option<&SecretsKey>,
    to: Option<&SecretsKey>,
) -> Result<usize> {
    let tx = client.transaction().await?;
    let mut changed = 0;
    let rows = tx
        .query(
            "SELECT key, value FROM _slonbase.params
             WHERE key = ANY($1) OR starts_with(key, $2) FOR UPDATE",
            &[&SEALED_PARAMS, &SEALED_PREFIX],
        )
        .await?;
    for row in rows {
        let param: String = row.get(0);
        let value = from_stored(row.get(1), from, param_name(&param))?;
        changed += tx
            .execute(
                "UPDATE _slonbase.params SET value = $2 WHERE key = $1",
                &[&param, &to_stored(&value, to)],
            )
            .await? as usize;
    }
    for row in tx
        .query("SELECT id, data FROM _slonbase.collections FOR UPDATE", &[])
        .await?
    {
        let (id, mut data): (String, Value) = (row.get(0), row.get(1));
        open_collection(&mut data, from)?;
        seal_collection(&mut data, to);
        changed += tx
            .execute(
                "UPDATE _slonbase.collections SET data = $2 WHERE id = $1",
                &[&id, &data],
            )
            .await? as usize;
    }
    let projects = tx
        .query_opt("SELECT to_regclass('_slonbase.projects') IS NOT NULL", &[])
        .await?
        .is_some_and(|r| r.get::<_, bool>(0));
    if projects {
        for column in project_secret_columns(&tx).await? {
            for row in tx
                .query(
                    &format!(
                        "SELECT name, {column} FROM _slonbase.projects WHERE {column} IS NOT NULL FOR UPDATE"
                    ),
                    &[],
                )
                .await?
            {
                let (name, stored): (String, String) = (row.get(0), row.get(1));
                let plain = open_text(&stored, from, "project secrets")?;
                changed += tx
                    .execute(
                        &format!("UPDATE _slonbase.projects SET {column} = $2 WHERE name = $1"),
                        &[&name, &seal_text(&plain, to)],
                    )
                    .await? as usize;
            }
        }
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

/// The sealed columns of `_slonbase.projects`: role passwords, and connected databases' URLs
/// (from migration 16 on).
async fn project_secret_columns(client: &impl GenericClient) -> Result<Vec<&'static str>> {
    let has_url = client
        .query_opt(
            "SELECT 1 FROM information_schema.columns
             WHERE table_schema = '_slonbase' AND table_name = 'projects' AND column_name = 'url'",
            &[],
        )
        .await?
        .is_some();
    Ok(if has_url {
        vec!["role_password", "url"]
    } else {
        vec!["role_password"]
    })
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
    fn keys_are_32_characters_or_more() {
        assert!(SecretsKey::new("short").is_err());
        assert!(SecretsKey::new(&KEY[..31]).is_err());
        assert!(SecretsKey::new(&"k".repeat(MAX_KEY_LENGTH + 1)).is_err());
        assert!(check_key(KEY).is_ok());
        // longer: a derived key
        assert!(check_key(&format!("{KEY}x")).is_ok());
        assert!(
            SecretsKey::new(&format!("{KEY}x"))
                .unwrap()
                .master
                .is_some()
        );
        assert!(SecretsKey::new(KEY).unwrap().direct.is_some());
    }

    #[test]
    fn a_longer_key_seals_each_value_under_a_key_of_its_own() {
        let long = format!("{KEY}-and-more-to-make-it-a-longer-key");
        let key = SecretsKey::new(&long).unwrap();
        let sealed = key.seal(b"hello");
        assert!(sealed.starts_with("v2."), "{sealed}");
        assert_ne!(
            sealed,
            key.seal(b"hello"),
            "a fresh salt and nonce each time"
        );
        assert_eq!(key.open(&sealed).unwrap(), b"hello");
        // not readable with another key, with the 32 characters it starts with, or changed
        let other = SecretsKey::new(&format!("{KEY}-another-longer-key-of-the-same-sort")).unwrap();
        assert!(other.open(&sealed).is_none());
        assert!(SecretsKey::new(KEY).unwrap().open(&sealed).is_none());
        let mut bytes = B64.decode(sealed.strip_prefix("v2.").unwrap()).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        assert!(key.open(&format!("v2.{}", B64.encode(bytes))).is_none());
        assert!(key.open("v2.").is_none());
        assert!(key.open("v2.AAAA").is_none());
        // a value sealed under the PocketBase scheme isn't opened by a derived key (rekey moves
        // them), nor the other way round
        let direct = SecretsKey::new(KEY).unwrap();
        let old = direct.seal(b"hello");
        assert!(!old.starts_with("v2."));
        assert!(key.open(&old).is_none());
        assert_eq!(direct.open(&old).unwrap(), b"hello");
    }

    #[test]
    fn stored_values_under_a_longer_key() {
        let key = SecretsKey::new(&format!("{KEY}-and-more-to-make-it-a-longer-key")).unwrap();
        let value = json!({ "smtp": { "password": "p" } });
        let stored = to_stored(&value, Some(&key));
        assert!(is_sealed(&stored));
        assert!(!stored.to_string().contains("\"p\""));
        assert_eq!(from_stored(stored, Some(&key), "settings").unwrap(), value);
        // text columns too
        let text = seal_text("a-role-password", Some(&key));
        assert!(text.starts_with("sealed:v2."), "{text}");
        assert_eq!(
            open_text(&text, Some(&key), "project secrets").unwrap(),
            "a-role-password"
        );
    }

    // the layout PocketBase's `security.Encrypt` writes (`gcm.Seal(nonce, nonce, data, nil)`
    // in standard base64: the nonce, then ciphertext and tag), so its settings can be imported
    #[test]
    fn opens_the_pocketbase_layout() {
        let key = SecretsKey::new(KEY).unwrap();
        let nonce = [7u8; NONCE_LEN];
        let mut data = br#"{"a":1}"#.to_vec();
        key.direct
            .as_ref()
            .unwrap()
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
