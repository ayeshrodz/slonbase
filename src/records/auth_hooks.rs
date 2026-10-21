//! The auth request hooks around the auth endpoints (see [`AuthRequestEvent`]): each
//! endpoint prepares its event, and its action runs as the hook chain's `next`.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use axum::Json;
use axum::extract::{FromRequestParts, Query};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use super::auth_api::{auth_alert, check_mfa, self_json};
use crate::app::App;
use crate::auth::{AuthRecord, RequestAuth};
use crate::collections::model::Collection;
use crate::core::Core;
use crate::error::ApiError;
use crate::filter::RequestInfo;
use crate::hooks::{AuthRequestEvent, Hook, Hooks};
use crate::ip::Client;

/// What an auth endpoint's hooks and actions need of the request (an extractor).
#[derive(Clone)]
pub struct Ctx {
    pub(crate) method: Method,
    pub(crate) client: Client,
    pub(crate) auth: RequestAuth,
    pub(crate) request: Arc<RequestInfo>,
    /// The `mfaId` of a sign-in (body or query string).
    pub(crate) mfa_id: String,
}

impl FromRequestParts<Arc<App>> for Ctx {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        app: &Arc<App>,
    ) -> Result<Self, Self::Rejection> {
        let auth = RequestAuth::from_request_parts(parts, app).await?;
        let Ok(client) = Client::from_request_parts(parts, app).await;
        let query: HashMap<String, String> = Query::try_from_uri(&parts.uri)
            .map(|q| q.0)
            .unwrap_or_default();
        let core = app.core()?;
        let request = super::api::request_info(
            &parts.method,
            &query,
            &parts.headers,
            None,
            &auth,
            &core.registry(),
        );
        Ok(Self {
            method: parts.method.clone(),
            client,
            auth,
            request: Arc::new(request),
            mfa_id: query.get("mfaId").cloned().unwrap_or_default(),
        })
    }
}

impl Ctx {
    /// Adds the request's body (`@request.body`, and its `mfaId`).
    pub(crate) fn with_body(mut self, body: &Map<String, Value>) -> Self {
        Arc::make_mut(&mut self.request).body = body.clone();
        if let Some(id) = body
            .get("mfaId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            self.mfa_id = id.to_string();
        }
        self
    }

    pub(crate) fn event(
        &self,
        core: &Arc<Core>,
        collection: &Arc<Collection>,
        record: Option<AuthRecord>,
        data: Map<String, Value>,
    ) -> AuthRequestEvent {
        AuthRequestEvent {
            core: core.clone(),
            collection: collection.clone(),
            record,
            data,
            auth: self.auth.clone(),
            request: self.request.clone(),
            response: None,
        }
    }
}

/// `data` of an event, from `(key, value)` pairs.
pub(super) fn data<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// Runs the hook's handlers around `action`, which answers the request (sets
/// `e.response`). A handler that doesn't call `next` answers with its own response.
pub(super) async fn run<F, Fut>(
    hook: fn(&Hooks) -> &Hook<AuthRequestEvent>,
    e: AuthRequestEvent,
    action: F,
) -> Result<Response, ApiError>
where
    F: FnOnce(AuthRequestEvent) -> Fut + Send + 'static,
    Fut: Future<Output = Result<AuthRequestEvent, ApiError>> + Send + 'static,
{
    let hooks = e.core.hooks.clone();
    let collection = e.collection.clone();
    let e = hook(&hooks).trigger(Some(&collection), e, action).await?;
    Ok(e.response.unwrap_or_else(|| StatusCode::OK.into_response()))
}

/// The record of an event, for actions that need one.
pub(super) fn record_of(e: &AuthRequestEvent) -> Result<AuthRecord, ApiError> {
    e.record
        .clone()
        .ok_or_else(|| ApiError::bad_request("Failed to authenticate."))
}

/// Answers a sign-in (`auth_method` set), token refresh or impersonation with the token
/// and the record, through `on_record_auth_request`. Sign-ins check MFA first, and
/// remember the device (auth alerts).
pub(super) async fn respond(
    core: &Arc<Core>,
    record: AuthRecord,
    token: String,
    auth_method: &str,
    meta: Option<Value>,
    ctx: Ctx,
) -> Result<Response, ApiError> {
    let collection = record.collection.clone();
    let e = ctx.event(
        core,
        &collection,
        Some(record),
        data([
            ("token", Value::String(token)),
            ("meta", meta.unwrap_or(Value::Null)),
            ("authMethod", Value::String(auth_method.to_string())),
        ]),
    );
    run(
        |h| &h.on_record_auth_request,
        e,
        move |mut e| async move {
            let record = record_of(&e)?;
            let method = e.str("authMethod").to_string();
            if !method.is_empty() {
                if let Some(response) =
                    check_mfa(&e.core, &record, &method, &ctx.method, &ctx.mfa_id).await?
                {
                    e.response = Some(response);
                    return Ok(e);
                }
                if let Err(err) = auth_alert(&e.core, &record, &ctx.client).await {
                    tracing::warn!("auth alert for record {}: {err:#}", record.id());
                }
            }
            // enriched as seen by the record itself
            let mut request = (*ctx.request).clone();
            request.auth = Some(record.info());
            let mut records = [self_json(&record.collection, &record.values)];
            super::api::enrich(&e.core, &request, &record.collection, &mut records).await?;
            let [record_json] = records;
            let mut out = json!({ "token": e.str("token"), "record": record_json });
            if let Some(meta) = e.data.get("meta").filter(|m| !m.is_null()) {
                out["meta"] = meta.clone();
            }
            e.response = Some(Json(out).into_response());
            Ok(e)
        },
    )
    .await
}
