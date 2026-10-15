//! One-time passwords sent by email (`request-otp`, `auth-with-otp`), compatible with
//! PocketBase. Only a bcrypt hash of each password is stored; a password is single-use,
//! expires after the collection's `otp.duration` and is invalidated after a few wrong
//! attempts.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::Method;
use axum::response::Response;
use serde_json::{Value, json};

use super::auth_api::{
    METHOD_OTP, auth_collection, auth_rule_allows, body_str, db_error, find_by_email, finish_auth,
    mfa_id, pool_error,
};
use super::store;
use super::values;
use crate::app::App;
use crate::auth::AuthRecord;
use crate::collections::model::{Collection, FIELD_VERIFIED};
use crate::collections::schema::table_ref;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::filter::quote_ident;
use crate::ids;
use crate::mail;
use crate::validation::Errors;

/// Wrong passwords tried before an OTP is invalidated.
const MAX_ATTEMPTS: i32 = 5;
/// Unexpired OTPs a record can have; further requests reuse the last one (no new email).
const MAX_ACTIVE: i64 = 9;
const OTP_COST: u32 = 10;

fn otp_enabled(c: &Collection) -> Result<(), ApiError> {
    if !c.auth_options().is_some_and(|a| a.otp.enabled) {
        return Err(ApiError::forbidden(
            "The collection is not configured to allow OTP authentication.",
        ));
    }
    Ok(())
}

/// `POST /api/collections/{collection}/request-otp`: emails a one-time password and
/// returns its id. The answer is the same whether or not the email is registered.
pub async fn request_otp(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    otp_enabled(&c)?;
    let email = body_str(&body, "email").trim();
    if email.is_empty() {
        let mut errors = Errors::new();
        errors.add("email", "validation_required", "Cannot be blank.");
        return Err(ApiError::invalid_data(
            "An error occurred while validating the submitted data.",
            &errors,
        ));
    }
    let options = &c.auth_options().expect("auth collection").otp;

    let Some(record) = find_by_email(&core, &c, email).await? else {
        // an id that looks like a real one
        return Ok(Json(
            json!({ "otpId": ids::random_id(ids::DEFAULT_ID_LENGTH) }),
        ));
    };

    let client = core.pool.get().await.map_err(pool_error)?;
    let active = client
        .query(
            "SELECT id FROM _slonbase.otps
             WHERE collection_id = $1 AND record_id = $2 AND attempts < $4
               AND created > now() - $3::bigint * interval '1 second'
             ORDER BY created DESC",
            &[&c.id, &record.id(), &options.duration, &MAX_ATTEMPTS],
        )
        .await
        .map_err(db_error)?;
    if active.len() as i64 >= MAX_ACTIVE {
        return Ok(Json(json!({ "otpId": active[0].get::<_, String>(0) })));
    }

    drop(client);

    // hashing, saving and sending happen in the background, so the answer takes as long
    // as for an unknown email
    let id = ids::random_id(ids::DEFAULT_ID_LENGTH);
    let otp_id = id.clone();
    let options = options.clone();
    let collection_id = c.id.clone();
    let core = core.clone();
    tokio::spawn(async move {
        let length = options.length.clamp(4, 32);
        let password =
            ids::generate_from_pattern(&format!("[0-9]{{{length}}}")).expect("valid pattern");
        let plain = password.clone();
        let saved = async {
            let hash = tokio::task::spawn_blocking(move || bcrypt::hash(plain, OTP_COST)).await??;
            let client = core.pool.get().await?;
            client
                .execute(
                    "INSERT INTO _slonbase.otps (id, collection_id, record_id, password_hash, sent_to)
                     VALUES ($1, $2, $3, $4, $5)",
                    &[&id, &collection_id, &record.id(), &hash, &record.email()],
                )
                .await?;
            anyhow::Ok(())
        };
        if let Err(err) = saved.await {
            tracing::error!(error = %format!("{err:#}"), "failed to create an OTP");
            return;
        }
        let settings = core.settings();
        let fill = |template: &str| {
            mail::render(
                &template
                    .replace("{OTP}", &password)
                    .replace("{OTP_ID}", &id),
                &settings,
                "",
            )
        };
        core.send_mail_for(
            mail::Message {
                to: record.email().to_string(),
                subject: fill(&options.email_template.subject),
                html: fill(&options.email_template.body),
            },
            "otp",
            Some(&record),
        );
    });
    Ok(Json(json!({ "otpId": otp_id })))
}

/// `POST /api/collections/{collection}/auth-with-otp` (`{"otpId", "password"}`)
pub async fn auth_with_otp(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    method: Method,
    origin: crate::ip::Client,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = auth_collection(&core, &collection)?;
    otp_enabled(&c)?;
    let options = &c.auth_options().expect("auth collection").otp;
    let invalid = || ApiError::bad_request("Invalid or expired OTP.");

    let otp_id = body_str(&body, "otpId");
    let password = body_str(&body, "password");
    if otp_id.is_empty() || password.is_empty() {
        let mut errors = Errors::new();
        for (key, value) in [("otpId", otp_id), ("password", password)] {
            if value.is_empty() {
                errors.add(key, "validation_required", "Cannot be blank.");
            }
        }
        return Err(ApiError::invalid_data(
            "An error occurred while validating the submitted data.",
            &errors,
        ));
    }

    let client = core.pool.get().await.map_err(pool_error)?;
    let row = client
        .query_opt(
            "SELECT record_id, password_hash, sent_to FROM _slonbase.otps
             WHERE id = $1 AND collection_id = $2 AND attempts < $3
               AND created > now() - $4::bigint * interval '1 second'",
            &[&otp_id, &c.id, &MAX_ATTEMPTS, &options.duration],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(invalid)?;
    let (record_id, hash, sent_to): (String, String, String) = (row.get(0), row.get(1), row.get(2));

    if !values::verify_password(password.to_string(), hash).await {
        client
            .execute(
                "UPDATE _slonbase.otps SET attempts = attempts + 1 WHERE id = $1",
                &[&otp_id],
            )
            .await
            .map_err(db_error)?;
        return Err(invalid());
    }
    // single use, also when two requests race
    if client
        .execute("DELETE FROM _slonbase.otps WHERE id = $1", &[&otp_id])
        .await
        .map_err(db_error)?
        == 0
    {
        return Err(invalid());
    }

    let mut values = store::find_by_id(&client, &c, &record_id)
        .await
        .map_err(db_error)?
        .ok_or_else(invalid)?;
    // receiving the password proves the email address
    let email = values
        .get(crate::collections::model::FIELD_EMAIL)
        .and_then(Value::as_str)
        .unwrap_or_default();
    let verified = values
        .get(FIELD_VERIFIED)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !verified && !email.is_empty() && email.eq_ignore_ascii_case(&sent_to) {
        client
            .execute(
                &format!(
                    "UPDATE {} SET {} = TRUE WHERE id = $1::text",
                    table_ref(&c.name),
                    quote_ident(FIELD_VERIFIED)
                ),
                &[&record_id],
            )
            .await
            .map_err(db_error)?;
        values.insert(FIELD_VERIFIED.into(), Value::Bool(true));
    }
    drop(client);

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
        METHOD_OTP,
        &method,
        &origin,
        mfa_id(&body, &query),
        None,
    )
    .await
}
