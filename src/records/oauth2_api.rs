//! OAuth2 sign-in (`auth-with-oauth2`), the redirect endpoint used by the SDKs' popup
//! flow (`/api/oauth2-redirect`) and the accounts linked to a record, compatible with
//! PocketBase.
//!
//! The provider's account is linked to a record in the `_externalAuths` collection. Without a
//! link, the signed-in record (if any) is linked, else the record with the provider's
//! verified email, else a new record is created (subject to the collection's create
//! rule).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::{Json, extract};
use bytes::Bytes;
use serde_json::{Map, Value, json};

use super::api;
use super::auth_api::{
    METHOD_OAUTH2, auth_collection, auth_rule_allows, body_str, db_error, find_by_email,
    finish_auth, mfa_id, new_token_key, pool_error,
};
use super::files::{RecordBody, Upload};
use super::store;
use super::values::{self, Values};
use crate::app::App;
use crate::auth::{AuthRecord, RequestAuth};
use crate::collections::model::{
    Collection, FIELD_EMAIL, FIELD_PASSWORD, FIELD_TOKEN_KEY, FIELD_VERIFIED, FieldKind,
};
use crate::collections::schema::table_ref;
use crate::collections::system::EXTERNAL_AUTHS_TABLE;
use crate::core::Core;
use crate::datetime;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::filter::quote_ident;
use crate::oauth2::{self, AuthUser, Provider};
use crate::validation::Errors;
use crate::{ids, realtime};

fn str_of<'a>(values: &'a Values, key: &str) -> &'a str {
    values.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// `POST /api/collections/{collection}/auth-with-oauth2`
#[allow(clippy::too_many_arguments)]
pub async fn auth_with_oauth2(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    method: Method,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    let options = c.auth_options().expect("auth collection");
    if !options.oauth2.enabled {
        return Err(ApiError::forbidden(
            "The collection is not configured to allow OAuth2 authentication.",
        ));
    }

    let provider_name = body_str(&body, "provider");
    let code = body_str(&body, "code");
    let verifier = body_str(&body, "codeVerifier");
    let redirect_url = match body_str(&body, "redirectURL") {
        "" => body_str(&body, "redirectUrl"),
        url => url,
    };
    let mut errors = Errors::new();
    for (key, value) in [
        ("provider", provider_name),
        ("code", code),
        ("redirectURL", redirect_url),
    ] {
        if value.is_empty() {
            errors.add(key, "validation_required", "Cannot be blank.");
        }
    }
    let provider = options
        .oauth2
        .providers
        .iter()
        .find(|p| p.name == provider_name)
        .and_then(Provider::new);
    if provider.is_none() && !provider_name.is_empty() {
        errors.invalid(
            "provider",
            format!("Provider with name {provider_name:?} is missing or is not enabled."),
        );
    }
    let create_data = match body.get("createData") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(data)) => data.clone(),
        Some(_) => {
            errors.invalid("createData", "Must be an object.");
            Map::new()
        }
    };
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(
            "An error occurred while validating the submitted data.",
            &errors,
        ));
    }
    let provider = provider.expect("checked above");

    let token = provider
        .exchange(code, verifier, redirect_url)
        .await
        .map_err(|e| {
            tracing::debug!(provider = %provider.name, error = %format!("{e:#}"), "OAuth2 code exchange failed");
            ApiError::bad_request("Failed to fetch OAuth2 token.")
        })?;
    let user = provider.user(&token).await.map_err(|e| {
        tracing::debug!(provider = %provider.name, error = %format!("{e:#}"), "fetching the OAuth2 user failed");
        ApiError::bad_request("Failed to fetch OAuth2 user.")
    })?;

    // the linked record, else the signed-in one, else the one with the same email
    let client = core.pool.get().await.map_err(pool_error)?;
    let mut found = None;
    let mut linked = false;
    if let Some(row) = client
        .query_opt(
            &format!(
                r#"SELECT "recordRef" FROM {EXTERNAL_AUTHS_TABLE}
                   WHERE "collectionRef" = $1 AND provider = $2 AND "providerId" = $3"#
            ),
            &[&c.id, &provider.name, &user.id],
        )
        .await
        .map_err(db_error)?
    {
        let record_id: String = row.get(0);
        found = store::find_by_id(&client, &c, &record_id)
            .await
            .map_err(db_error)?;
        linked = found.is_some();
    }
    drop(client);
    let signed_in = auth
        .any_record()
        .cloned()
        .filter(|r| r.collection.id == c.id);
    if found.is_none() {
        if let Some(record) = &signed_in {
            found = Some(record.values.clone());
        } else if !user.email.is_empty() {
            found = find_by_email(&core, &c, &user.email)
                .await?
                .map(|r| r.values);
        }
    }

    let is_new = found.is_none();
    let values = match found {
        Some(values) => {
            let is_signed_in = signed_in
                .as_ref()
                .is_some_and(|r| r.id() == str_of(&values, "id"));
            update_existing(&core, &c, values, &user, is_signed_in).await?
        }
        None => create(&app, &core, &c, create_data, &user).await?,
    };
    let record_id = str_of(&values, "id").to_string();

    if !linked {
        let client = core.pool.get().await.map_err(pool_error)?;
        client
            .execute(
                &format!(
                    r#"INSERT INTO {EXTERNAL_AUTHS_TABLE}
                           (id, "collectionRef", "recordRef", provider, "providerId", created, updated)
                       VALUES ($1, $2, $3, $4, $5, now(), now()) ON CONFLICT DO NOTHING"#
                ),
                &[
                    &ids::random_id(ids::DEFAULT_ID_LENGTH),
                    &c.id,
                    &record_id,
                    &provider.name,
                    &user.id,
                ],
            )
            .await
            .map_err(db_error)?;
        let owner: Option<String> = client
            .query_opt(
                &format!(
                    r#"SELECT "recordRef" FROM {EXTERNAL_AUTHS_TABLE}
                       WHERE "collectionRef" = $1 AND provider = $2 AND "providerId" = $3"#
                ),
                &[&c.id, &provider.name, &user.id],
            )
            .await
            .map_err(db_error)?
            .map(|row| row.get(0));
        if owner.as_deref() != Some(record_id.as_str()) {
            return Err(ApiError::bad_request(format!(
                "The record is already linked to another {} account.",
                provider.display_name
            )));
        }
    }

    let record = AuthRecord {
        collection: c,
        values,
        refreshable: true,
    };
    if !auth_rule_allows(&core, &record, &method).await? {
        return Err(ApiError::forbidden(
            "The request doesn't satisfy the collection requirements to authenticate.",
        ));
    }
    finish_auth(
        &core,
        &record,
        METHOD_OAUTH2,
        &method,
        mfa_id(&body, &query),
        Some(user.meta(is_new)),
    )
    .await
}

/// Updates an existing record signing in with a provider. An unverified record with the
/// provider's (verified) email may have been created by someone who doesn't own the
/// address, so its password and sessions are reset before it's marked verified.
async fn update_existing(
    core: &Core,
    c: &Collection,
    values: Values,
    user: &AuthUser,
    is_signed_in: bool,
) -> Result<Values, ApiError> {
    let id = str_of(&values, "id").to_string();
    let email = str_of(&values, FIELD_EMAIL).to_string();
    let verified = values
        .get(FIELD_VERIFIED)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut sets: Vec<(&str, Value)> = Vec::new();

    if !user.email.is_empty() && !verified && email.eq_ignore_ascii_case(&user.email) {
        if !is_signed_in {
            let field = c
                .field(FIELD_PASSWORD)
                .expect("auth collections have a password");
            let hash = values::hash_password(field, ids::random_secret(24))
                .await
                .map_err(ApiError::internal)?;
            sets.push((FIELD_PASSWORD, Value::String(hash)));
            sets.push((FIELD_TOKEN_KEY, Value::String(new_token_key())));
        }
        sets.push((FIELD_VERIFIED, Value::Bool(true)));
    }
    if email.is_empty() && !user.email.is_empty() {
        sets.push((FIELD_EMAIL, Value::String(user.email.clone())));
        sets.push((FIELD_VERIFIED, Value::Bool(true)));
    }
    if sets.is_empty() {
        return Ok(values);
    }
    sets.dedup_by(|a, b| a.0 == b.0);

    let client = core.pool.get().await.map_err(pool_error)?;
    let mut assignments = Vec::new();
    let mut texts: Vec<String> = vec![id.clone()];
    for (column, value) in &sets {
        match value {
            Value::Bool(b) => assignments.push(format!("{} = {b}", quote_ident(column))),
            Value::String(s) => {
                texts.push(s.clone());
                assignments.push(format!("{} = ${}::text", quote_ident(column), texts.len()));
            }
            _ => unreachable!(),
        }
    }
    let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        texts.iter().map(|t| t as _).collect();
    client
        .execute(
            &format!(
                "UPDATE {} SET {} WHERE id = $1::text",
                table_ref(&c.name),
                assignments.join(", ")
            ),
            &params,
        )
        .await
        .map_err(|e| {
            if e.as_db_error().map(|d| d.code())
                == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION)
            {
                ApiError::bad_request("The provider's email is already used by another record.")
            } else {
                db_error(e)
            }
        })?;
    store::find_by_id(&client, c, &id)
        .await
        .map_err(db_error)?
        .ok_or_else(ApiError::not_found)
}

/// Creates a record for a new provider account, through the records API as a guest
/// (so the create rule applies), from `createData` and the mapped provider fields.
async fn create(
    app: &Arc<App>,
    core: &Core,
    c: &Arc<Collection>,
    mut data: Map<String, Value>,
    user: &AuthUser,
) -> Result<Values, ApiError> {
    let options = c.auth_options().expect("auth collection");
    if !user.email.is_empty() {
        data.insert(FIELD_EMAIL.into(), Value::String(user.email.clone()));
    }
    if !data.contains_key(FIELD_PASSWORD) {
        let password = ids::random_secret(24);
        data.insert(FIELD_PASSWORD.into(), Value::String(password.clone()));
        data.insert("passwordConfirm".into(), Value::String(password));
    }
    let mapped = &options.oauth2.mapped_fields;
    let mut uploads = Vec::new();
    for (field_name, value) in [
        (&mapped.id, &user.id),
        (&mapped.name, &user.name),
        (&mapped.username, &user.username),
    ] {
        if !field_name.is_empty() && !value.is_empty() && !data.contains_key(field_name) {
            data.insert(field_name.clone(), Value::String(value.clone()));
        }
    }
    let avatar_field = c
        .field(&mapped.avatar_url)
        .filter(|_| !mapped.avatar_url.is_empty());
    if let Some(field) =
        avatar_field.filter(|f| !data.contains_key(&f.name) && !user.avatar_url.is_empty())
    {
        if matches!(field.kind, FieldKind::File(_)) {
            if let Some(file) = oauth2::download(&user.avatar_url).await {
                uploads.push(Upload {
                    key: field.name.clone(),
                    original_name: "avatar".into(),
                    data: Bytes::from(file),
                });
            }
        } else {
            data.insert(field.name.clone(), Value::String(user.avatar_url.clone()));
        }
    }

    let created = api::create_request(
        &app.core()?,
        &c.name,
        &HashMap::new(),
        &Method::POST,
        &HeaderMap::new(),
        RequestAuth::default(),
        RecordBody {
            data,
            uploads,
            from_form: false,
        },
    )
    .await?;
    let id = created
        .record
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let client = core.pool.get().await.map_err(pool_error)?;
    if !user.email.is_empty() {
        client
            .execute(
                &format!(
                    "UPDATE {} SET {} = TRUE WHERE id = $1::text",
                    table_ref(&c.name),
                    quote_ident(FIELD_VERIFIED)
                ),
                &[&id],
            )
            .await
            .map_err(db_error)?;
    }
    store::find_by_id(&client, c, &id)
        .await
        .map_err(db_error)?
        .ok_or_else(ApiError::not_found)
}

// ---------------------------------------------------------------------------
// the redirect endpoint

fn redirect_page(success: bool) -> Response {
    let message = if success {
        "Sign-in complete. You can close this window and go back to the app."
    } else {
        "Sign-in failed. Close this window and try again."
    };
    let status = if success {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    (
        status,
        Html(format!(
            "<!doctype html><meta charset=utf-8><title>slonbase</title>\
             <p style=\"font-family:system-ui;margin:3rem\">{message}</p>\
             <script>window.close()</script>"
        )),
    )
        .into_response()
}

async fn redirect(core: &Core, params: &HashMap<String, String>) -> Response {
    let get = |key: &str| params.get(key).cloned().unwrap_or_default();
    let state = get("state");
    if state.is_empty() {
        return redirect_page(false);
    }
    let data = json!({ "state": state, "code": get("code"), "error": get("error") }).to_string();
    if data.len() > 7000 {
        return redirect_page(false);
    }
    if !realtime::send_to_client(core, &state, realtime::OAUTH2_TOPIC, data.clone()) {
        // the client may be connected to another instance
        let sent = async {
            let client = core.pool.get().await.ok()?;
            realtime::notify(&client, &format!("oauth2:{data}"))
                .await
                .ok()
        };
        if sent.await.is_none() {
            return redirect_page(false);
        }
    }
    redirect_page(true)
}

/// `GET /api/oauth2-redirect`: where providers send the user back in the SDKs' popup
/// flow. The code is passed to the realtime client whose id is the `state`.
pub async fn redirect_get(
    State(app): State<Arc<App>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    Ok(redirect(&core, &params).await)
}

/// `POST /api/oauth2-redirect` (`response_mode=form_post`)
pub async fn redirect_post(
    State(app): State<Arc<App>>,
    Form(params): Form<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    Ok(redirect(&core, &params).await)
}

// ---------------------------------------------------------------------------
// linked accounts

fn owner_or_superuser(auth: &RequestAuth, c: &Collection, id: &str) -> Result<(), ApiError> {
    if auth.is_superuser() || auth.is_record(&c.id, id) {
        Ok(())
    } else if auth.record.is_none() {
        Err(ApiError::unauthorized(
            "The request requires valid record authorization token.",
        ))
    } else {
        Err(ApiError::forbidden(
            "Only superusers and the record itself can manage its linked accounts.",
        ))
    }
}

/// `GET /api/collections/{collection}/records/{id}/external-auths`
pub async fn list_external_auths(
    State(app): State<Arc<App>>,
    extract::Path((collection, id)): extract::Path<(String, String)>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    owner_or_superuser(&auth, &c, &id)?;
    let client = core.pool.get().await.map_err(pool_error)?;
    let rows = client
        .query(
            &format!(
                r#"SELECT id, provider, "providerId", {}, {} FROM {EXTERNAL_AUTHS_TABLE}
                   WHERE "collectionRef" = $1 AND "recordRef" = $2 ORDER BY created"#,
                datetime::sql_format("created"),
                datetime::sql_format("updated")
            ),
            &[&c.id, &id],
        )
        .await
        .map_err(db_error)?;
    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<_, String>(0),
                "collectionId": c.id,
                "recordId": id,
                "provider": row.get::<_, String>(1),
                "providerId": row.get::<_, String>(2),
                "created": row.get::<_, String>(3),
                "updated": row.get::<_, String>(4),
            })
        })
        .collect();
    Ok(Json(Value::Array(items)))
}

/// `DELETE /api/collections/{collection}/records/{id}/external-auths/{provider}`
pub async fn unlink_external_auth(
    State(app): State<Arc<App>>,
    extract::Path((collection, id, provider)): extract::Path<(String, String, String)>,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    owner_or_superuser(&auth, &c, &id)?;
    let client = core.pool.get().await.map_err(pool_error)?;
    let deleted = client
        .execute(
            &format!(
                r#"DELETE FROM {EXTERNAL_AUTHS_TABLE}
                   WHERE "collectionRef" = $1 AND "recordRef" = $2 AND provider = $3"#
            ),
            &[&c.id, &id, &provider],
        )
        .await
        .map_err(db_error)?;
    if deleted == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}
