//! Superuser accounts: records of the `_superusers` system collection. Setup uses these
//! helpers before the app is running; once it is, superusers are managed like any other
//! auth records.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use anyhow::Result;
use deadpool_postgres::Pool;

use crate::db::describe_pool_error;
use crate::ids;
use crate::migrations::SCHEMA_LOCK_KEY;

pub const MIN_PASSWORD_LENGTH: usize = 10;
/// bcrypt only uses the first 72 bytes of the input.
pub const MAX_PASSWORD_BYTES: usize = 72;
const MAX_EMAIL_LENGTH: usize = 255;
const BCRYPT_COST: u32 = 12;
const TABLE: &str = r#"slonbase."_superusers""#;

/// Hash compared against when an email doesn't exist,
/// so failed logins take the same time either way.
static DUMMY_HASH: LazyLock<String> =
    LazyLock::new(|| bcrypt::hash(ids::random_secret(16), BCRYPT_COST).expect("bcrypt hash"));

/// Field validation errors, keyed by field name.
pub type FieldErrors = BTreeMap<&'static str, String>;

pub fn validate(email: &str, password: &str) -> FieldErrors {
    let mut errors = FieldErrors::new();

    let email = email.trim();
    let valid_email = email.len() <= MAX_EMAIL_LENGTH
        && match email.split_once('@') {
            Some((local, domain)) => {
                !local.is_empty()
                    && !domain.is_empty()
                    && !domain.contains('@')
                    && !email.chars().any(char::is_whitespace)
            }
            None => false,
        };
    if !valid_email {
        errors.insert("email", "Must be a valid email address.".into());
    }

    if password.chars().count() < MIN_PASSWORD_LENGTH {
        errors.insert(
            "password",
            format!("Must be at least {MIN_PASSWORD_LENGTH} characters."),
        );
    } else if password.len() > MAX_PASSWORD_BYTES {
        errors.insert(
            "password",
            format!("Must be at most {MAX_PASSWORD_BYTES} bytes."),
        );
    }

    errors
}

async fn hash_password(password: String) -> Result<String> {
    Ok(tokio::task::spawn_blocking(move || bcrypt::hash(password, BCRYPT_COST)).await??)
}

async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || bcrypt::verify(password, &hash).unwrap_or(false))
        .await
        .unwrap_or(false)
}

pub async fn count(pool: &Pool) -> Result<i64> {
    let client = pool.get().await.map_err(describe_pool_error)?;
    let row = client
        .query_one(&format!("SELECT count(*) FROM {TABLE}"), &[])
        .await?;
    Ok(row.get(0))
}

pub enum CreateFirst {
    Created,
    /// Another superuser already exists, nothing was created.
    AlreadyExists,
}

/// Creates the first superuser, unless one already exists. Serialized with an
/// advisory lock so instances racing through setup can't both create one.
pub async fn create_first(pool: &Pool, email: &str, password: &str) -> Result<CreateFirst> {
    let hash = hash_password(password.to_string()).await?;

    let mut client = pool.get().await.map_err(describe_pool_error)?;
    let tx = client.transaction().await?;
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&SCHEMA_LOCK_KEY])
        .await?;

    let existing: i64 = tx
        .query_one(&format!("SELECT count(*) FROM {TABLE}"), &[])
        .await?
        .get(0);
    if existing > 0 {
        return Ok(CreateFirst::AlreadyExists);
    }

    tx.execute(
        &format!(
            r#"INSERT INTO {TABLE} (id, email, password, "tokenKey", "emailVisibility", verified, created, updated)
               VALUES ($1, $2, $3, $4, FALSE, TRUE, now(), now())"#
        ),
        &[
            &ids::random_id(ids::DEFAULT_ID_LENGTH),
            &email.trim(),
            &hash,
            &ids::generate_from_pattern("[a-zA-Z0-9]{50}").expect("valid pattern"),
        ],
    )
    .await?;
    // a new installation: unlike PocketBase's, it starts with rate limits on
    crate::settings::Settings::init(&tx).await?;
    tx.commit().await?;

    Ok(CreateFirst::Created)
}

/// Checks a superuser's credentials.
pub async fn verify(pool: &Pool, email: &str, password: &str) -> Result<bool> {
    let client = pool.get().await.map_err(describe_pool_error)?;
    let row = client
        .query_opt(
            &format!("SELECT password FROM {TABLE} WHERE lower(email) = lower($1)"),
            &[&email.trim()],
        )
        .await?;

    let (hash, found) = match row {
        Some(row) => (row.get::<_, String>(0), true),
        None => (DUMMY_HASH.clone(), false),
    };
    let valid = verify_password(password.to_string(), hash).await;
    Ok(found && valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        assert!(validate("admin@example.com", "0123456789").is_empty());
        assert!(validate("  admin@example.com ", "0123456789").is_empty());

        for email in ["", "admin", "@example.com", "admin@", "a@b@c", "ad min@x.y"] {
            assert!(
                validate(email, "0123456789").contains_key("email"),
                "{email}"
            );
        }

        assert!(validate("a@b.c", "short").contains_key("password"));
        assert!(validate("a@b.c", &"x".repeat(73)).contains_key("password"));
        assert!(validate("a@b.c", &"x".repeat(72)).is_empty());
    }
}
