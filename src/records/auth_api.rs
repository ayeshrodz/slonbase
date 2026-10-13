//! Auth endpoints (`/api/collections/{collection}/auth-with-password` and friends),
//! compatible with PocketBase, for auth collections and for `_superusers`.
//!
//! Endpoints that start a flow by email (verification, password reset) always answer 204,
//! whether or not the email exists, and send the email in the background, so they can't
//! be used to find out which emails are registered.

use std::sync::{Arc, LazyLock};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use super::store::{self, ALIAS, record_json, visible_fields};
use super::values::{self, Values};
use crate::app::App;
use crate::auth::{AuthRecord, RequestAuth, TokenType, find_superuser, verify_record_token};
use crate::collections::model::{
    Collection, EmailTemplate, FIELD_EMAIL, FIELD_PASSWORD, FIELD_TOKEN_KEY, FIELD_VERIFIED,
    SUPERUSERS_COLLECTION_ID, SUPERUSERS_COLLECTION_NAME,
};
use crate::collections::schema::table_ref;
use crate::core::Core;
use crate::db::describe_pool_error;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::filter::{Params, RequestInfo, Scope, quote_ident, to_sql};
use crate::validation::Errors;
use crate::{mail, superusers};

/// A real bcrypt hash (default cost) to compare against when no record matched,
/// so that unknown identities take as long to reject as wrong passwords.
static DUMMY_HASH: LazyLock<String> =
    LazyLock::new(|| bcrypt::hash(crate::ids::random_secret(16), 10).expect("bcrypt hash"));

fn is_superusers(name: &str) -> bool {
    name == SUPERUSERS_COLLECTION_NAME || name == SUPERUSERS_COLLECTION_ID
}

fn auth_collection(core: &Core, name: &str) -> Result<Arc<Collection>, ApiError> {
    core.registry()
        .get(name)
        .filter(|c| c.is_auth())
        .cloned()
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "Missing or invalid auth collection context.",
            )
        })
}

fn pool_error(e: deadpool_postgres::PoolError) -> ApiError {
    ApiError::internal(describe_pool_error(e))
}

fn db_error(e: tokio_postgres::Error) -> ApiError {
    ApiError::internal(e.into())
}

fn body_str<'a>(body: &'a Map<String, Value>, key: &str) -> &'a str {
    body.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// A record as returned to itself: visible fields, including its email.
fn self_json(c: &Collection, values: &Values) -> Value {
    let fields = visible_fields(c, false);
    let mut out = Values::new();
    for f in fields {
        if let Some(v) = values.get(&f.name) {
            out.insert(f.name.clone(), v.clone());
        }
    }
    record_json(c, out)
}

fn auth_response(
    core: &Core,
    record: &AuthRecord,
    refreshable: bool,
    duration: Option<i64>,
) -> Json<Value> {
    Json(json!({
        "token": record.issue_auth_token(core, duration, refreshable),
        "record": self_json(&record.collection, &record.values),
    }))
}

/// Whether the collection's `authRule` lets this record sign in.
async fn auth_rule_allows(
    core: &Core,
    record: &AuthRecord,
    method: &Method,
) -> Result<bool, ApiError> {
    let c = &record.collection;
    let rule = match c.auth_options().and_then(|a| a.auth_rule.as_deref()) {
        None => return Ok(false),
        Some(rule) if rule.trim().is_empty() => return Ok(true),
        Some(rule) => rule,
    };
    let mut request = RequestInfo::new(method.as_str());
    request.auth = Some(record.info());
    request.registry = Some(core.registry());
    let scope = Scope {
        collection: c,
        alias: ALIAS,
        request: &request,
        allow_hidden: true,
    };
    let mut params = Params::default();
    let id_param = params.push(record.id());
    let condition = to_sql(rule, &scope, &mut params)
        .map_err(|e| ApiError::internal(anyhow::anyhow!("invalid stored auth rule: {e}")))?;
    let client = core.pool.get().await.map_err(pool_error)?;
    let row = client
        .query_opt(
            &format!(
                "SELECT 1 FROM {} {ALIAS} WHERE {ALIAS}.\"id\" = {id_param} AND {condition}",
                table_ref(&c.name)
            ),
            &params.as_refs(),
        )
        .await
        .map_err(db_error)?;
    Ok(row.is_some())
}

async fn find_by_email(
    core: &Core,
    c: &Collection,
    email: &str,
) -> Result<Option<AuthRecord>, ApiError> {
    let client = core.pool.get().await.map_err(pool_error)?;
    let condition = format!(
        "lower({ALIAS}.{}) = lower($1::text)",
        quote_ident(FIELD_EMAIL)
    );
    let values = store::find_by(&client, c, &condition, email.trim())
        .await
        .map_err(db_error)?;
    Ok(values.map(|values| AuthRecord {
        collection: Arc::new(c.clone()),
        values,
        refreshable: true,
    }))
}

fn send_template(core: &Core, template: &EmailTemplate, to: &str, token: &str) {
    let settings = core.settings();
    core.send_mail(mail::Message {
        to: to.to_string(),
        subject: mail::render(&template.subject, &settings, token),
        html: mail::render(&template.body, &settings, token),
    });
}

fn new_token_key() -> String {
    crate::ids::generate_from_pattern("[a-zA-Z0-9]{50}").expect("valid pattern")
}

// ---------------------------------------------------------------------------
// sign in

/// `POST /api/collections/{collection}/auth-with-password`
pub async fn auth_with_password(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    method: Method,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let identity = body_str(&body, "identity").trim().to_string();
    let password = body_str(&body, "password").to_string();
    let failed = || ApiError::bad_request("Failed to authenticate.");
    if identity.is_empty() || password.is_empty() {
        return Err(failed());
    }

    if is_superusers(&collection) {
        if !superusers::verify(&core.pool, &identity, &password)
            .await
            .map_err(ApiError::internal)?
        {
            return Err(failed());
        }
        let superuser = find_superuser(&core, "lower(email) = lower($1)", &identity)
            .await
            .ok_or_else(failed)?;
        return Ok(Json(json!({
            "token": superuser.issue_token(&core),
            "record": superuser.to_json(),
        })));
    }

    let c = auth_collection(&core, &collection)?;
    let options = c.auth_options().expect("auth collection");
    if !options.password_auth.enabled {
        return Err(ApiError::forbidden(
            "The collection is not configured to allow password authentication.",
        ));
    }
    let requested_field = body_str(&body, "identityField");
    let fields: Vec<&String> = if requested_field.is_empty() {
        options.password_auth.identity_fields.iter().collect()
    } else {
        match options
            .password_auth
            .identity_fields
            .iter()
            .find(|f| f.eq_ignore_ascii_case(requested_field))
        {
            Some(f) => vec![f],
            None => return Err(ApiError::bad_request("Invalid identity field.")),
        }
    };

    let client = core.pool.get().await.map_err(pool_error)?;
    let mut found = None;
    for name in fields {
        let Some(field) = c.field(name) else { continue };
        let column = format!("{ALIAS}.{}", quote_ident(&field.name));
        let condition = if field.name == FIELD_EMAIL {
            format!("lower({column}) = lower($1::text)")
        } else {
            format!("{column} = $1::text")
        };
        if let Some(values) = store::find_by(&client, &c, &condition, &identity)
            .await
            .map_err(db_error)?
        {
            found = Some(values);
            break;
        }
    }
    drop(client);

    // compare against something even when nothing matched, so timing doesn't reveal it
    let hash = found
        .as_ref()
        .and_then(|v| v.get(FIELD_PASSWORD))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| DUMMY_HASH.clone());
    let valid = values::verify_password(password, hash).await;
    let Some(values) = found.filter(|_| valid) else {
        return Err(failed());
    };

    let record = AuthRecord {
        collection: c,
        values,
        refreshable: true,
    };
    if !auth_rule_allows(&core, &record, &method).await? {
        return Err(failed());
    }
    Ok(auth_response(&core, &record, true, None))
}

/// `POST /api/collections/{collection}/auth-refresh`
pub async fn auth_refresh(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    method: Method,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    if is_superusers(&collection) {
        let superuser = auth.require_superuser()?;
        return Ok(Json(json!({
            "token": superuser.issue_token(&core),
            "record": superuser.to_json(),
        })));
    }

    let c = auth_collection(&core, &collection)?;
    let record = auth
        .record
        .filter(|r| r.collection.id == c.id)
        .ok_or_else(|| {
            ApiError::unauthorized("The request requires valid record authorization token.")
        })?;
    if !record.refreshable {
        return Err(ApiError::forbidden(
            "The current auth token is not refreshable.",
        ));
    }
    if !auth_rule_allows(&core, &record, &method).await? {
        return Err(ApiError::forbidden(
            "The authenticated record is not allowed to sign in.",
        ));
    }
    Ok(auth_response(&core, &record, true, None))
}

/// `GET /api/collections/{collection}/auth-methods`
pub async fn auth_methods(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let (enabled, identity_fields) = if is_superusers(&collection) {
        (true, vec!["email".to_string()])
    } else {
        let c = auth_collection(&core, &collection)?;
        let o = c.auth_options().expect("auth collection");
        (
            o.password_auth.enabled,
            o.password_auth.identity_fields.clone(),
        )
    };
    Ok(Json(json!({
        "password": { "enabled": enabled, "identityFields": identity_fields },
        "oauth2": { "enabled": false, "providers": [] },
        "mfa": { "enabled": false, "duration": 0 },
        "otp": { "enabled": false, "duration": 0 },
    })))
}

/// `POST /api/collections/{collection}/impersonate/{id}` (superusers): a non-refreshable
/// auth token for any record, optionally with a custom `duration` in seconds.
pub async fn impersonate(
    State(app): State<Arc<App>>,
    Path((collection, id)): Path<(String, String)>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let c = auth_collection(&core, &collection)?;
    let duration = match body.get("duration").and_then(Value::as_i64) {
        None | Some(0) => None,
        Some(d) if (1..=365 * 86_400).contains(&d) => Some(d),
        Some(_) => return Err(ApiError::bad_request("Invalid duration.")),
    };
    let client = core.pool.get().await.map_err(pool_error)?;
    let values = store::find_by_id(&client, &c, &id)
        .await
        .map_err(db_error)?
        .ok_or_else(ApiError::not_found)?;
    let record = AuthRecord {
        collection: c,
        values,
        refreshable: false,
    };
    Ok(auth_response(&core, &record, false, duration))
}

// ---------------------------------------------------------------------------
// email verification

/// `POST /api/collections/{collection}/request-verification`
pub async fn request_verification(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let email = body_str(&body, "email");
    if email.trim().is_empty() {
        let mut errors = Errors::new();
        errors.add("email", "validation_required", "Cannot be blank.");
        return Err(ApiError::invalid_data(
            "An error occurred while validating the submitted data.",
            &errors,
        ));
    }

    if let Some(record) = find_by_email(&core, &c, email).await? {
        let verified = record
            .values
            .get(FIELD_VERIFIED)
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !verified {
            let options = c.auth_options().expect("auth collection");
            let mut claims = Map::new();
            claims.insert("email".into(), Value::String(record.email().to_string()));
            let token = record.issue(
                &core,
                TokenType::Verification,
                claims,
                options.verification_token.duration,
            );
            send_template(
                &core,
                &options.verification_template,
                record.email(),
                &token,
            );
        }
    }
    Ok(no_content())
}

/// `POST /api/collections/{collection}/confirm-verification`
pub async fn confirm_verification(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let invalid = || ApiError::bad_request("Invalid or expired verification token.");

    let (record, claims) =
        verify_record_token(&core, body_str(&body, "token"), TokenType::Verification)
            .await
            .filter(|(r, _)| r.collection.id == c.id)
            .ok_or_else(invalid)?;
    let claimed = claims
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !claimed.eq_ignore_ascii_case(record.email()) {
        return Err(invalid());
    }

    let client = core.pool.get().await.map_err(pool_error)?;
    client
        .execute(
            &format!(
                "UPDATE {} SET {} = TRUE WHERE id = $1::text",
                table_ref(&c.name),
                quote_ident(FIELD_VERIFIED)
            ),
            &[&record.id()],
        )
        .await
        .map_err(db_error)?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------
// password reset

/// `POST /api/collections/{collection}/request-password-reset`
pub async fn request_password_reset(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let email = body_str(&body, "email");
    if email.trim().is_empty() {
        let mut errors = Errors::new();
        errors.add("email", "validation_required", "Cannot be blank.");
        return Err(ApiError::invalid_data(
            "An error occurred while validating the submitted data.",
            &errors,
        ));
    }

    if let Some(record) = find_by_email(&core, &c, email).await? {
        let options = c.auth_options().expect("auth collection");
        let mut claims = Map::new();
        claims.insert("email".into(), Value::String(record.email().to_string()));
        let token = record.issue(
            &core,
            TokenType::PasswordReset,
            claims,
            options.password_reset_token.duration,
        );
        send_template(
            &core,
            &options.reset_password_template,
            record.email(),
            &token,
        );
    }
    Ok(no_content())
}

/// `POST /api/collections/{collection}/confirm-password-reset`
pub async fn confirm_password_reset(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let password = body_str(&body, "password").to_string();

    let mut errors = Errors::new();
    let field = c
        .field(FIELD_PASSWORD)
        .expect("auth collections have a password field");
    if password.is_empty() {
        errors.add("password", "validation_required", "Cannot be blank.");
    } else if let Err(message) = values::check_password(field, &password) {
        errors.invalid("password", message);
    }
    if body_str(&body, "passwordConfirm") != password {
        errors.invalid("passwordConfirm", "Values don't match.");
    }
    let token_result =
        verify_record_token(&core, body_str(&body, "token"), TokenType::PasswordReset)
            .await
            .filter(|(r, _)| r.collection.id == c.id)
            .filter(|(r, claims)| {
                claims
                    .get("email")
                    .and_then(Value::as_str)
                    .is_some_and(|e| e.eq_ignore_ascii_case(r.email()))
            });
    if token_result.is_none() {
        errors.invalid("token", "Invalid or expired password reset token.");
    }
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(
            "Failed to reset the password.",
            &errors,
        ));
    }
    let (record, _) = token_result.expect("checked above");

    let hash = values::hash_password(field, password)
        .await
        .map_err(ApiError::internal)?;
    let client = core.pool.get().await.map_err(pool_error)?;
    // a new token key signs out every session and makes the reset token single-use;
    // receiving the email also proves ownership of the address
    client
        .execute(
            &format!(
                "UPDATE {} SET {} = $2::text, {} = $3::text, {} = TRUE WHERE id = $1::text",
                table_ref(&c.name),
                quote_ident(FIELD_PASSWORD),
                quote_ident(FIELD_TOKEN_KEY),
                quote_ident(FIELD_VERIFIED)
            ),
            &[&record.id(), &hash, &new_token_key()],
        )
        .await
        .map_err(db_error)?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------
// email change

/// `POST /api/collections/{collection}/request-email-change` (the record itself)
pub async fn request_email_change(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let record = auth
        .record
        .filter(|r| r.collection.id == c.id)
        .ok_or_else(|| {
            ApiError::unauthorized("The request requires valid record authorization token.")
        })?;

    let new_email = body_str(&body, "newEmail").trim().to_string();
    let mut errors = Errors::new();
    let field = c
        .field(FIELD_EMAIL)
        .expect("auth collections have an email field");
    if new_email.is_empty() {
        errors.add("newEmail", "validation_required", "Cannot be blank.");
    } else if let Err(message) = values::check_value(field, &Value::String(new_email.clone())) {
        errors.invalid("newEmail", message);
    } else if find_by_email(&core, &c, &new_email).await?.is_some() {
        errors.invalid("newEmail", "Invalid or already in use email address.");
    }
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(
            "Failed to request the email change.",
            &errors,
        ));
    }

    let options = c.auth_options().expect("auth collection");
    let mut claims = Map::new();
    claims.insert("email".into(), Value::String(record.email().to_string()));
    claims.insert("newEmail".into(), Value::String(new_email.clone()));
    let token = record.issue(
        &core,
        TokenType::EmailChange,
        claims,
        options.email_change_token.duration,
    );
    send_template(
        &core,
        &options.confirm_email_change_template,
        &new_email,
        &token,
    );
    Ok(no_content())
}

/// `POST /api/collections/{collection}/confirm-email-change`
pub async fn confirm_email_change(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let mut errors = Errors::new();

    let token_result = verify_record_token(&core, body_str(&body, "token"), TokenType::EmailChange)
        .await
        .filter(|(r, _)| r.collection.id == c.id)
        .filter(|(r, claims)| {
            claims
                .get("email")
                .and_then(Value::as_str)
                .is_some_and(|e| e.eq_ignore_ascii_case(r.email()))
        });
    let Some((record, claims)) = token_result else {
        errors.invalid("token", "Invalid or expired email change token.");
        return Err(ApiError::invalid_data(
            "Failed to confirm the email change.",
            &errors,
        ));
    };
    let hash = record.str(FIELD_PASSWORD).to_string();
    if !values::verify_password(body_str(&body, "password").to_string(), hash).await {
        errors.invalid("password", "Invalid account password.");
        return Err(ApiError::invalid_data(
            "Failed to confirm the email change.",
            &errors,
        ));
    }
    let new_email = claims
        .get("newEmail")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let client = core.pool.get().await.map_err(pool_error)?;
    let result = client
        .execute(
            &format!(
                "UPDATE {} SET {} = $2::text, {} = $3::text, {} = TRUE WHERE id = $1::text",
                table_ref(&c.name),
                quote_ident(FIELD_EMAIL),
                quote_ident(FIELD_TOKEN_KEY),
                quote_ident(FIELD_VERIFIED)
            ),
            &[&record.id(), &new_email, &new_token_key()],
        )
        .await;
    if let Err(e) = result {
        if e.as_db_error().map(|d| d.code())
            == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION)
        {
            errors.invalid("token", "The new email address is already in use.");
            return Err(ApiError::invalid_data(
                "Failed to confirm the email change.",
                &errors,
            ));
        }
        return Err(db_error(e));
    }
    Ok(no_content())
}
