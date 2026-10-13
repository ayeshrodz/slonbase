//! Authentication: HS256 JWTs for superusers and auth collection records, and the
//! request extractor that resolves the caller.
//!
//! Each token is signed with a key derived from the installation secret, the collection,
//! the token type and the record's `token_key`. A token of one type can't be used as
//! another, and rotating a record's token key (on password or email change) invalidates
//! all of its tokens, which also makes reset and email change tokens single-use.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Map, Value, json};
use sha2::Sha256;

use crate::app::App;
use crate::collections::model::{
    Collection, FIELD_EMAIL, FIELD_TOKEN_KEY, SUPERUSERS_COLLECTION_ID, SUPERUSERS_COLLECTION_NAME,
};
use crate::core::Core;
use crate::datetime;
use crate::error::ApiError;
use crate::filter::compile::AuthInfo;
use crate::records::store;
use crate::records::values::Values;

/// Superuser tokens are valid for 1 day (PocketBase's default).
pub const SUPERUSER_TOKEN_DURATION_SECS: i64 = 86_400;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    Auth,
    Verification,
    PasswordReset,
    EmailChange,
    File,
}

impl TokenType {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenType::Auth => "auth",
            TokenType::Verification => "verification",
            TokenType::PasswordReset => "passwordReset",
            TokenType::EmailChange => "emailChange",
            TokenType::File => "file",
        }
    }
}

fn signing_key(core: &Core, collection_id: &str, kind: TokenType, token_key: &str) -> Vec<u8> {
    let mut key = core.auth_secret().to_vec();
    for part in [collection_id, kind.as_str(), token_key] {
        key.push(0);
        key.extend_from_slice(part.as_bytes());
    }
    key
}

fn mac(key: &[u8], message: &str) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    mac
}

/// Signs a token. `claims` must contain `id` and `collectionId`; `type` and `exp` are added.
pub fn sign(
    core: &Core,
    kind: TokenType,
    token_key: &str,
    mut claims: Map<String, Value>,
    duration_secs: i64,
) -> String {
    claims.insert("type".into(), Value::String(kind.as_str().into()));
    claims.insert(
        "exp".into(),
        Value::from(chrono::Utc::now().timestamp() + duration_secs),
    );
    let collection_id = claims
        .get("collectionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let header = B64.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = B64.encode(Value::Object(claims).to_string());
    let message = format!("{header}.{payload}");
    let signature = mac(
        &signing_key(core, &collection_id, kind, token_key),
        &message,
    )
    .finalize()
    .into_bytes();
    format!("{message}.{}", B64.encode(signature))
}

/// Decoded but **unverified** claims of an unexpired token of the given type.
fn unverified_claims(token: &str, kind: TokenType) -> Option<Map<String, Value>> {
    let mut parts = token.split('.');
    let (header, payload, _signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let header: Value = serde_json::from_slice(&B64.decode(header).ok()?).ok()?;
    if header.get("alg")?.as_str()? != "HS256" {
        return None;
    }
    let claims: Map<String, Value> = serde_json::from_slice(&B64.decode(payload).ok()?).ok()?;
    if claims.get("type")?.as_str()? != kind.as_str()
        || claims.get("exp")?.as_i64()? <= chrono::Utc::now().timestamp()
    {
        return None;
    }
    Some(claims)
}

fn verify_signature(
    core: &Core,
    token: &str,
    collection_id: &str,
    kind: TokenType,
    token_key: &str,
) -> bool {
    let Some(dot) = token.rfind('.') else {
        return false;
    };
    let Ok(signature) = B64.decode(&token[dot + 1..]) else {
        return false;
    };
    mac(
        &signing_key(core, collection_id, kind, token_key),
        &token[..dot],
    )
    .verify_slice(&signature)
    .is_ok()
}

// ---------------------------------------------------------------------------
// superusers

#[derive(Debug, Clone)]
pub struct Superuser {
    pub id: String,
    pub email: String,
    pub created: String,
    pub updated: String,
    token_key: String,
}

impl Superuser {
    pub fn to_json(&self) -> Value {
        json!({
            "collectionId": SUPERUSERS_COLLECTION_ID,
            "collectionName": SUPERUSERS_COLLECTION_NAME,
            "id": self.id,
            "email": self.email,
            "emailVisibility": false,
            "verified": true,
            "created": self.created,
            "updated": self.updated,
        })
    }

    /// A short-lived token for protected files.
    pub fn issue_file_token(&self, core: &Core) -> String {
        let mut claims = Map::new();
        claims.insert("id".into(), Value::String(self.id.clone()));
        claims.insert("collectionId".into(), SUPERUSERS_COLLECTION_ID.into());
        sign(core, TokenType::File, &self.token_key, claims, 180)
    }

    pub fn issue_token(&self, core: &Core) -> String {
        let mut claims = Map::new();
        claims.insert("id".into(), Value::String(self.id.clone()));
        claims.insert("collectionId".into(), SUPERUSERS_COLLECTION_ID.into());
        claims.insert("refreshable".into(), Value::Bool(true));
        sign(
            core,
            TokenType::Auth,
            &self.token_key,
            claims,
            SUPERUSER_TOKEN_DURATION_SECS,
        )
    }
}

pub async fn find_superuser(core: &Core, condition: &str, value: &str) -> Option<Superuser> {
    let client = core.pool.get().await.ok()?;
    let row = client
        .query_opt(
            &format!(
                "SELECT id, email, token_key, {}, {} FROM _slonbase.superusers WHERE {condition}",
                datetime::sql_format("created"),
                datetime::sql_format("updated"),
            ),
            &[&value],
        )
        .await
        .ok()??;
    Some(Superuser {
        id: row.get(0),
        email: row.get(1),
        token_key: row.get(2),
        created: row.get(3),
        updated: row.get(4),
    })
}

// ---------------------------------------------------------------------------
// auth collection records

/// A record of an auth collection, with all its values (including secrets).
#[derive(Debug, Clone)]
pub struct AuthRecord {
    pub collection: Arc<Collection>,
    pub values: Values,
    /// False when authenticated with a non-refreshable (impersonation) token.
    pub refreshable: bool,
}

impl AuthRecord {
    pub fn str(&self, field: &str) -> &str {
        self.values
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    pub fn id(&self) -> &str {
        self.str("id")
    }

    pub fn email(&self) -> &str {
        self.str(FIELD_EMAIL)
    }

    pub fn token_key(&self) -> &str {
        self.str(FIELD_TOKEN_KEY)
    }

    /// Signs a token of this record with extra claims.
    pub fn issue(
        &self,
        core: &Core,
        kind: TokenType,
        extra: Map<String, Value>,
        duration_secs: i64,
    ) -> String {
        let mut claims = extra;
        claims.insert("id".into(), Value::String(self.id().to_string()));
        claims.insert(
            "collectionId".into(),
            Value::String(self.collection.id.clone()),
        );
        sign(core, kind, self.token_key(), claims, duration_secs)
    }

    /// An auth token; `refreshable` is false for impersonation tokens.
    pub fn issue_auth_token(
        &self,
        core: &Core,
        duration_secs: Option<i64>,
        refreshable: bool,
    ) -> String {
        let duration = duration_secs.unwrap_or_else(|| {
            self.collection
                .auth_options()
                .map(|a| a.auth_token.duration)
                .unwrap_or(432_000)
        });
        let mut claims = Map::new();
        claims.insert("refreshable".into(), Value::Bool(refreshable));
        self.issue(core, TokenType::Auth, claims, duration)
    }

    pub fn info(&self) -> AuthInfo {
        AuthInfo {
            id: self.id().to_string(),
            collection_id: self.collection.id.clone(),
            collection_name: self.collection.name.clone(),
            values: self.values.clone(),
        }
    }
}

/// Verifies a record token of the given type and returns its record and claims.
pub async fn verify_record_token(
    core: &Core,
    token: &str,
    kind: TokenType,
) -> Option<(AuthRecord, Map<String, Value>)> {
    let claims = unverified_claims(token, kind)?;
    let collection_id = claims.get("collectionId")?.as_str()?;
    let id = claims.get("id")?.as_str()?;

    let collection = core.registry().by_id(collection_id).cloned()?;
    if !collection.is_auth() {
        return None;
    }
    let client = core.pool.get().await.ok()?;
    let values = store::find_by_id(&client, &collection, id).await.ok()??;
    let refreshable = claims
        .get("refreshable")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let record = AuthRecord {
        collection,
        values,
        refreshable,
    };
    if !verify_signature(core, token, collection_id, kind, record.token_key()) {
        return None;
    }
    Some((record, claims))
}

// ---------------------------------------------------------------------------
// the request extractor

/// The authenticated caller. Invalid or expired tokens make the request a guest
/// request (like PocketBase), rather than failing it.
#[derive(Debug, Clone, Default)]
pub struct RequestAuth {
    pub superuser: Option<Superuser>,
    pub record: Option<AuthRecord>,
}

impl RequestAuth {
    pub fn is_superuser(&self) -> bool {
        self.superuser.is_some()
    }

    pub fn require_superuser(&self) -> Result<&Superuser, ApiError> {
        self.superuser.as_ref().ok_or_else(|| {
            ApiError::unauthorized("The request requires valid superuser authorization token.")
        })
    }

    /// The caller as seen by rules (`@request.auth.*`).
    pub fn info(&self) -> Option<AuthInfo> {
        if let Some(record) = &self.record {
            return Some(record.info());
        }
        self.superuser.as_ref().map(|s| {
            let mut values = Map::new();
            values.insert("id".into(), Value::String(s.id.clone()));
            values.insert("email".into(), Value::String(s.email.clone()));
            AuthInfo {
                id: s.id.clone(),
                collection_id: SUPERUSERS_COLLECTION_ID.into(),
                collection_name: SUPERUSERS_COLLECTION_NAME.into(),
                values,
            }
        })
    }

    /// Whether the caller is this record of this collection.
    pub fn is_record(&self, collection_id: &str, id: &str) -> bool {
        self.record
            .as_ref()
            .is_some_and(|r| r.collection.id == collection_id && r.id() == id)
    }
}

/// Resolves an auth token of a superuser or of an auth collection record.
pub async fn authenticate(core: &Core, token: &str) -> RequestAuth {
    authenticate_as(core, token, TokenType::Auth).await
}

/// Resolves a token of the given type (`auth` or `file`) of a superuser or a record.
pub async fn authenticate_as(core: &Core, token: &str, kind: TokenType) -> RequestAuth {
    let Some(claims) = unverified_claims(token, kind) else {
        return RequestAuth::default();
    };
    let Some(collection_id) = claims.get("collectionId").and_then(Value::as_str) else {
        return RequestAuth::default();
    };

    if collection_id == SUPERUSERS_COLLECTION_ID {
        let Some(id) = claims.get("id").and_then(Value::as_str) else {
            return RequestAuth::default();
        };
        let superuser = find_superuser(core, "id = $1", id).await.filter(|s| {
            verify_signature(core, token, SUPERUSERS_COLLECTION_ID, kind, &s.token_key)
        });
        return RequestAuth {
            superuser,
            record: None,
        };
    }

    RequestAuth {
        superuser: None,
        record: verify_record_token(core, token, kind)
            .await
            .map(|(record, _)| record),
    }
}

/// Extracts the token from an `Authorization` header value (`Bearer` prefix optional).
pub fn token_from_header(raw: &str) -> Option<&str> {
    let token = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .unwrap_or(raw)
        .trim();
    (!token.is_empty()).then_some(token)
}

impl FromRequestParts<Arc<App>> for RequestAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        app: &Arc<App>,
    ) -> Result<Self, Self::Rejection> {
        let Some(token) = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(token_from_header)
        else {
            return Ok(Self::default());
        };
        let Ok(core) = app.core() else {
            return Ok(Self::default());
        };
        Ok(authenticate(&core, token).await)
    }
}
