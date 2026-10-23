//! One-time passwords sent by email (`request-otp`, `auth-with-otp`), compatible with
//! PocketBase. Only a bcrypt hash of each password is stored; a password is single-use,
//! expires after the collection's `otp.duration` and is invalidated after a few wrong
//! attempts.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::auth_api::{
    METHOD_OTP, auth_collection, auth_rule_allows, body_str, db_error, find_by_email, pool_error,
    with_response,
};
use super::auth_hooks::{self, Ctx};
use super::store;
use super::values;
use crate::app::App;
use crate::auth::AuthRecord;
use crate::collections::model::{Collection, FIELD_VERIFIED};
use crate::collections::schema::table_ref;
use crate::core::Core;
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
    ctx: Ctx,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
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
    let record = find_by_email(&core, &c, email).await?;
    let length = c
        .auth_options()
        .expect("auth collection")
        .otp
        .length
        .clamp(4, 32);
    let password =
        ids::generate_from_pattern(&format!("[0-9]{{{length}}}")).expect("valid pattern");

    let ctx = ctx.with_body(&body);
    let e = ctx.event(
        &core,
        &c,
        record,
        auth_hooks::data([("password", Value::String(password))]),
    );
    auth_hooks::run(
        |h| &h.on_record_request_otp_request,
        e,
        |mut e| async move {
            let otp_id = create_otp(
                e.core.clone(),
                e.collection.clone(),
                e.record.clone(),
                e.str("password").to_string(),
            )
            .await?;
            e.response = Some(Json(json!({ "otpId": otp_id })).into_response());
            Ok(e)
        },
    )
    .await
}

/// Creates and emails the event's one-time password, returning its id: an id that
/// looks like a real one when there's no record, and the last one when the record has
/// too many.
async fn create_otp(
    core: Arc<Core>,
    c: Arc<Collection>,
    record: Option<AuthRecord>,
    password: String,
) -> Result<String, ApiError> {
    let Some(record) = record else {
        return Ok(ids::random_id(ids::DEFAULT_ID_LENGTH));
    };
    let options = c.auth_options().expect("auth collection").otp.clone();

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
        return Ok(active[0].get::<_, String>(0));
    }
    drop(client);

    // hashing, saving and sending happen in the background, so the answer takes as long
    // as for an unknown email
    let id = ids::random_id(ids::DEFAULT_ID_LENGTH);
    let otp_id = id.clone();
    tokio::spawn(async move {
        let plain = password.clone();
        let saved = async {
            let hash = tokio::task::spawn_blocking(move || bcrypt::hash(plain, OTP_COST)).await??;
            let client = core.pool.get().await?;
            client
                .execute(
                    "INSERT INTO _slonbase.otps (id, collection_id, record_id, password_hash, sent_to)
                     VALUES ($1, $2, $3, $4, $5)",
                    &[&id, &c.id, &record.id(), &hash, &record.email()],
                )
                .await?;
            anyhow::Ok(())
        };
        if let Err(err) = saved.await {
            tracing::error!(error = %format!("{err:#}"), "failed to create an OTP");
            return;
        }
        send_otp(&core, &record, &id, &password);
    });
    Ok(otp_id)
}

/// Emails the record a one-time password (with its id).
pub(crate) fn send_otp(core: &Core, record: &AuthRecord, otp_id: &str, password: &str) {
    let options = &record
        .collection
        .auth_options()
        .expect("auth collection")
        .otp;
    let settings = core.settings();
    let fill = |template: &str| {
        mail::render(
            &template
                .replace("{OTP}", password)
                .replace("{OTP_ID}", otp_id),
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
        Some(record),
    );
}

/// `POST /api/collections/{collection}/auth-with-otp` (`{"otpId", "password"}`)
pub async fn auth_with_otp(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    ctx: Ctx,
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
    // each guess takes an attempt before it's checked, in the statement that checks the
    // limit, so guesses sent at the same time can't get past it
    let row = client
        .query_opt(
            "UPDATE _slonbase.otps SET attempts = attempts + 1
             WHERE id = $1 AND collection_id = $2 AND attempts < $3
               AND created > now() - $4::bigint * interval '1 second'
             RETURNING record_id, password_hash, sent_to",
            &[&otp_id, &c.id, &MAX_ATTEMPTS, &options.duration],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(invalid)?;
    let (record_id, hash, sent_to): (String, String, String) = (row.get(0), row.get(1), row.get(2));

    if !values::verify_password(password.to_string(), hash).await {
        return Err(invalid());
    }
    let values = store::find_by_id(&client, &c, &record_id)
        .await
        .map_err(db_error)?
        .ok_or_else(invalid)?;
    drop(client);

    let record = AuthRecord {
        collection: c.clone(),
        values,
        refreshable: true,
    };
    let otp = json!({
        "id": otp_id,
        "collectionRef": c.id,
        "recordRef": record_id,
        "sentTo": sent_to,
    });
    let ctx = ctx.with_body(&body);
    let e = ctx.event(&core, &c, Some(record), auth_hooks::data([("otp", otp)]));
    auth_hooks::run(
        |h| &h.on_record_auth_with_otp_request,
        e,
        move |e| async move {
            let record =
                use_otp(&e.core, auth_hooks::record_of(&e)?, e.data["otp"].clone()).await?;
            if !auth_rule_allows(&e.core, &record, &ctx.method).await? {
                return Err(ApiError::forbidden(
                    "The request doesn't satisfy the collection requirements to authenticate.",
                ));
            }
            let token = record.issue_auth_token(&e.core, None, true);
            let response =
                auth_hooks::respond(&e.core, record, token, METHOD_OTP, None, ctx).await?;
            Ok(with_response(e, response))
        },
    )
    .await
}

/// Uses up the event's OTP, and marks the record verified when the password was sent to
/// its email (receiving it proves the address). Returns the record.
async fn use_otp(core: &Core, mut record: AuthRecord, otp: Value) -> Result<AuthRecord, ApiError> {
    let invalid = || ApiError::bad_request("Invalid or expired OTP.");
    let client = core.pool.get().await.map_err(pool_error)?;
    // single use, also when two requests race
    if client
        .execute(
            "DELETE FROM _slonbase.otps WHERE id = $1",
            &[&otp["id"].as_str().unwrap_or_default()],
        )
        .await
        .map_err(db_error)?
        == 0
    {
        return Err(invalid());
    }
    let sent_to = otp["sentTo"].as_str().unwrap_or_default();
    let verified = record
        .values
        .get(FIELD_VERIFIED)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !verified && !record.email().is_empty() && record.email().eq_ignore_ascii_case(sent_to) {
        client
            .execute(
                &format!(
                    "UPDATE {} SET {} = TRUE WHERE id = $1::text",
                    table_ref(&record.collection.name),
                    quote_ident(FIELD_VERIFIED)
                ),
                &[&record.id()],
            )
            .await
            .map_err(db_error)?;
        record
            .values
            .insert(FIELD_VERIFIED.into(), Value::Bool(true));
    }
    Ok(record)
}

/// Creates a one-time password for the auth record with `email` without sending it (the
/// `slonbase superuser otp` command): its id and the password. Fails when the collection's
/// OTPs are off or no record has the email.
pub async fn issue(
    core: &crate::core::Core,
    collection: &str,
    email: &str,
) -> anyhow::Result<(String, String)> {
    let c = core
        .registry()
        .get(collection)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no collection {collection:?}"))?;
    let options = c
        .auth_options()
        .ok_or_else(|| anyhow::anyhow!("{collection} isn't an auth collection"))?
        .otp
        .clone();
    anyhow::ensure!(
        options.enabled,
        "one-time passwords are off for {collection}: turn them on in the collection's sign-in options"
    );
    let record = find_by_email(core, &c, email)
        .await
        .map_err(|e| anyhow::anyhow!(e.message))?
        .ok_or_else(|| anyhow::anyhow!("no {collection} record has the email {email:?}"))?;
    let length = options.length.clamp(4, 32);
    let password =
        ids::generate_from_pattern(&format!("[0-9]{{{length}}}")).expect("valid pattern");
    let plain = password.clone();
    let hash = tokio::task::spawn_blocking(move || bcrypt::hash(plain, OTP_COST)).await??;
    let id = ids::random_id(ids::DEFAULT_ID_LENGTH);
    let client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    client
        .execute(
            "INSERT INTO _slonbase.otps (id, collection_id, record_id, password_hash, sent_to)
             VALUES ($1, $2, $3, $4, $5)",
            &[&id, &c.id, &record.id(), &hash, &record.email()],
        )
        .await?;
    Ok((id, password))
}
