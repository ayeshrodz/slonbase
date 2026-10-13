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
    if let Err(err) = core.storage().delete_prefix(&c.id).await {
        tracing::warn!(error = %format!("{err:#}"), "failed to delete the collection's files");
    }
    tracing::info!(collection = %c.name, "deleted collection");

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/collections/meta/scaffolds`: the default definition of each collection type
/// (used by the dashboard for new collections).
pub async fn scaffolds(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    app.core()?;
    auth.require_superuser()?;
    let scaffold = |kind: &str| {
        let mut c =
            parse(Map::from_iter([("type".to_string(), json!(kind))])).expect("valid scaffold");
        // the only errors are about the empty name
        let _ = validate::prepare(&mut c, None, &Registry::default());
        c.id.clear();
        c.name.clear();
        to_json(&c)
    };
    Ok(Json(json!({
        "base": scaffold("base"),
        "auth": scaffold("auth"),
        // view collections aren't supported yet; creating one fails with an error
        "view": {
            "id": "", "name": "", "type": "view", "system": false, "fields": [], "indexes": [],
            "listRule": null, "viewRule": null, "createRule": null, "updateRule": null,
            "deleteRule": null, "viewQuery": "", "created": "", "updated": "",
        },
    })))
}

/// `GET /api/collections/meta/oauth2-providers`: the supported OAuth2 providers.
pub async fn oauth2_providers(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    app.core()?;
    auth.require_superuser()?;
    static PROVIDERS: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../assets/oauth2-providers.json"))
            .expect("valid providers file")
    });
    Ok(Json(PROVIDERS.clone()))
}

/// `PUT /api/collections/import` (`{"collections": [...], "deleteMissing": bool}`): creates
/// and updates collections (matched by id, then name) in one transaction, and with
/// `deleteMissing` deletes the non-system collections that aren't in the list.
pub async fn import(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let failed = "Failed to import collections.";
    let delete_missing = body
        .get("deleteMissing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let items = match body.get("collections") {
        Some(Value::Array(items)) if !items.is_empty() => items.clone(),
        _ => {
            let mut errors = crate::validation::Errors::new();
            errors.add("collections", "validation_required", "Cannot be blank.");
            return Err(ApiError::invalid_data(failed, &errors));
        }
    };

    let _guard = core.schema_lock.lock().await;
    let registry = fresh_registry(&core).await?;

    // the imported definitions, merged over the stored ones they match
    let mut pairs: Vec<(Option<Arc<Collection>>, Collection)> = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        let Value::Object(map) = item else {
            return Err(ApiError::bad_request(format!(
                "{failed} Collection {i} must be an object."
            )));
        };
        let id = map.get("id").and_then(Value::as_str).unwrap_or_default();
        let name = map.get("name").and_then(Value::as_str).unwrap_or_default();
        let old = registry
            .by_id(id)
            .or_else(|| {
                registry
                    .get(name)
                    .filter(|_| id.is_empty() || registry.by_id(id).is_none())
            })
            .cloned();
        let mut merged = match &old {
            Some(old) => match serde_json::to_value(old.as_ref()).expect("serializes") {
                Value::Object(m) => m,
                _ => unreachable!(),
            },
            None => Map::new(),
        };
        merged.extend(map);
        let mut c = parse(merged).map_err(|e| {
            ApiError::bad_request(format!("{failed} Collection {i}: {}", e.message))
        })?;
        match &old {
            Some(old) => c.id = old.id.clone(),
            // new collections get their ids now, so others can be validated against them
            None if c.id.is_empty() => {
                c.id = format!("pbc_{}", crate::ids::random_id(10));
            }
            None => {}
        }
        pairs.push((old, c));
    }

    // validate everything against the registry as it will be after the import
    let imported_ids: Vec<String> = pairs.iter().map(|(_, c)| c.id.clone()).collect();
    let deleted: Vec<Arc<Collection>> = if delete_missing {
        registry
            .all()
            .iter()
            .filter(|c| !c.system && !imported_ids.contains(&c.id))
            .cloned()
            .collect()
    } else {
        vec![]
    };
    let mut target: Vec<Collection> = registry
        .all()
        .iter()
        .filter(|c| !deleted.iter().any(|d| d.id == c.id))
        .filter(|c| !imported_ids.contains(&c.id))
        .map(|c| c.as_ref().clone())
        .collect();
    target.extend(pairs.iter().map(|(_, c)| c.clone()));
    let mut errors = crate::validation::Errors::new();
    for (i, (old, c)) in pairs.iter_mut().enumerate() {
        // checked against every other collection (its own id and name aren't taken)
        let others = Registry::new(target.iter().filter(|o| o.id != c.id).cloned().collect());
        errors.extend_prefixed(
            &format!("collections.{i}"),
            validate::prepare(c, old.as_deref(), &others),
        );
    }
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }

    let mut changes: Vec<Change> = deleted.iter().map(|c| Change::Delete(c)).collect();
    for (old, new) in &pairs {
        changes.push(match old {
            Some(old) if old.as_ref() == new => continue,
            Some(old) => Change::Update { old, new },
            None => Change::Create(new),
        });
    }
    let registry = store::apply_all(&core.pool, changes)
        .await
        .map_err(|e| write_error(e, failed))?;
    core.set_registry(registry);
    for c in &deleted {
        if let Err(err) = core.storage().delete_prefix(&c.id).await {
            tracing::warn!(error = %format!("{err:#}"), "failed to delete the collection's files");
        }
    }
    tracing::info!(
        imported = pairs.len(),
        deleted = deleted.len(),
        "imported collections"
    );
    Ok(StatusCode::NO_CONTENT.into_response())
}
