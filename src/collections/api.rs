//! The collections REST API (`/api/collections`), superusers only.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use super::model::Collection;
use super::store::{self, Change, Registry, WriteError};
use super::validate;
use crate::app::App;
use crate::auth::RequestAuth;
use crate::core::Core;
use crate::error::ApiError;
use crate::extract::JsonObject;

fn not_found() -> ApiError {
    ApiError::not_found()
}

fn write_error(err: WriteError, message: &str) -> ApiError {
    match err {
        WriteError::Conflict(detail) => ApiError::bad_request(format!("{message} {detail}.")),
        WriteError::Other(e) => ApiError::internal(e),
    }
}

/// Reloads the registry under the schema lock, so validation sees changes
/// made by other instances.
async fn fresh_registry(core: &Core) -> Result<Registry, ApiError> {
    let registry = store::load_from_pool(&core.pool)
        .await
        .map_err(ApiError::internal)?;
    core.set_registry(registry.clone());
    Ok(registry)
}

/// A collection as returned by the API: OAuth2 client secrets are left out.
pub fn to_json(c: &Collection) -> Value {
    let mut value = serde_json::to_value(c).expect("serializes");
    if let Some(Value::Array(providers)) = value.pointer_mut("/oauth2/providers") {
        for provider in providers {
            provider["clientSecret"] = Value::String(String::new());
        }
    }
    value
}

fn parse(body: Map<String, Value>) -> Result<Collection, ApiError> {
    serde_json::from_value(Value::Object(body))
        .map_err(|e| ApiError::bad_request(format!("Invalid collection data: {e}.")))
}

/// `GET /api/collections`
pub async fn list(
    State(app): State<Arc<App>>,
    Query(query): Query<HashMap<String, String>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;

    let registry = core.registry();
    let all = registry.all();
    let page = query
        .get("page")
        .and_then(|p| p.parse::<usize>().ok())
        .filter(|p| *p >= 1)
        .unwrap_or(1);
    let per_page = query
        .get("perPage")
        .and_then(|p| p.parse::<usize>().ok())
        .filter(|p| *p >= 1)
        .unwrap_or(30)
        .min(1000);
    let items: Vec<Value> = all
        .iter()
        .skip((page - 1).saturating_mul(per_page))
        .take(per_page)
        .map(|c| to_json(c))
        .collect();

    Ok(Json(json!({
        "page": page,
        "perPage": per_page,
        "totalItems": all.len(),
        "totalPages": all.len().div_ceil(per_page),
        "items": items,
    })))
}

/// `GET /api/collections/{collection}`
pub async fn view(
    State(app): State<Arc<App>>,
    Path(id_or_name): Path<String>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let registry = core.registry();
    let c = registry.get(&id_or_name).ok_or_else(not_found)?;
    Ok(Json(to_json(c)))
}

/// `POST /api/collections`
pub async fn create(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let failed = "Failed to create collection.";

    let mut c = parse(body)?;
    let _guard = core.schema_lock.lock().await;
    let registry = fresh_registry(&core).await?;

    let errors = validate::prepare(&mut c, None, &registry);
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }
    let registry = store::apply(&core.pool, Change::Create(&c))
        .await
        .map_err(|e| write_error(e, failed))?;
    let saved = registry.by_id(&c.id).cloned().ok_or_else(not_found)?;
    core.set_registry(registry);
    tracing::info!(collection = %saved.name, "created collection");

    Ok(Json(to_json(&saved)))
}

/// `PATCH /api/collections/{collection}`. Top-level keys in the body replace the stored ones.
pub async fn update(
    State(app): State<Arc<App>>,
    Path(id_or_name): Path<String>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let failed = "Failed to update collection.";

    let _guard = core.schema_lock.lock().await;
    let registry = fresh_registry(&core).await?;
    let old = registry.get(&id_or_name).cloned().ok_or_else(not_found)?;

    let mut merged = match serde_json::to_value(old.as_ref()).expect("serializes") {
        Value::Object(map) => map,
        _ => unreachable!(),
    };
    merged.extend(body);
    let mut new = parse(merged)?;

    let errors = validate::prepare(&mut new, Some(&old), &registry);
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }
    let registry = store::apply(
        &core.pool,
        Change::Update {
            old: &old,
            new: &new,
        },
    )
    .await
    .map_err(|e| write_error(e, failed))?;
    let saved = registry.by_id(&new.id).cloned().ok_or_else(not_found)?;
    core.set_registry(registry);
    tracing::info!(collection = %saved.name, "updated collection");

    Ok(Json(to_json(&saved)))
}

/// `DELETE /api/collections/{collection}`
pub async fn delete(
    State(app): State<Arc<App>>,
    Path(id_or_name): Path<String>,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let failed = "Failed to delete collection.";

    let _guard = core.schema_lock.lock().await;
    let registry = fresh_registry(&core).await?;
    let c = registry.get(&id_or_name).cloned().ok_or_else(not_found)?;
    if c.system {
        return Err(ApiError::bad_request(
            "System collections can't be deleted.",
        ));
    }
    let referencing: Vec<String> = registry
        .references_to(&c.id)
        .into_iter()
        .filter(|(other, _)| other.id != c.id)
        .map(|(other, field)| format!("{}.{field}", other.name))
        .collect();
    if !referencing.is_empty() {
        return Err(ApiError::bad_request(format!(
            "The collection is referenced by other collections ({}); remove those relation fields first.",
            referencing.join(", ")
        )));
    }

    let registry = store::apply(&core.pool, Change::Delete(&c))
        .await
        .map_err(|e| write_error(e, failed))?;
    core.set_registry(registry);
    if let Err(err) = core.storage.delete_prefix(&c.id).await {
        tracing::warn!(error = %format!("{err:#}"), "failed to delete the collection's files");
    }
    tracing::info!(collection = %c.name, "deleted collection");

    Ok(StatusCode::NO_CONTENT.into_response())
}
