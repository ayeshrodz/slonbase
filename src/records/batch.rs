//! Batch requests (`POST /api/batch`), compatible with PocketBase: record creates,
//! updates, upserts and deletes run in one transaction, so either all of them apply or
//! none does.
//!
//! The body is `{"requests": [{"method", "url", "headers", "body"}]}`, as JSON or as
//! multipart form data with the JSON in `@jsonPayload` and files named
//! `requests.{index}.{field}`. Each request runs with the batch's auth (its own
//! `Authorization` header is ignored) and its response is `{"status", "body"}`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use super::txtask::TxHandle;

use super::api::{WriteContext, create_in, delete_in, update_in};
use super::files::{RecordBody, Upload};
use super::store;
use crate::app::App;
use crate::auth::RequestAuth;
use crate::collections::model::{Collection, RuleAction};
use crate::collections::store::Registry;
use crate::core::Core;
use crate::error::ApiError;
use crate::hooks::{DataEvent, RecordRequestEvent};

/// The default and maximum body size (PocketBase's default).
const MAX_BODY: u64 = 128 << 20;

enum Action {
    Create,
    Upsert,
    Update(String),
    Delete(String),
}

struct SubRequest {
    method: Method,
    collection: Arc<Collection>,
    action: Action,
    query: HashMap<String, String>,
    headers: HeaderMap,
    body: Map<String, Value>,
    uploads: Vec<Upload>,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::bad_request(message)
}

/// The error of the request at `index`, in PocketBase's shape.
fn failed_at(index: usize, error: ApiError) -> ApiError {
    let mut err = ApiError::bad_request("Batch transaction failed.");
    let response = json!({
        "status": error.status.as_u16(),
        "message": error.message,
        "data": error.data,
    });
    err.data.insert(
        "requests".into(),
        json!({ index.to_string(): {
            "code": "batch_request_failed",
            "message": "Batch request failed.",
            "response": response,
        } }),
    );
    err.details = error.details;
    err
}

fn parse(
    index: usize,
    raw: &Value,
    uploads: &mut Vec<Upload>,
    registry: &crate::collections::store::Registry,
    base_headers: &HeaderMap,
) -> Result<SubRequest, ApiError> {
    let str_of = |key: &str| raw.get(key).and_then(Value::as_str).unwrap_or_default();
    let method = Method::from_bytes(str_of("method").to_ascii_uppercase().as_bytes())
        .map_err(|_| invalid("Invalid batch request method."))?;
    let url = str_of("url");
    let (path, query_string) = url.split_once('?').unwrap_or((url, ""));
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let (collection, action) = match (method.as_str(), segments.as_slice()) {
        ("POST", ["api", "collections", c, "records"]) => (*c, Action::Create),
        ("PUT", ["api", "collections", c, "records"]) => (*c, Action::Upsert),
        ("PATCH", ["api", "collections", c, "records", id]) => (*c, Action::Update(id.to_string())),
        ("DELETE", ["api", "collections", c, "records", id]) => {
            (*c, Action::Delete(id.to_string()))
        }
        _ => return Err(invalid("Unknown batch request action.")),
    };
    let collection = registry
        .get(collection)
        .cloned()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "Missing collection context."))?;

    let query = form_urlencoded::parse(query_string.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut headers = base_headers.clone();
    if let Some(Value::Object(own)) = raw.get("headers") {
        for (name, value) in own {
            let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
                continue;
            };
            if name == header::AUTHORIZATION {
                continue; // the batch's auth applies
            }
            if let Some(value) = value.as_str().and_then(|v| HeaderValue::from_str(v).ok()) {
                headers.insert(name, value);
            }
        }
    }
    let body = match raw.get("body") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(body)) => body.clone(),
        Some(_) => return Err(invalid("The batch request body must be an object.")),
    };
    // files of this request: `requests.{index}.{field}` (or `requests[{index}].{field}`)
    let prefixes = [format!("requests.{index}."), format!("requests[{index}].")];
    let mut own_uploads = Vec::new();
    let mut i = 0;
    while i < uploads.len() {
        let field = prefixes
            .iter()
            .find_map(|p| uploads[i].key.strip_prefix(p.as_str()))
            .map(str::to_string);
        match field {
            Some(field) => {
                let mut upload = uploads.remove(i);
                upload.key = field;
                own_uploads.push(upload);
            }
            None => i += 1,
        }
    }
    Ok(SubRequest {
        method,
        collection,
        action,
        query,
        headers,
        body,
        uploads: own_uploads,
    })
}

/// `POST /api/batch`
pub async fn batch(
    State(app): State<Arc<App>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    auth: RequestAuth,
    mut input: RecordBody,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let settings = core.settings().batch.clone();
    let timeout_secs = settings.timeout;
    if !settings.enabled || settings.max_requests == 0 {
        return Err(ApiError::forbidden("Batch requests are not allowed."));
    }
    let max_body = match settings.max_body_size {
        0 => MAX_BODY,
        size => size.min(MAX_BODY),
    };
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if length > max_body {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request entity too large.",
        ));
    }

    let raw_requests = match input.data.get("requests") {
        Some(Value::Array(items)) if !items.is_empty() => items.clone(),
        _ => {
            let mut errors = crate::validation::Errors::new();
            errors.add("requests", "validation_required", "Cannot be blank.");
            return Err(ApiError::invalid_data(
                "Invalid batch request data.",
                &errors,
            ));
        }
    };
    if raw_requests.len() > settings.max_requests as usize {
        let mut errors = crate::validation::Errors::new();
        errors.invalid(
            "requests",
            format!(
                "The batch can have at most {} requests.",
                settings.max_requests
            ),
        );
        return Err(ApiError::invalid_data(
            "Invalid batch request data.",
            &errors,
        ));
    }
    let request = Arc::new(super::api::request_info(
        &Method::POST,
        &query,
        &headers,
        None,
        &auth,
        &core.registry(),
    ));
    let e = DataEvent {
        core: core.clone(),
        collection: None,
        auth: auth.clone(),
        request: Some(request),
        data: crate::hooks::data([("batch", Value::Array(raw_requests))]),
        response: None,
    };
    let uploads = std::mem::take(&mut input.uploads);
    let from_form = input.from_form;
    let e = core
        .hooks
        .clone()
        .on_batch_request
        .trigger(None, e, move |mut e| async move {
            let requests = match &e.data["batch"] {
                Value::Array(items) => items.clone(),
                _ => {
                    return Err(ApiError::bad_request(
                        "The batch must be a list of requests.",
                    ));
                }
            };
            let results = run_batch(
                &e.core,
                &headers,
                &e.auth,
                requests,
                uploads,
                from_form,
                timeout_secs,
            )
            .await?;
            e.response = Some(Json(results).into_response());
            Ok(e)
        })
        .await?;
    Ok(e.response.unwrap_or_else(|| StatusCode::OK.into_response()))
}

/// Runs the batch's requests in one transaction and returns their responses.
async fn run_batch(
    core: &Arc<Core>,
    headers: &HeaderMap,
    auth: &RequestAuth,
    raw_requests: Vec<Value>,
    mut uploads: Vec<Upload>,
    from_form: bool,
    timeout_secs: u64,
) -> Result<Value, ApiError> {
    let registry = core.registry();
    let mut base_headers = headers.clone();
    // the parts' own content type, not the batch's
    base_headers.remove(header::CONTENT_TYPE);
    base_headers.remove(header::CONTENT_LENGTH);
    let mut requests = Vec::with_capacity(raw_requests.len());
    for (i, raw) in raw_requests.iter().enumerate() {
        requests.push(
            parse(i, raw, &mut uploads, &registry, &base_headers).map_err(|e| failed_at(i, e))?,
        );
    }

    let (tx, effects, worker) = TxHandle::start(core);

    let run = async {
        let mut done = Vec::with_capacity(requests.len());
        for (i, request) in requests.into_iter().enumerate() {
            let item = run_request(core, &registry, auth, &tx, request, from_form)
                .await
                .map_err(|e| failed_at(i, e))?;
            done.push(item);
        }
        tx.commit().await?;
        Ok(done)
    };
    let timeout = Duration::from_secs(timeout_secs.max(1));
    let outcome = match tokio::time::timeout(timeout, run).await {
        Ok(outcome) => outcome,
        Err(_) => Err(ApiError::bad_request(
            "Batch transaction failed: the batch took too long.",
        )),
    };
    // without a commit, the transaction rolls back when its task ends
    drop(tx);
    if outcome.is_err() {
        worker.abort();
    }
    let _ = worker.await;
    let effects = std::mem::take(&mut *effects.lock().await);
    let done = effects.settle(core, outcome).await?;

    let mut results = Vec::with_capacity(done.len());
    for Done {
        query,
        event,
        deleted,
    } in done
    {
        if let Some(response) = event.response {
            results.push(response_json(response).await);
        } else if deleted {
            results.push(json!({ "status": 204, "body": null }));
        } else {
            let body = super::api::respond(
                core,
                &event.auth,
                &event.request,
                &event.collection,
                Value::Object(event.record),
                &query,
            )
            .await?;
            results.push(json!({ "status": 200, "body": body }));
        }
    }
    Ok(Value::Array(results))
}

/// A request of the batch that went through.
struct Done {
    query: HashMap<String, String>,
    event: RecordRequestEvent,
    deleted: bool,
}

/// Runs one request of the batch: its request hooks, with the write (their `next`) in the
/// batch's transaction.
async fn run_request(
    core: &Arc<Core>,
    registry: &Arc<Registry>,
    auth: &RequestAuth,
    tx: &TxHandle,
    request: SubRequest,
    from_form: bool,
) -> Result<Done, ApiError> {
    let SubRequest {
        method,
        collection: c,
        action,
        query,
        headers,
        mut body,
        uploads,
    } = request;
    if from_form {
        // multipart batches carry JSON field values as text
        form_values(&c, &mut body);
    }
    // an upsert updates the record with the body's id, if it exists
    let action = match action {
        Action::Upsert => {
            let id = body
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let exists = !id.is_empty() && {
                let (c, id) = (c.clone(), id.clone());
                tx.run(move |tx, _| {
                    Box::pin(async move {
                        store::find_by_id(tx, &c, &id)
                            .await
                            .map(|found| found.is_some())
                            .map_err(|e| ApiError::internal(e.into()))
                    })
                })
                .await?
            };
            if exists {
                Action::Update(id)
            } else {
                Action::Create
            }
        }
        other => other,
    };
    let request = Arc::new(super::api::request_info(
        &method,
        &query,
        &headers,
        Some(&body),
        auth,
        registry,
    ));
    let hooks = &core.hooks;
    let (hook, rule) = match &action {
        Action::Create => (&hooks.on_record_create_request, RuleAction::Create),
        Action::Update(_) => (&hooks.on_record_update_request, RuleAction::Update),
        Action::Delete(_) => (&hooks.on_record_delete_request, RuleAction::Delete),
        Action::Upsert => unreachable!("resolved above"),
    };
    let target = match &action {
        Action::Update(id) | Action::Delete(id) => Some(id.clone()),
        _ => None,
    };
    // as in single requests, hooks see the current record (with an update's data applied)
    let mut current = None;
    let record = match &target {
        Some(id) if hook.has_handlers(Some(&c)) => {
            super::api::ensure_allowed(&c, rule, auth)?;
            let (c, id, auth, request) = (c.clone(), id.clone(), auth.clone(), request.clone());
            let found = tx
                .run(move |tx, _| {
                    Box::pin(async move {
                        super::api::current_in(tx, &c, &id, rule, &auth, &request).await
                    })
                })
                .await?;
            current = Some(found.clone());
            let mut record = found;
            if rule == RuleAction::Update {
                record.extend(body);
            }
            record
        }
        _ => body,
    };
    let event = RecordRequestEvent {
        core: core.clone(),
        collection: c.clone(),
        record,
        auth: auth.clone(),
        request,
        response: None,
    };
    let (registry, tx) = (registry.clone(), tx.clone());
    let deleted = matches!(action, Action::Delete(_));
    let event = hook
        .trigger(Some(&c), event, move |mut e| async move {
            let mut body = std::mem::take(&mut e.record);
            if let Some(current) = &current {
                if rule == RuleAction::Update {
                    // only what the request or the hooks changed
                    body.retain(|k, v| current.get(k) != Some(v));
                }
            }
            let (core, auth, request, c) = (
                e.core.clone(),
                e.auth.clone(),
                e.request.clone(),
                e.collection.clone(),
            );
            let record = tx
                .run(move |tx, effects| {
                    Box::pin(async move {
                        let ctx = WriteContext {
                            core: &core,
                            registry: &registry,
                            auth: &auth,
                            request: &request,
                        };
                        match action {
                            Action::Create => create_in(&ctx, tx, &c, body, uploads, effects)
                                .await
                                .map(Some),
                            Action::Update(id) => {
                                update_in(&ctx, tx, &c, &id, body, uploads, effects)
                                    .await
                                    .map(Some)
                            }
                            Action::Delete(id) => {
                                delete_in(&ctx, tx, &c, &id, effects).await.map(|_| None)
                            }
                            Action::Upsert => unreachable!("resolved above"),
                        }
                    })
                })
                .await?;
            e.record = match record {
                Some(record) => super::api::into_map(record),
                None => current.unwrap_or_default(),
            };
            Ok(e)
        })
        .await?;
    Ok(Done {
        query,
        event,
        deleted,
    })
}

/// A hook's own response as a batch result.
async fn response_json(response: axum::response::Response) -> Value {
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_BODY as usize)
        .await
        .unwrap_or_default();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    json!({ "status": status, "body": body })
}

/// Parses JSON fields sent as text (multipart batches), like single requests do.
fn form_values(c: &Collection, body: &mut Map<String, Value>) {
    for field in &c.fields {
        if let crate::collections::model::FieldKind::Json(_) = field.kind {
            if let Some(Value::String(s)) = body.get(&field.name) {
                if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                    body.insert(field.name.clone(), parsed);
                }
            }
        }
    }
}
