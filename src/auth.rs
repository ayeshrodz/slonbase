//! Superuser authentication: password login and HS256 JWT auth tokens.
//!
//! Tokens are signed with the installation secret combined with the superuser's
//! `token_key`, so rotating a token key invalidates all of that user's tokens.

use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRequestParts, State};
use axum::http::header;
use axum::http::request::Parts;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::app::App;
use crate::collections::model::{SUPERUSERS_COLLECTION_ID, SUPERUSERS_COLLECTION_NAME};
use crate::core::Core;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::{datetime, superusers};

/// Superuser tokens are valid for 1 day (PocketBase's default).
pub const SUPERUSER_TOKEN_DURATION_SECS: i64 = 86_400;

type HmacSha256 = Hmac<Sha256>;

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
}

fn signing_key(core: &Core, token_key: &str) -> Vec<u8> {
    let mut key = core.auth_secret().to_vec();
    key.extend_from_slice(token_key.as_bytes());
    key
}

fn mac(key: &[u8], message: &str) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    mac
}

pub fn issue_token(core: &Core, superuser: &Superuser) -> String {
    let header = B64.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = json!({
        "id": superuser.id,
        "type": "auth",
        "collectionId": SUPERUSERS_COLLECTION_ID,
        "refreshable": true,
        "exp": chrono::Utc::now().timestamp() + SUPERUSER_TOKEN_DURATION_SECS,
    });
    let payload = B64.encode(claims.to_string());
    let message = format!("{header}.{payload}");
    let signature = mac(&signing_key(core, &superuser.token_key), &message)
        .finalize()
        .into_bytes();
    format!("{message}.{}", B64.encode(signature))
}

/// Returns the superuser a token belongs to, or `None` for any invalid token.
pub async fn authenticate(core: &Core, token: &str) -> Option<Superuser> {
    let mut parts = token.split('.');
    let (header, payload, signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }

    let header: Value = serde_json::from_slice(&B64.decode(header).ok()?).ok()?;
    if header.get("alg")?.as_str()? != "HS256" {
        return None;
    }
    let claims: Value = serde_json::from_slice(&B64.decode(payload).ok()?).ok()?;
    if claims.get("type")?.as_str()? != "auth"
        || claims.get("collectionId")?.as_str()? != SUPERUSERS_COLLECTION_ID
        || claims.get("exp")?.as_i64()? <= chrono::Utc::now().timestamp()
    {
        return None;
    }
    let id = claims.get("id")?.as_str()?;
    let signature = B64.decode(signature).ok()?;

    let superuser = find_superuser(core, "id = $1", id).await?;
    let signed_part = &token[..token.rfind('.')?];
    mac(&signing_key(core, &superuser.token_key), signed_part)
        .verify_slice(&signature)
        .ok()?;
    Some(superuser)
}

async fn find_superuser(core: &Core, condition: &str, value: &str) -> Option<Superuser> {
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

/// The authenticated caller. Invalid or expired tokens make the request a guest
/// request (like PocketBase), rather than failing it.
#[derive(Debug, Clone, Default)]
pub struct RequestAuth {
    pub superuser: Option<Superuser>,
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
}

impl FromRequestParts<Arc<App>> for RequestAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        app: &Arc<App>,
    ) -> Result<Self, Self::Rejection> {
        let Some(raw) = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
        else {
            return Ok(Self::default());
        };
        let token = raw
            .strip_prefix("Bearer ")
            .or_else(|| raw.strip_prefix("bearer "))
            .unwrap_or(raw)
            .trim();
        if token.is_empty() {
            return Ok(Self::default());
        }
        let Ok(core) = app.core() else {
            return Ok(Self::default());
        };
        Ok(Self {
            superuser: authenticate(&core, token).await,
        })
    }
}

fn auth_response(core: &Core, superuser: &Superuser) -> Json<Value> {
    Json(json!({
        "token": issue_token(core, superuser),
        "record": superuser.to_json(),
    }))
}

/// `POST /api/collections/_superusers/auth-with-password`
pub async fn auth_with_password(
    State(app): State<Arc<App>>,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let identity = body
        .get("identity")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let password = body
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let failed = || ApiError::bad_request("Failed to authenticate.");
    if identity.is_empty() || password.is_empty() {
        return Err(failed());
    }
    if !superusers::verify(&core.pool, identity, password)
        .await
        .map_err(ApiError::internal)?
    {
        return Err(failed());
    }
    let superuser = find_superuser(&core, "lower(email) = lower($1)", identity.trim())
        .await
        .ok_or_else(failed)?;
    Ok(auth_response(&core, &superuser))
}

/// `POST /api/collections/_superusers/auth-refresh`
pub async fn auth_refresh(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let superuser = auth.require_superuser()?;
    Ok(auth_response(&core, superuser))
}
