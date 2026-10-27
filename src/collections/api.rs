//! The collections REST API (`/api/collections`), superusers only.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use super::model::Collection;
use super::store::{self, Change, Registry, WriteError};
use super::validate;
use crate::app::App;
use crate::auth::RequestAuth;
use crate::core::Core;
use crate::db::TextQuery;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::hooks::{self, DataEvent};

fn not_found() -> ApiError {
    ApiError::not_found()
}

fn write_error(err: WriteError, message: &str) -> ApiError {
    match err {
        WriteError::Conflict(detail) => ApiError::bad_request(format!("{message} {detail}.")),
        WriteError::Other(e) => ApiError::internal(e),
    }
}

/// Fills a view collection's fields from its query (keeping the options of fields with the
/// same names). Changing the query needs the SQL console to be enabled, since the query
/// runs with the database role's privileges.
async fn infer_view(
    core: &Core,
    c: &mut Collection,
    old: Option<&Collection>,
    errors_prefix: &str,
    failed: &str,
) -> Result<(), ApiError> {
    if !c.is_view() {
        return Ok(());
    }
    let query = c.view_query.clone().unwrap_or_default();
    let changed = old.is_none_or(|o| o.view_query.as_deref() != Some(query.as_str()));
    if changed && !core.sql_console {
        return Err(ApiError::forbidden(
            "View queries run SQL with the database role's privileges, so creating or changing \
             them needs the SQL console: start slonbase with --sql-console.",
        ));
    }
    let mut client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    // inspected in a transaction that's rolled back, with collections on the search path
    let tx = client
        .transaction()
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    tx.batch_execute(super::view::SEARCH_PATH)
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    let previous = if c.fields.is_empty() {
        old.map(|o| o.fields.clone()).unwrap_or_default()
    } else {
        c.fields.clone()
    };
    match super::view::infer_fields(&tx, &query, &previous).await {
        Ok(fields) => {
            c.fields = fields;
            Ok(())
        }
        Err(message) => {
            let mut errors = crate::validation::Errors::new();
            errors.invalid(format!("{errors_prefix}viewQuery"), message);
            Err(ApiError::invalid_data(failed, &errors))
        }
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
    headers: HeaderMap,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;

    let registry = core.registry();
    let all = filtered(&core, registry.all(), &query, &auth).await?;
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
    let result = json!({
        "page": page,
        "perPage": per_page,
        "totalItems": all.len(),
        "totalPages": all.len().div_ceil(per_page),
        "items": items,
    });
    let request = request_info(&core, &auth, &Method::GET, &query, &headers, None);
    request_hook(
        &core,
        |h| &h.on_collections_list_request,
        auth,
        request,
        hooks::data([("collections", result["items"].clone()), ("result", result)]),
        |mut e| async move {
            // like PocketBase, the response is `result` (whose items are the collections)
            e.response = Some(Json(e.data["result"].clone()).into_response());
            Ok(e)
        },
    )
    .await
}

/// The request as the collection request hooks see it.
fn request_info(
    core: &Core,
    auth: &RequestAuth,
    method: &Method,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    body: Option<&Map<String, Value>>,
) -> Arc<crate::filter::RequestInfo> {
    Arc::new(crate::records::api::request_info(
        method,
        query,
        headers,
        body,
        auth,
        &core.registry(),
    ))
}

/// Runs a collection request hook around `action`, which answers the request (sets
/// `response`). A handler that doesn't call `next` answers with its own response.
async fn request_hook<F, Fut>(
    core: &Arc<Core>,
    pick: fn(&hooks::Hooks) -> &hooks::Hook<DataEvent>,
    auth: RequestAuth,
    request: Arc<crate::filter::RequestInfo>,
    data: Map<String, Value>,
    action: F,
) -> Result<Response, ApiError>
where
    F: FnOnce(DataEvent) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<DataEvent, ApiError>> + Send + 'static,
{
    let e = DataEvent {
        core: core.clone(),
        collection: None,
        auth,
        request: Some(request),
        data,
        response: None,
    };
    let e = pick(&core.hooks).trigger(None, e, action).await?;
    Ok(e.response.unwrap_or_else(|| StatusCode::OK.into_response()))
}

/// A collection in an event's `data` (as the handlers left it).
fn object_of(value: &Value) -> Result<Map<String, Value>, ApiError> {
    match value {
        Value::Object(map) => Ok(map.clone()),
        _ => Err(ApiError::bad_request("The collection must be an object.")),
    }
}

/// The columns `filter` and `sort` can use on collections.
const META_COLUMNS: [(&str, &str, &str); 11] = [
    ("id", "text", "text"),
    ("name", "text", "text"),
    ("type", "text", "text"),
    ("system", "bool", "boolean"),
    ("listRule", "text", "text"),
    ("viewRule", "text", "text"),
    ("createRule", "text", "text"),
    ("updateRule", "text", "text"),
    ("deleteRule", "text", "text"),
    ("created", "date", "timestamptz"),
    ("updated", "date", "timestamptz"),
];

/// The collections matching the `filter` query parameter, in the `sort` order (by name
/// without one). The filter language runs in PostgreSQL over the collections' metadata.
async fn filtered(
    core: &Core,
    all: &[Arc<Collection>],
    query: &HashMap<String, String>,
    auth: &RequestAuth,
) -> Result<Vec<Arc<Collection>>, ApiError> {
    let filter = query.get("filter").map(|f| f.trim()).unwrap_or_default();
    let sort = query.get("sort").map(|s| s.trim()).unwrap_or_default();
    if filter.is_empty() && sort.is_empty() {
        return Ok(all.to_vec());
    }
    let fields: Vec<Value> = META_COLUMNS
        .iter()
        .map(|(name, kind, _)| json!({ "name": name, "type": kind }))
        .collect();
    let meta = parse(
        json!({ "name": "collections", "type": "base", "fields": fields })
            .as_object()
            .cloned()
            .unwrap_or_default(),
    )?;
    let alias = "_c";
    let mut request = crate::filter::RequestInfo::new("GET");
    request.auth = auth.info();
    let scope = crate::filter::Scope {
        collection: &meta,
        alias,
        request: &request,
        allow_hidden: true,
    };
    let mut params = crate::filter::Params::default();
    let invalid = |e: crate::filter::CompileError| {
        ApiError::bad_request(format!("Invalid filter or sort: {}", e.0))
    };
    let where_sql = if filter.is_empty() {
        "TRUE".to_string()
    } else {
        crate::filter::to_sql(filter, &scope, &mut params).map_err(invalid)?
    };
    let mut order = if sort.is_empty() {
        Vec::new()
    } else {
        crate::filter::compile_sort(sort, &meta, alias, true).map_err(invalid)?
    };
    order.push(format!("{alias}.\"name\""));
    let rows: Vec<Value> = all
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let v = to_json(c);
            let mut row = Map::new();
            // the sort's tie-breaker, as in records tables
            row.insert("_seq".into(), json!(i));
            for (name, _, _) in META_COLUMNS {
                row.insert(
                    name.to_string(),
                    v.get(name).cloned().unwrap_or(Value::Null),
                );
            }
            Value::Object(row)
        })
        .collect();
    let data = params.push(Value::Array(rows).to_string());
    let columns: Vec<String> = META_COLUMNS
        .iter()
        .map(|(name, _, sql)| format!("{} {sql}", crate::filter::quote_ident(name)))
        .chain(std::iter::once("_seq bigint".to_string()))
        .collect();
    let sql = format!(
        "SELECT {alias}.id FROM jsonb_to_recordset(({data})::jsonb) AS {alias}({}) WHERE {where_sql} ORDER BY {}",
        columns.join(", "),
        order.join(", ")
    );
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let ids = client.query_text(&sql, &params).await.map_err(|e| {
        let detail = e
            .as_db_error()
            .map(|db| db.message().to_string())
            .unwrap_or_else(|| e.to_string());
        ApiError::bad_request(format!("Invalid filter or sort: {detail}"))
    })?;
    let by_id: HashMap<&str, &Arc<Collection>> = all.iter().map(|c| (c.id.as_str(), c)).collect();
    Ok(ids
        .iter()
        .filter_map(|row| {
            by_id
                .get(row.get::<_, String>(0).as_str())
                .map(|c| (*c).clone())
        })
        .collect())
}

/// `GET /api/collections/{collection}`
pub async fn view(
    State(app): State<Arc<App>>,
    Path(id_or_name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let registry = core.registry();
    let c = registry.get(&id_or_name).ok_or_else(not_found)?;
    let request = request_info(&core, &auth, &Method::GET, &query, &headers, None);
    request_hook(
        &core,
        |h| &h.on_collection_view_request,
        auth,
        request,
        hooks::data([("collection", to_json(c))]),
        |mut e| async move {
            e.response = Some(Json(e.data["collection"].clone()).into_response());
            Ok(e)
        },
    )
    .await
}

/// Runs a collection hook on `c` before its change; returns the collection the handlers
/// left (or `c` itself without handlers).
async fn before_hook(
    core: &Arc<Core>,
    hook: &crate::hooks::Hook<crate::hooks::CollectionEvent>,
    c: Collection,
) -> Result<Collection, ApiError> {
    if !hook.has_handlers(Some(&c)) {
        return Ok(c);
    }
    let collection = match serde_json::to_value(&c).expect("serializes") {
        Value::Object(map) => map,
        _ => unreachable!(),
    };
    let e = crate::hooks::CollectionEvent {
        core: core.clone(),
        collection,
    };
    let e = hook.trigger(Some(&c), e, |e| async { Ok(e) }).await?;
    let mut changed = parse(e.collection)?;
    // the identity stays
    changed.id = c.id.clone();
    Ok(changed)
}

/// Runs a collection execute hook (`on_collection_create_execute`, ...) around `apply`, the
/// schema change: handlers' `next` applies it. A handler that doesn't call `next` cancels it.
async fn execute_hook<T, F, Fut>(
    core: &Arc<Core>,
    hook: &crate::hooks::Hook<crate::hooks::CollectionEvent>,
    c: &Collection,
    apply: F,
) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    if !hook.has_handlers(Some(c)) {
        return apply().await;
    }
    let collection = match serde_json::to_value(c).expect("serializes") {
        Value::Object(map) => map,
        _ => unreachable!(),
    };
    let e = crate::hooks::CollectionEvent {
        core: core.clone(),
        collection,
    };
    let outcome = Arc::new(std::sync::Mutex::new(None));
    let slot = outcome.clone();
    let hooked = hook
        .trigger(Some(c), e, move |e| async move {
            let result = apply().await;
            let failed = result.as_ref().err().cloned();
            *slot.lock().expect("outcome lock") = Some(result);
            match failed {
                Some(err) => Err(err),
                None => Ok(e),
            }
        })
        .await;
    match outcome.lock().expect("outcome lock").take() {
        // a failed change fails, whatever the handlers did with the error
        Some(Err(err)) => Err(err),
        Some(Ok(value)) => hooked.map(|_| value),
        None => {
            hooked?;
            Err(ApiError::bad_request("The change was cancelled by a hook."))
        }
    }
}

/// Reports a failed collection change to its after-error hook, in the background (not when
/// the collection isn't there).
fn after_error(
    core: &Arc<Core>,
    pick: fn(&crate::hooks::Hooks) -> &crate::hooks::Hook<crate::hooks::CollectionErrorEvent>,
    c: Option<&Collection>,
    err: &ApiError,
) {
    let Some(c) = c else {
        return;
    };
    if err.status == StatusCode::NOT_FOUND || !pick(&core.hooks).has_handlers(Some(c)) {
        return;
    }
    let (core, c, error) = (core.clone(), c.clone(), err.clone());
    tokio::spawn(async move {
        let collection = match to_json(&c) {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let e = crate::hooks::CollectionErrorEvent {
            core: core.clone(),
            collection,
            error,
        };
        if let Err(err) = pick(&core.hooks)
            .trigger(Some(&c), e, |e| async { Ok(e) })
            .await
        {
            tracing::error!(collection = %c.name, error = %err.message, "a collection hook failed");
        }
    });
}

/// Runs a collection hook after a change, in the background.
fn after_hook(
    core: &Arc<Core>,
    pick: fn(&crate::hooks::Hooks) -> &crate::hooks::Hook<crate::hooks::CollectionEvent>,
    c: &Collection,
) {
    if !pick(&core.hooks).has_handlers(Some(c)) {
        return;
    }
    let (core, c) = (core.clone(), c.clone());
    tokio::spawn(async move {
        let collection = match to_json(&c) {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let e = crate::hooks::CollectionEvent {
            core: core.clone(),
            collection,
        };
        if let Err(err) = pick(&core.hooks)
            .trigger(Some(&c), e, |e| async { Ok(e) })
            .await
        {
            tracing::error!(collection = %c.name, error = %err.message, "a collection hook failed");
        }
    });
}

/// `POST /api/collections`
pub async fn create(
    State(app): State<Arc<App>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    // the collection as submitted, with its defaults (like PocketBase's); unmasked, as it's
    // what gets saved
    let submitted = serde_json::to_value(parse(body.clone())?).expect("serializes");
    let request = request_info(&core, &auth, &Method::POST, &query, &headers, Some(&body));
    request_hook(
        &core,
        |h| &h.on_collection_create_request,
        auth,
        request,
        hooks::data([("collection", submitted)]),
        |mut e| async move {
            let saved = create_collection(&e.core, object_of(&e.data["collection"])?).await?;
            let json = to_json(&saved);
            e.data.insert("collection".into(), json.clone());
            e.response = Some(Json(json).into_response());
            Ok(e)
        },
    )
    .await
}

/// Creates a collection from its JSON definition, through the collection hooks.
async fn create_collection(
    core: &Arc<Core>,
    body: Map<String, Value>,
) -> Result<Arc<Collection>, ApiError> {
    let c = parse(body)?;
    let attempted = c.clone();
    let result = create_checked(core, c).await;
    if let Err(err) = &result {
        after_error(
            core,
            |h| &h.on_collection_after_create_error,
            Some(&attempted),
            err,
        );
    }
    result
}

async fn create_checked(core: &Arc<Core>, c: Collection) -> Result<Arc<Collection>, ApiError> {
    let failed = "Failed to create collection.";
    let _guard = core.schema_lock.lock().await;
    let registry = fresh_registry(core).await?;

    let mut c = before_hook(core, &core.hooks.on_collection_create, c).await?;
    infer_view(core, &mut c, None, "", failed).await?;
    let errors = validate::prepare(&mut c, None, &registry);
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }
    // changes are ignored: the collection is validated
    before_hook(core, &core.hooks.on_collection_validate, c.clone()).await?;
    let hook = &core.hooks.on_collection_create_execute;
    let (pool, new) = (core.pool.clone(), c.clone());
    let registry = execute_hook(core, hook, &c, move || async move {
        store::apply(&pool, Change::Create(&new))
            .await
            .map(|applied| applied.registry)
            .map_err(|e| write_error(e, failed))
    })
    .await?;
    let saved = registry.by_id(&c.id).cloned().ok_or_else(not_found)?;
    core.set_registry(registry);
    tracing::info!(collection = %saved.name, "created collection");
    after_hook(core, |h| &h.on_collection_after_create_success, &saved);
    #[cfg(feature = "js")]
    crate::js::migrate::automigrate(core, vec![crate::js::migrate::Change::Created(&saved)]).await;
    Ok(saved)
}

/// `PATCH /api/collections/{collection}`. Top-level keys in the body replace the stored ones.
pub async fn update(
    State(app): State<Arc<App>>,
    Path(id_or_name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let old = core
        .registry()
        .get(&id_or_name)
        .cloned()
        .ok_or_else(not_found)?;
    let mut merged = match serde_json::to_value(old.as_ref()).expect("serializes") {
        Value::Object(map) => map,
        _ => unreachable!(),
    };
    merged.extend(body.clone());
    // the collection with the request applied (like PocketBase's), unmasked
    let submitted = serde_json::to_value(parse(merged)?).expect("serializes");
    let request = request_info(&core, &auth, &Method::PATCH, &query, &headers, Some(&body));
    let id = old.id.clone();
    request_hook(
        &core,
        |h| &h.on_collection_update_request,
        auth,
        request,
        hooks::data([("collection", submitted)]),
        |mut e| async move {
            let saved = update_collection(&e.core, &id, object_of(&e.data["collection"])?).await?;
            let json = to_json(&saved);
            e.data.insert("collection".into(), json.clone());
            e.response = Some(Json(json).into_response());
            Ok(e)
        },
    )
    .await
}

/// Replaces the collection `id` with a definition (keys missing from it keep their stored
/// values), through the collection hooks.
async fn update_collection(
    core: &Arc<Core>,
    id: &str,
    definition: Map<String, Value>,
) -> Result<Arc<Collection>, ApiError> {
    let mut attempted = core.registry().by_id(id).map(|c| c.as_ref().clone());
    let result = update_checked(core, id, definition, &mut attempted).await;
    if let Err(err) = &result {
        after_error(
            core,
            |h| &h.on_collection_after_update_error,
            attempted.as_ref(),
            err,
        );
    }
    result
}

/// [`update_collection`]; `attempted` becomes the collection as it would be saved.
async fn update_checked(
    core: &Arc<Core>,
    id: &str,
    definition: Map<String, Value>,
    attempted: &mut Option<Collection>,
) -> Result<Arc<Collection>, ApiError> {
    let failed = "Failed to update collection.";
    let _guard = core.schema_lock.lock().await;
    let registry = fresh_registry(core).await?;
    let old = registry.by_id(id).cloned().ok_or_else(not_found)?;

    let mut merged = match serde_json::to_value(old.as_ref()).expect("serializes") {
        Value::Object(map) => map,
        _ => unreachable!(),
    };
    merged.extend(definition);
    // the identity stays
    merged.insert("id".into(), Value::String(old.id.clone()));
    let new = parse(merged)?;
    *attempted = Some(new.clone());
    let mut new = before_hook(core, &core.hooks.on_collection_update, new).await?;
    *attempted = Some(new.clone());

    infer_view(core, &mut new, Some(&old), "", failed).await?;
    let errors = validate::prepare(&mut new, Some(&old), &registry);
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }
    before_hook(core, &core.hooks.on_collection_validate, new.clone()).await?;
    let hook = &core.hooks.on_collection_update_execute;
    let (pool, stored, changed) = (core.pool.clone(), old.clone(), new.clone());
    let applied = execute_hook(core, hook, &new, move || async move {
        let change = Change::Update {
            old: &stored,
            new: &changed,
        };
        store::apply(&pool, change)
            .await
            .map_err(|e| write_error(e, failed))
    })
    .await?;
    let saved = applied
        .registry
        .by_id(&new.id)
        .cloned()
        .ok_or_else(not_found)?;
    core.set_registry(applied.registry);
    tracing::info!(collection = %saved.name, "updated collection");
    after_hook(core, |h| &h.on_collection_after_update_success, &saved);
    crate::records::files::remove_field_files(core, &applied.removed_files).await;
    #[cfg(feature = "js")]
    if saved.as_ref() != old.as_ref() {
        crate::js::migrate::automigrate(
            core,
            vec![crate::js::migrate::Change::Updated {
                old: &old,
                new: &saved,
            }],
        )
        .await;
    }
    Ok(saved)
}

/// `DELETE /api/collections/{collection}`
pub async fn delete(
    State(app): State<Arc<App>>,
    Path(id_or_name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let c = core
        .registry()
        .get(&id_or_name)
        .cloned()
        .ok_or_else(not_found)?;
    let request = request_info(&core, &auth, &Method::DELETE, &query, &headers, None);
    request_hook(
        &core,
        |h| &h.on_collection_delete_request,
        auth,
        request,
        hooks::data([("collection", to_json(&c))]),
        |mut e| async move {
            delete_collection(&e.core, &c.id).await?;
            #[cfg(feature = "js")]
            crate::js::migrate::automigrate(&e.core, vec![crate::js::migrate::Change::Deleted(&c)])
                .await;
            e.response = Some(StatusCode::NO_CONTENT.into_response());
            Ok(e)
        },
    )
    .await
}

/// Deletes a collection with its records and files (not system collections, nor ones
/// other collections refer to).
pub(crate) async fn delete_collection(core: &Arc<Core>, id_or_name: &str) -> Result<(), ApiError> {
    let result = delete_checked(core, id_or_name).await;
    if let Err(err) = &result {
        let stored = core.registry().get(id_or_name).cloned();
        after_error(
            core,
            |h| &h.on_collection_after_delete_error,
            stored.as_deref(),
            err,
        );
    }
    result
}

async fn delete_checked(core: &Arc<Core>, id_or_name: &str) -> Result<(), ApiError> {
    let failed = "Failed to delete collection.";
    let _guard = core.schema_lock.lock().await;
    let registry = fresh_registry(core).await?;
    let c = registry.get(id_or_name).cloned().ok_or_else(not_found)?;
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

    // changes are ignored: the collection is deleted
    before_hook(core, &core.hooks.on_collection_delete, c.as_ref().clone()).await?;
    let hook = &core.hooks.on_collection_delete_execute;
    let (pool, deleted) = (core.pool.clone(), c.clone());
    let registry = execute_hook(core, hook, &c, move || async move {
        store::apply(&pool, Change::Delete(&deleted))
            .await
            .map(|applied| applied.registry)
            .map_err(|e| write_error(e, failed))
    })
    .await?;
    core.set_registry(registry);
    after_hook(core, |h| &h.on_collection_after_delete_success, &c);
    if let Err(err) = core.storage().delete_prefix(&c.id).await {
        tracing::warn!(error = %format!("{err:#}"), "failed to delete the collection's files");
    }
    tracing::info!(collection = %c.name, "deleted collection");
    Ok(())
}

/// Creates or updates one collection from its JSON definition (see
/// [`import_definitions`]) and returns it as saved.
#[cfg(feature = "js")]
pub(crate) async fn save_definition(
    core: &Arc<Core>,
    definition: Value,
) -> Result<Arc<Collection>, ApiError> {
    let saved = import_definitions(core, vec![definition], false).await?;
    saved.into_iter().next().ok_or_else(not_found)
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
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
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
    let request = request_info(&core, &auth, &Method::PUT, &query, &headers, Some(&body));
    request_hook(
        &core,
        |h| &h.on_collections_import_request,
        auth,
        request,
        hooks::data([
            ("collectionsData", Value::Array(items)),
            ("deleteMissing", delete_missing.into()),
        ]),
        |mut e| async move {
            let items = match &e.data["collectionsData"] {
                Value::Array(items) => items.clone(),
                _ => return Err(ApiError::bad_request("collectionsData must be a list.")),
            };
            let delete_missing = e.data["deleteMissing"].as_bool().unwrap_or(false);
            import_with_migrations(&e.core, items, delete_missing).await?;
            e.response = Some(StatusCode::NO_CONTENT.into_response());
            Ok(e)
        },
    )
    .await
}

/// [`import_definitions`], plus the automigrate files of what changed.
async fn import_with_migrations(
    core: &Arc<Core>,
    items: Vec<Value>,
    delete_missing: bool,
) -> Result<(), ApiError> {
    let before = fresh_registry(core).await?;
    let saved = import_definitions(core, items, delete_missing).await?;
    #[cfg(feature = "js")]
    {
        use crate::js::migrate::Change as Written;
        let after = core.registry();
        let mut changes: Vec<Written> = Vec::new();
        for c in &saved {
            match before.by_id(&c.id) {
                Some(old) if old.as_ref() == c.as_ref() => {}
                Some(old) => changes.push(Written::Updated { old, new: c }),
                None => changes.push(Written::Created(c)),
            }
        }
        for c in before.all() {
            if !c.system && after.by_id(&c.id).is_none() {
                changes.push(Written::Deleted(c));
            }
        }
        crate::js::migrate::automigrate(core, changes).await;
    }
    #[cfg(not(feature = "js"))]
    let _ = (before, saved);
    Ok(())
}

/// Creates or updates collections from their PocketBase JSON definitions (and deletes the
/// other non-system collections with `delete_missing`), all in one transaction: the
/// collection hooks run around [`super::tx::import`].
pub(crate) async fn import_definitions(
    core: &Arc<Core>,
    items: Vec<Value>,
    delete_missing: bool,
) -> Result<Vec<Arc<Collection>>, ApiError> {
    let failed = "Failed to import collections.";
    let _guard = core.schema_lock.lock().await;
    let registry = fresh_registry(core).await?;

    // the collection hooks see each definition merged over the stored one it matches
    let mut definitions = Vec::with_capacity(items.len());
    let mut olds: Vec<Option<Arc<Collection>>> = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        let Value::Object(mut map) = item else {
            return Err(ApiError::bad_request(format!(
                "{failed} Collection {i} must be an object."
            )));
        };
        if !super::tx::importable_system(&registry, &mut map) {
            continue;
        }
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
            None if c.id.is_empty() => {
                c.id = format!("pbc_{}", crate::ids::random_id(10));
            }
            None => {}
        }
        let hook = if old.is_some() {
            &core.hooks.on_collection_update
        } else {
            &core.hooks.on_collection_create
        };
        let c = before_hook(core, hook, c).await?;
        if c.is_view()
            && !core.sql_console
            && old.as_ref().is_none_or(|o| o.view_query != c.view_query)
        {
            return Err(ApiError::forbidden(
                "View queries run SQL with the database role's privileges, so creating or changing \
                 them needs the SQL console: start slonbase with --sql-console.",
            ));
        }
        definitions.push(serde_json::to_value(&c).expect("serializes"));
        olds.push(old);
    }
    if delete_missing {
        let ids: Vec<&str> = definitions
            .iter()
            .filter_map(|d| d.get("id").and_then(Value::as_str))
            .collect();
        for c in registry
            .all()
            .iter()
            .filter(|c| !c.system && !ids.contains(&c.id.as_str()))
        {
            before_hook(core, &core.hooks.on_collection_delete, c.as_ref().clone()).await?;
        }
    }

    let mut client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let tx = client
        .transaction()
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    let imported = super::tx::import(&tx, &registry, definitions, delete_missing, false).await?;
    tx.commit()
        .await
        .map_err(|e| ApiError::internal(e.into()))?;

    for (old, saved) in olds.iter().zip(&imported.saved) {
        match old {
            Some(old) if old.as_ref() == saved.as_ref() => {}
            Some(_) => after_hook(core, |h| &h.on_collection_after_update_success, saved),
            None => after_hook(core, |h| &h.on_collection_after_create_success, saved),
        }
    }
    for c in &imported.deleted {
        after_hook(core, |h| &h.on_collection_after_delete_success, c);
    }
    core.set_registry(imported.registry);
    for c in &imported.deleted {
        if let Err(err) = core.storage().delete_prefix(&c.id).await {
            tracing::warn!(error = %format!("{err:#}"), "failed to delete the collection's files");
        }
    }
    crate::records::files::remove_field_files(core, &imported.removed_files).await;
    tracing::info!(
        imported = imported.saved.len(),
        deleted = imported.deleted.len(),
        "imported collections"
    );
    Ok(imported.saved)
}
