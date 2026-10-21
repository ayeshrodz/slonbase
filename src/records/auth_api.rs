//! Auth endpoints (`/api/collections/{collection}/auth-with-password` and friends),
//! compatible with PocketBase, for auth collections and for `_superusers`.
//!
//! Endpoints that start a flow by email (verification, password reset) always answer 204,
//! whether or not the email exists, and send the email in the background, so they can't
//! be used to find out which emails are registered.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::auth_hooks::{self, Ctx};
use super::store::{self, ALIAS, record_json, visible_fields};
use super::values::{self, Values};
use crate::app::App;
use crate::auth::{AuthRecord, TokenType, verify_record_token};
use crate::collections::model::{
    Collection, EmailTemplate, FIELD_EMAIL, FIELD_PASSWORD, FIELD_TOKEN_KEY, FIELD_VERIFIED,
};
use crate::collections::schema::table_ref;
use crate::core::Core;
use crate::db::TextQuery;
use crate::db::describe_pool_error;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::filter::{Params, RequestInfo, Scope, quote_ident, to_sql};
use crate::hooks::AuthRequestEvent;
use crate::ip::Client;
use crate::mail;
use crate::validation::Errors;

/// Real bcrypt hashes (by cost) to compare against when no record matched, so that
/// unknown identities take as long to reject as wrong passwords.
static DUMMY_HASHES: LazyLock<std::sync::Mutex<HashMap<u32, String>>> =
    LazyLock::new(Default::default);

fn dummy_hash(c: &Collection) -> String {
    let cost = match c.field(FIELD_PASSWORD).map(|f| &f.kind) {
        Some(crate::collections::model::FieldKind::Password(o)) if o.cost != 0 => o.cost,
        _ => 10,
    };
    DUMMY_HASHES
        .lock()
        .expect("dummy hashes lock")
        .entry(cost)
        .or_insert_with(|| bcrypt::hash(crate::ids::random_secret(16), cost).expect("bcrypt hash"))
        .clone()
}

pub(super) fn auth_collection(core: &Core, name: &str) -> Result<Arc<Collection>, ApiError> {
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

pub(super) fn pool_error(e: deadpool_postgres::PoolError) -> ApiError {
    ApiError::internal(describe_pool_error(e))
}

pub(super) fn db_error(e: tokio_postgres::Error) -> ApiError {
    ApiError::internal(e.into())
}

pub(super) fn body_str<'a>(body: &'a Map<String, Value>, key: &str) -> &'a str {
    body.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// A record as returned to itself: visible fields, including its email.
pub(super) fn self_json(c: &Collection, values: &Values) -> Value {
    let fields = visible_fields(c, false);
    let mut out = Values::new();
    for f in fields {
        if let Some(v) = values.get(&f.name) {
            out.insert(f.name.clone(), v.clone());
        }
    }
    record_json(c, out)
}

/// Whether the collection's `authRule` lets this record sign in.
pub(super) async fn auth_rule_allows(
    core: &Core,
    record: &AuthRecord,
    method: &Method,
) -> Result<bool, ApiError> {
    let rule = record
        .collection
        .auth_options()
        .and_then(|a| a.auth_rule.clone());
    record_matches(core, record, rule.as_deref(), method).await
}

/// Whether a record matches a rule (`None`: no record does, `""`: every record does).
async fn record_matches(
    core: &Core,
    record: &AuthRecord,
    rule: Option<&str>,
    method: &Method,
) -> Result<bool, ApiError> {
    let c = &record.collection;
    let rule = match rule {
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
        .map_err(|e| ApiError::internal(anyhow::anyhow!("invalid stored rule: {e}")))?;
    let client = core.pool.get().await.map_err(pool_error)?;
    let row = client
        .query_text_opt(
            &format!(
                "SELECT 1 FROM {} {ALIAS} WHERE {ALIAS}.\"id\" = {id_param} AND {condition}",
                table_ref(&c.name)
            ),
            &params,
        )
        .await
        .map_err(db_error)?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// multi-factor authentication

pub(super) const METHOD_PASSWORD: &str = "password";
pub(super) const METHOD_OTP: &str = "otp";
pub(super) const METHOD_OAUTH2: &str = "oauth2";

/// The MFA step of a sign-in made with `method`: when the collection requires MFA for the
/// record, the first sign-in answers `401 {"mfaId"}` (returned here), and the sign-in with
/// another method that sends that `mfaId` passes (`None`).
pub(super) async fn check_mfa(
    core: &Core,
    record: &AuthRecord,
    method: &str,
    http_method: &Method,
    mfa_id: &str,
) -> Result<Option<Response>, ApiError> {
    let c = &record.collection;
    let mfa = &c.auth_options().expect("auth collection").mfa;
    if !mfa.enabled || !record_matches(core, record, Some(&mfa.rule), http_method).await? {
        return Ok(None);
    }
    let client = core.pool.get().await.map_err(pool_error)?;
    if mfa_id.is_empty() {
        let id = crate::ids::random_id(crate::ids::DEFAULT_ID_LENGTH);
        client
            .execute(
                "INSERT INTO _slonbase.mfas (id, collection_id, record_id, method) VALUES ($1, $2, $3, $4)",
                &[&id, &c.id, &record.id(), &method],
            )
            .await
            .map_err(db_error)?;
        return Ok(Some(
            (StatusCode::UNAUTHORIZED, Json(json!({ "mfaId": id }))).into_response(),
        ));
    }

    let row = client
        .query_opt(
            "SELECT collection_id, record_id, method FROM _slonbase.mfas
             WHERE id = $1 AND created > now() - $2::bigint * interval '1 second'",
            &[&mfa_id, &mfa.duration],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(|| ApiError::bad_request("Invalid or expired MFA session."))?;
    let (collection_id, record_id, first): (String, String, String) =
        (row.get(0), row.get(1), row.get(2));
    if collection_id != c.id || record_id != record.id() {
        return Err(ApiError::bad_request("Invalid MFA session."));
    }
    if first == method {
        return Err(ApiError::bad_request(
            "A different authentication method is required.",
        ));
    }
    // single use, also when two requests race
    if client
        .execute("DELETE FROM _slonbase.mfas WHERE id = $1", &[&mfa_id])
        .await
        .map_err(db_error)?
        == 0
    {
        return Err(ApiError::bad_request("Invalid or expired MFA session."));
    }
    Ok(None)
}

/// Known devices kept per record.
const MAX_ORIGINS: i64 = 5;

/// Remembers the device a record signed in from, and emails the record when it's a new
/// one (but not on its first sign-in, or the first since a password change).
pub(super) async fn auth_alert(
    core: &Core,
    record: &AuthRecord,
    client: &Client,
) -> anyhow::Result<()> {
    let c = &record.collection;
    let settings = core.settings();
    let ip = client
        .ip(&settings.trusted_proxy)
        .map(|ip| ip.to_string())
        .unwrap_or_default();
    let user_agent = client.user_agent();
    let fingerprint = hex(&Sha256::digest(format!("{ip}\n{user_agent}")));
    let token_key = record
        .values
        .get(FIELD_TOKEN_KEY)
        .and_then(Value::as_str)
        .unwrap_or_default();
    let key_hash = hex(&Sha256::digest(token_key));

    let db = core.pool.get().await?;
    // devices from before the last password change don't count
    db.execute(
        "DELETE FROM _slonbase.auth_origins
         WHERE collection_id = $1 AND record_id = $2 AND key_hash <> $3",
        &[&c.id, &record.id(), &key_hash],
    )
    .await?;
    let known: i64 = db
        .query_one(
            "SELECT count(*) FROM _slonbase.auth_origins WHERE collection_id = $1 AND record_id = $2",
            &[&c.id, &record.id()],
        )
        .await?
        .get(0);
    // (xmax = 0) is true for a row this statement inserted, false for an updated one
    let is_new: bool = db
        .query_one(
            "INSERT INTO _slonbase.auth_origins (collection_id, record_id, fingerprint, key_hash)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (collection_id, record_id, fingerprint) DO UPDATE SET updated = now()
             RETURNING (xmax = 0)",
            &[&c.id, &record.id(), &fingerprint, &key_hash],
        )
        .await?
        .get(0);
    db.execute(
        "DELETE FROM _slonbase.auth_origins WHERE collection_id = $1 AND record_id = $2
         AND fingerprint NOT IN (SELECT fingerprint FROM _slonbase.auth_origins
             WHERE collection_id = $1 AND record_id = $2 ORDER BY updated DESC LIMIT $3)",
        &[&c.id, &record.id(), &MAX_ORIGINS],
    )
    .await?;
    drop(db);

    let options = &c.auth_options().expect("auth collection").auth_alert;
    let email = record
        .values
        .get(FIELD_EMAIL)
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !options.enabled || !is_new || known == 0 || email.is_empty() {
        return Ok(());
    }
    let info = alert_info(&ip, user_agent);
    let fill = |text: &str| mail::render(&text.replace("{ALERT_INFO}", &info), &settings, "");
    core.send_mail_for(
        mail::Message {
            to: email.to_string(),
            subject: fill(&options.email_template.subject),
            html: fill(&options.email_template.body),
        },
        "authAlert",
        Some(record),
    );
    Ok(())
}

/// `{ALERT_INFO}`: when and where the sign-in came from (HTML-escaped).
pub(crate) fn alert_info(ip: &str, user_agent: &str) -> String {
    let when = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let mut info = when.to_string();
    for part in [ip, user_agent] {
        if !part.is_empty() {
            info.push_str(" - ");
            info.push_str(part);
        }
    }
    mail::html_escape(&info)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) async fn find_by_email(
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

pub(super) fn send_template(
    core: &Core,
    template: &EmailTemplate,
    to: &str,
    token: &str,
    kind: &str,
    record: &AuthRecord,
) {
    let settings = core.settings();
    core.send_mail_for(
        mail::Message {
            to: to.to_string(),
            subject: mail::render(&template.subject, &settings, token),
            html: mail::render(&template.body, &settings, token),
        },
        kind,
        Some(record),
    );
}

pub(super) fn new_token_key() -> String {
    crate::ids::generate_from_pattern("[a-zA-Z0-9]{50}").expect("valid pattern")
}

// ---------------------------------------------------------------------------
// sign in

/// `POST /api/collections/{collection}/auth-with-password`
pub async fn auth_with_password(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    ctx: Ctx,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let identity = body_str(&body, "identity").trim().to_string();
    let password = body_str(&body, "password").to_string();
    let failed = || ApiError::bad_request("Failed to authenticate.");
    if identity.is_empty() || password.is_empty() {
        return Err(failed());
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

    let ctx = ctx.with_body(&body);
    let record = found.map(|values| AuthRecord {
        collection: c.clone(),
        values,
        refreshable: true,
    });
    let e = ctx.event(
        &core,
        &c,
        record,
        auth_hooks::data([
            ("identity", Value::String(identity)),
            ("identityField", Value::String(requested_field.to_string())),
            ("password", Value::String(password)),
        ]),
    );
    auth_hooks::run(
        |h| &h.on_record_auth_with_password_request,
        e,
        move |e| async move {
            // compare against something even when nothing matched, so timing doesn't
            // reveal it
            let hash = e
                .record
                .as_ref()
                .map(|r| r.str(FIELD_PASSWORD).to_string())
                .unwrap_or_else(|| dummy_hash(&e.collection));
            let valid = values::verify_password(e.str("password").to_string(), hash).await;
            let failed = || ApiError::bad_request("Failed to authenticate.");
            let Some(record) = e.record.clone().filter(|_| valid) else {
                return Err(failed());
            };
            if !auth_rule_allows(&e.core, &record, &ctx.method).await? {
                return Err(failed());
            }
            let token = record.issue_auth_token(&e.core, None, true);
            let response =
                auth_hooks::respond(&e.core, record, token, METHOD_PASSWORD, None, ctx).await?;
            Ok(with_response(e, response))
        },
    )
    .await
}

/// Sets the event's response (for actions that end with another hooked response).
pub(super) fn with_response(mut e: AuthRequestEvent, response: Response) -> AuthRequestEvent {
    e.response = Some(response);
    e
}

/// `POST /api/collections/{collection}/auth-refresh`
pub async fn auth_refresh(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    ctx: Ctx,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let record = ctx
        .auth
        .any_record()
        .cloned()
        .filter(|r| r.collection.id == c.id)
        .ok_or_else(|| {
            ApiError::unauthorized("The request requires valid record authorization token.")
        })?;
    if !record.refreshable {
        return Err(ApiError::forbidden(
            "The current auth token is not refreshable.",
        ));
    }

    let e = ctx.event(&core, &c, Some(record), Map::new());
    auth_hooks::run(
        |h| &h.on_record_auth_refresh_request,
        e,
        move |e| async move {
            let record = auth_hooks::record_of(&e)?;
            if !auth_rule_allows(&e.core, &record, &ctx.method).await? {
                return Err(ApiError::forbidden(
                    "The authenticated record is not allowed to sign in.",
                ));
            }
            let token = record.issue_auth_token(&e.core, None, true);
            let response = auth_hooks::respond(&e.core, record, token, "", None, ctx).await?;
            Ok(with_response(e, response))
        },
    )
    .await
}

/// `GET /api/collections/{collection}/auth-methods`
pub async fn auth_methods(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let o = c.auth_options().expect("auth collection");
    let providers: Vec<Value> = if o.oauth2.enabled {
        o.oauth2
            .providers
            .iter()
            .filter_map(crate::oauth2::Provider::new)
            .map(|p| p.auth_method())
            .collect()
    } else {
        vec![]
    };
    let duration = |enabled: bool, duration: i64| if enabled { duration } else { 0 };
    Ok(Json(json!({
        "password": {
            "enabled": o.password_auth.enabled,
            "identityFields": o.password_auth.identity_fields,
        },
        "oauth2": { "enabled": o.oauth2.enabled, "providers": providers },
        "mfa": { "enabled": o.mfa.enabled, "duration": duration(o.mfa.enabled, o.mfa.duration) },
        "otp": { "enabled": o.otp.enabled, "duration": duration(o.otp.enabled, o.otp.duration) },
    })))
}

/// `POST /api/collections/{collection}/impersonate/{id}` (superusers): a non-refreshable
/// auth token for any record, optionally with a custom `duration` in seconds.
pub async fn impersonate(
    State(app): State<Arc<App>>,
    Path((collection, id)): Path<(String, String)>,
    ctx: Ctx,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    ctx.auth.require_superuser()?;
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
    let token = record.issue_auth_token(&core, duration, false);
    let ctx = ctx.with_body(&body);
    auth_hooks::respond(&core, record, token, "", None, ctx).await
}

// ---------------------------------------------------------------------------
// email verification

/// `POST /api/collections/{collection}/request-verification`
pub async fn request_verification(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    ctx: Ctx,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let email = body_str(&body, "email");
    if email.trim().is_empty() {
        return Err(blank("email"));
    }
    let Some(record) = find_by_email(&core, &c, email).await? else {
        return Ok(no_content());
    };
    let ctx = ctx.with_body(&body);
    let e = ctx.event(&core, &c, Some(record), Map::new());
    auth_hooks::run(
        |h| &h.on_record_request_verification_request,
        e,
        |mut e| async move {
            let record = auth_hooks::record_of(&e)?;
            let verified = record
                .values
                .get(FIELD_VERIFIED)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !verified {
                let options = e.collection.auth_options().expect("auth collection");
                let token = record.issue(
                    &e.core,
                    TokenType::Verification,
                    email_claims(&record, None),
                    options.verification_token.duration,
                );
                send_template(
                    &e.core,
                    &options.verification_template,
                    record.email(),
                    &token,
                    "verification",
                    &record,
                );
            }
            e.response = Some(no_content());
            Ok(e)
        },
    )
    .await
}

/// `{"email"}` (and `"newEmail"`) claims of a token sent by email.
fn email_claims(record: &AuthRecord, new_email: Option<&str>) -> Map<String, Value> {
    let mut claims = Map::new();
    claims.insert("email".into(), Value::String(record.email().to_string()));
    if let Some(new_email) = new_email {
        claims.insert("newEmail".into(), Value::String(new_email.to_string()));
    }
    claims
}

fn blank(key: &str) -> ApiError {
    let mut errors = Errors::new();
    errors.add(key, "validation_required", "Cannot be blank.");
    ApiError::invalid_data(
        "An error occurred while validating the submitted data.",
        &errors,
    )
}

/// `POST /api/collections/{collection}/confirm-verification`
pub async fn confirm_verification(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    ctx: Ctx,
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

    let ctx = ctx.with_body(&body);
    let e = ctx.event(&core, &c, Some(record), Map::new());
    auth_hooks::run(
        |h| &h.on_record_confirm_verification_request,
        e,
        |mut e| async move {
            let record = auth_hooks::record_of(&e)?;
            let client = e.core.pool.get().await.map_err(pool_error)?;
            client
                .execute(
                    &format!(
                        "UPDATE {} SET {} = TRUE WHERE id = $1::text",
                        table_ref(&e.collection.name),
                        quote_ident(FIELD_VERIFIED)
                    ),
                    &[&record.id()],
                )
                .await
                .map_err(db_error)?;
            e.response = Some(no_content());
            Ok(e)
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// password reset

/// `POST /api/collections/{collection}/request-password-reset`
pub async fn request_password_reset(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    ctx: Ctx,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let email = body_str(&body, "email");
    if email.trim().is_empty() {
        return Err(blank("email"));
    }
    let Some(record) = find_by_email(&core, &c, email).await? else {
        return Ok(no_content());
    };
    let ctx = ctx.with_body(&body);
    let e = ctx.event(&core, &c, Some(record), Map::new());
    auth_hooks::run(
        |h| &h.on_record_request_password_reset_request,
        e,
        |mut e| async move {
            let record = auth_hooks::record_of(&e)?;
            let options = e.collection.auth_options().expect("auth collection");
            let token = record.issue(
                &e.core,
                TokenType::PasswordReset,
                email_claims(&record, None),
                options.password_reset_token.duration,
            );
            send_template(
                &e.core,
                &options.reset_password_template,
                record.email(),
                &token,
                "passwordReset",
                &record,
            );
            e.response = Some(no_content());
            Ok(e)
        },
    )
    .await
}

/// `POST /api/collections/{collection}/confirm-password-reset`
pub async fn confirm_password_reset(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    ctx: Ctx,
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

    let ctx = ctx.with_body(&body);
    let e = ctx.event(&core, &c, Some(record), Map::new());
    auth_hooks::run(
        |h| &h.on_record_confirm_password_reset_request,
        e,
        move |mut e| async move {
            let record = auth_hooks::record_of(&e)?;
            let field = e
                .collection
                .field(FIELD_PASSWORD)
                .expect("auth collections have a password field");
            let hash = values::hash_password(field, password)
                .await
                .map_err(ApiError::internal)?;
            let client = e.core.pool.get().await.map_err(pool_error)?;
            // a new token key signs out every session and makes the reset token
            // single-use; receiving the email also proves ownership of the address
            client
                .execute(
                    &format!(
                        "UPDATE {} SET {} = $2::text, {} = $3::text, {} = TRUE WHERE id = $1::text",
                        table_ref(&e.collection.name),
                        quote_ident(FIELD_PASSWORD),
                        quote_ident(FIELD_TOKEN_KEY),
                        quote_ident(FIELD_VERIFIED)
                    ),
                    &[&record.id(), &hash, &new_token_key()],
                )
                .await
                .map_err(db_error)?;
            e.response = Some(no_content());
            Ok(e)
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// email change

/// `POST /api/collections/{collection}/request-email-change` (the record itself)
pub async fn request_email_change(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    ctx: Ctx,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let record = ctx
        .auth
        .any_record()
        .cloned()
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

    let ctx = ctx.with_body(&body);
    let e = ctx.event(
        &core,
        &c,
        Some(record),
        auth_hooks::data([("newEmail", Value::String(new_email))]),
    );
    auth_hooks::run(
        |h| &h.on_record_request_email_change_request,
        e,
        |mut e| async move {
            let record = auth_hooks::record_of(&e)?;
            let new_email = e.str("newEmail").to_string();
            let options = e.collection.auth_options().expect("auth collection");
            let token = record.issue(
                &e.core,
                TokenType::EmailChange,
                email_claims(&record, Some(&new_email)),
                options.email_change_token.duration,
            );
            send_template(
                &e.core,
                &options.confirm_email_change_template,
                &new_email,
                &token,
                "emailChange",
                &record,
            );
            e.response = Some(no_content());
            Ok(e)
        },
    )
    .await
}

/// `POST /api/collections/{collection}/confirm-email-change`
pub async fn confirm_email_change(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    ctx: Ctx,
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

    let ctx = ctx.with_body(&body);
    let e = ctx.event(
        &core,
        &c,
        Some(record),
        auth_hooks::data([("newEmail", Value::String(new_email))]),
    );
    auth_hooks::run(
        |h| &h.on_record_confirm_email_change_request,
        e,
        |mut e| async move {
            let record = auth_hooks::record_of(&e)?;
            let client = e.core.pool.get().await.map_err(pool_error)?;
            let result = client
                .execute(
                    &format!(
                        "UPDATE {} SET {} = $2::text, {} = $3::text, {} = TRUE WHERE id = $1::text",
                        table_ref(&e.collection.name),
                        quote_ident(FIELD_EMAIL),
                        quote_ident(FIELD_TOKEN_KEY),
                        quote_ident(FIELD_VERIFIED)
                    ),
                    &[&record.id(), &e.str("newEmail"), &new_token_key()],
                )
                .await;
            if let Err(err) = result {
                if err.as_db_error().map(|d| d.code())
                    == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION)
                {
                    let mut errors = Errors::new();
                    errors.invalid("token", "The new email address is already in use.");
                    return Err(ApiError::invalid_data(
                        "Failed to confirm the email change.",
                        &errors,
                    ));
                }
                return Err(db_error(err));
            }
            e.response = Some(no_content());
            Ok(e)
        },
    )
    .await
}
