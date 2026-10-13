//! The records REST API (`/api/collections/{collection}/records`), compatible with PocketBase.
//!
//! API rules: `null` means superusers only, `""` means anyone, anything else is a filter the
//! record must match. Superusers bypass rules. List requests apply the rule as an extra
//! filter; view/update/delete answer 404 for records the rule hides; create checks the rule
//! against the inserted row inside the transaction and rolls back on failure.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use deadpool_postgres::{GenericClient, Transaction};
use serde_json::{Map, Value, json};
use tokio_postgres::Row;

use super::values::{self, Values};
use crate::app::App;
use crate::auth::RequestAuth;
use crate::collections::indexes::Index;
use crate::collections::model::{Collection, Field, FieldKind, RuleAction, Storage};
use crate::collections::schema::table_ref;
use crate::collections::store::Registry;
use crate::datetime;
use crate::db::describe_pool_error;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::filter::{Params, RequestInfo, Scope, compile_sort, quote_ident, to_sql};
use crate::validation::Errors;

const ALIAS: &str = "t";
const DEFAULT_PER_PAGE: i64 = 30;
const MAX_PER_PAGE: i64 = 1000;
const MAX_CASCADE_DEPTH: usize = 32;

fn find_collection(registry: &Registry, name: &str) -> Result<Arc<Collection>, ApiError> {
    registry
        .get(name)
        .cloned()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "Missing collection context."))
}

fn request_info(
    method: &Method,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    body: Option<&Map<String, Value>>,
) -> RequestInfo {
    let mut info = RequestInfo::new(method.as_str());
    info.query = query.clone();
    info.headers = headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str().ok().map(|v| {
                (
                    k.as_str().to_ascii_lowercase().replace('-', "_"),
                    v.to_string(),
                )
            })
        })
        .collect();
    if let Some(body) = body {
        info.body = body.clone();
    }
    info
}

/// The rule condition for an action, `None` when nothing needs checking.
fn rule_sql(
    c: &Collection,
    action: RuleAction,
    auth: &RequestAuth,
    request: &RequestInfo,
    params: &mut Params,
) -> Result<Option<String>, ApiError> {
    if auth.is_superuser() {
        return Ok(None);
    }
    match c.rule(action) {
        None => Err(ApiError::superusers_only()),
        Some(rule) if rule.trim().is_empty() => Ok(None),
        Some(rule) => {
            let scope = Scope {
                collection: c,
                alias: ALIAS,
                request,
                allow_hidden: true,
            };
            to_sql(rule, &scope, params)
                .map(Some)
                .map_err(|e| ApiError::internal(anyhow::anyhow!("invalid stored rule: {e}")))
        }
    }
}

/// Checks only that the action is allowed at all (non-null rule or superuser).
fn ensure_allowed(c: &Collection, action: RuleAction, auth: &RequestAuth) -> Result<(), ApiError> {
    if !auth.is_superuser() && c.rule(action).is_none() {
        return Err(ApiError::superusers_only());
    }
    Ok(())
}

/// Columns returned to the client, in field order.
fn visible_fields(c: &Collection, include_hidden: bool) -> Vec<&Field> {
    c.fields
        .iter()
        .filter(|f| include_hidden || !f.hidden)
        .collect()
}

fn select_list(fields: &[&Field]) -> String {
    fields
        .iter()
        .map(|f| {
            let column = format!("{ALIAS}.{}", quote_ident(&f.name));
            let expr = match f.storage() {
                Storage::Date => datetime::sql_format(&column),
                _ => column,
            };
            format!("{expr} AS {}", quote_ident(&f.name))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reads normalized values from a row selected with [`select_list`].
fn row_values(fields: &[&Field], row: &Row) -> Values {
    let mut map = Map::with_capacity(fields.len());
    for (i, f) in fields.iter().enumerate() {
        let value = match f.storage() {
            Storage::Text => Value::String(row.get(i)),
            Storage::Number => values::number(row.get(i)),
            Storage::Bool => Value::Bool(row.get(i)),
            Storage::Date => Value::String(row.get::<_, Option<String>>(i).unwrap_or_default()),
            Storage::Multi => row.get(i),
            Storage::Json => row.get::<_, Option<Value>>(i).unwrap_or(Value::Null),
        };
        map.insert(f.name.clone(), value);
    }
    map
}

fn record_json(c: &Collection, values: Values) -> Value {
    let mut out = Map::with_capacity(values.len() + 2);
    out.insert("collectionId".into(), Value::String(c.id.clone()));
    out.insert("collectionName".into(), Value::String(c.name.clone()));
    out.extend(values);
    Value::Object(out)
}

/// Applies the `fields` query parameter (`id,title`; `*` keeps everything).
fn pick_fields(record: Value, fields: Option<&String>) -> Value {
    let Some(raw) = fields.map(|f| f.trim()).filter(|f| !f.is_empty()) else {
        return record;
    };
    let wanted: HashSet<&str> = raw.split(',').map(str::trim).collect();
    if wanted.contains("*") {
        return record;
    }
    match record {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(k, _)| wanted.contains(k.as_str()))
                .collect(),
        ),
        other => other,
    }
}

fn db_error(err: tokio_postgres::Error) -> ApiError {
    ApiError::internal(err.into())
}

fn bad_filter(e: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(format!("Invalid filter parameters: {e}."))
}

/// `GET /api/collections/{collection}/records`
pub async fn list(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    method: Method,
    headers: HeaderMap,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let c = find_collection(&core.registry(), &collection)?;
    let request = request_info(&method, &query, &headers, None);
    let superuser = auth.is_superuser();

    let page = query
        .get("page")
        .and_then(|p| p.parse::<i64>().ok())
        .filter(|p| *p >= 1)
        .unwrap_or(1);
    let per_page = query
        .get("perPage")
        .and_then(|p| p.parse::<i64>().ok())
        .filter(|p| *p >= 1)
        .unwrap_or(DEFAULT_PER_PAGE)
        .min(MAX_PER_PAGE);
    let skip_total = query
        .get("skipTotal")
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "t"));
    let offset = (page - 1).saturating_mul(per_page);

    let mut params = Params::default();
    let mut conditions = Vec::new();
    if let Some(rule) = rule_sql(&c, RuleAction::List, &auth, &request, &mut params)? {
        conditions.push(rule);
    }
    if let Some(filter) = query.get("filter").filter(|f| !f.trim().is_empty()) {
        let scope = Scope {
            collection: &c,
            alias: ALIAS,
            request: &request,
            allow_hidden: superuser,
        };
        conditions.push(to_sql(filter, &scope, &mut params).map_err(bad_filter)?);
    }
    let where_sql = if conditions.is_empty() {
        "TRUE".to_string()
    } else {
        conditions.join(" AND ")
    };
    let order = compile_sort(
        query.get("sort").map(String::as_str).unwrap_or_default(),
        &c,
        ALIAS,
        superuser,
    )
    .map_err(|e| ApiError::bad_request(format!("Invalid sort parameter: {e}.")))?;

    let fields = visible_fields(&c, superuser);
    let table = table_ref(&c.name);
    let items_sql = format!(
        "SELECT {} FROM {table} {ALIAS} WHERE {where_sql} ORDER BY {} LIMIT {per_page} OFFSET {offset}",
        select_list(&fields),
        order.join(", ")
    );
    let count_sql = format!("SELECT count(*) FROM {table} {ALIAS} WHERE {where_sql}");

    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(describe_pool_error(e)))?;
    let refs = params.as_refs();
    // pipelined on the same connection
    let (rows, total) = if skip_total {
        (client.query(&items_sql, &refs).await.map_err(db_error)?, -1)
    } else {
        let (rows, count) = tokio::try_join!(
            client.query(&items_sql, &refs),
            client.query_one(&count_sql, &refs)
        )
        .map_err(db_error)?;
        (rows, count.get::<_, i64>(0))
    };

    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            pick_fields(
                record_json(&c, row_values(&fields, row)),
                query.get("fields"),
            )
        })
        .collect();
    let total_pages = if total < 0 {
        -1
    } else {
        (total + per_page - 1) / per_page
    };

    Ok(Json(json!({
        "page": page,
        "perPage": per_page,
        "totalItems": total,
        "totalPages": total_pages,
        "items": items,
    })))
}

/// `GET /api/collections/{collection}/records/{id}`
pub async fn view(
    State(app): State<Arc<App>>,
    Path((collection, id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    method: Method,
    headers: HeaderMap,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let c = find_collection(&core.registry(), &collection)?;
    let request = request_info(&method, &query, &headers, None);

    let mut params = Params::default();
    let id_param = params.push(id);
    let mut where_sql = format!("{ALIAS}.\"id\" = {id_param}");
    if let Some(rule) = rule_sql(&c, RuleAction::View, &auth, &request, &mut params)? {
        where_sql = format!("{where_sql} AND {rule}");
    }

    let fields = visible_fields(&c, auth.is_superuser());
    let sql = format!(
        "SELECT {} FROM {} {ALIAS} WHERE {where_sql}",
        select_list(&fields),
        table_ref(&c.name)
    );
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(describe_pool_error(e)))?;
    let row = client
        .query_opt(&sql, &params.as_refs())
        .await
        .map_err(db_error)?
        .ok_or_else(ApiError::not_found)?;

    Ok(Json(pick_fields(
        record_json(&c, row_values(&fields, &row)),
        query.get("fields"),
    )))
}

/// `POST /api/collections/{collection}/records`
pub async fn create(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    method: Method,
    headers: HeaderMap,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let registry = core.registry();
    let c = find_collection(&registry, &collection)?;
    ensure_allowed(&c, RuleAction::Create, &auth)?;
    let request = request_info(&method, &query, &headers, Some(&body));
    let failed = "Failed to create record.";

    let mut values = values::defaults(&c);
    let mut errors = Errors::new();
    values::apply_body(&c, &mut values, &body, true, &mut errors);
    values::finalize(&c, &mut values, true, &request.now, &mut errors);
    values::validate(&c, &values, &mut errors);
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }

    let mut client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(describe_pool_error(e)))?;
    let tx = client.transaction().await.map_err(db_error)?;
    check_relations(&tx, &registry, &c, &values, &mut errors).await?;
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }

    let mut params = Params::default();
    let mut columns = Vec::new();
    let mut placeholders = Vec::new();
    for field in &c.fields {
        let value = values
            .get(&field.name)
            .cloned()
            .unwrap_or_else(|| values::default_value(field));
        let placeholder = match values::to_param(field, &value) {
            Some(p) => params.push(p),
            None => params.push_null(),
        };
        columns.push(quote_ident(&field.name));
        placeholders.push(format!("({placeholder}){}", values::sql_cast(field)));
    }
    let fields = visible_fields(&c, auth.is_superuser());
    let sql = format!(
        "INSERT INTO {} AS {ALIAS} ({}) VALUES ({}) RETURNING {}",
        table_ref(&c.name),
        columns.join(", "),
        placeholders.join(", "),
        select_list(&fields)
    );
    let row = tx
        .query_one(&sql, &params.as_refs())
        .await
        .map_err(|e| write_error(e, &c, failed))?;

    // the create rule is checked against the inserted record
    let mut rule_params = Params::default();
    let id = values
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let id_param = rule_params.push(id);
    if let Some(rule) = rule_sql(&c, RuleAction::Create, &auth, &request, &mut rule_params)? {
        let check = format!(
            "SELECT 1 FROM {} {ALIAS} WHERE {ALIAS}.\"id\" = {id_param} AND {rule}",
            table_ref(&c.name)
        );
        if tx
            .query_opt(&check, &rule_params.as_refs())
            .await
            .map_err(db_error)?
            .is_none()
        {
            // dropping the transaction rolls it back
            return Err(ApiError::bad_request(failed));
        }
    }
    tx.commit().await.map_err(db_error)?;

    Ok(Json(record_json(&c, row_values(&fields, &row))))
}

/// `PATCH /api/collections/{collection}/records/{id}`
#[allow(clippy::too_many_arguments)]
pub async fn update(
    State(app): State<Arc<App>>,
    Path((collection, id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    method: Method,
    headers: HeaderMap,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let registry = core.registry();
    let c = find_collection(&registry, &collection)?;
    let request = request_info(&method, &query, &headers, Some(&body));
    let failed = "Failed to update record.";

    // the rule is checked against the current record (with @request.body available)
    let mut params = Params::default();
    let id_param = params.push(id.clone());
    let mut where_sql = format!("{ALIAS}.\"id\" = {id_param}");
    if let Some(rule) = rule_sql(&c, RuleAction::Update, &auth, &request, &mut params)? {
        where_sql = format!("{where_sql} AND {rule}");
    }

    let all_fields = visible_fields(&c, true);
    let mut client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(describe_pool_error(e)))?;
    let tx = client.transaction().await.map_err(db_error)?;
    let current = tx
        .query_opt(
            &format!(
                "SELECT {} FROM {} {ALIAS} WHERE {where_sql} FOR UPDATE OF {ALIAS}",
                select_list(&all_fields),
                table_ref(&c.name)
            ),
            &params.as_refs(),
        )
        .await
        .map_err(db_error)?
        .ok_or_else(ApiError::not_found)?;
    let mut values = row_values(&all_fields, &current);

    let mut errors = Errors::new();
    values::apply_body(&c, &mut values, &body, false, &mut errors);
    values::finalize(&c, &mut values, false, &request.now, &mut errors);
    values::validate(&c, &values, &mut errors);
    if errors.is_empty() {
        check_relations(&tx, &registry, &c, &values, &mut errors).await?;
    }
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }

    let mut params = Params::default();
    let mut sets = Vec::new();
    for field in c.fields.iter().filter(|f| !f.is_primary_key()) {
        let value = values
            .get(&field.name)
            .cloned()
            .unwrap_or_else(|| values::default_value(field));
        let placeholder = match values::to_param(field, &value) {
            Some(p) => params.push(p),
            None => params.push_null(),
        };
        sets.push(format!(
            "{} = ({placeholder}){}",
            quote_ident(&field.name),
            values::sql_cast(field)
        ));
    }
    let id_param = params.push(id);
    let fields = visible_fields(&c, auth.is_superuser());
    let row = if sets.is_empty() {
        current
    } else {
        let sql = format!(
            "UPDATE {} AS {ALIAS} SET {} WHERE {ALIAS}.\"id\" = {id_param} RETURNING {}",
            table_ref(&c.name),
            sets.join(", "),
            select_list(&all_fields)
        );
        tx.query_one(&sql, &params.as_refs())
            .await
            .map_err(|e| write_error(e, &c, failed))?
    };
    tx.commit().await.map_err(db_error)?;

    let mut output = row_values(&all_fields, &row);
    output.retain(|name, _| fields.iter().any(|f| &f.name == name));
    Ok(Json(record_json(&c, output)))
}

/// `DELETE /api/collections/{collection}/records/{id}`
pub async fn delete(
    State(app): State<Arc<App>>,
    Path((collection, id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    method: Method,
    headers: HeaderMap,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let registry = core.registry();
    let c = find_collection(&registry, &collection)?;
    let request = request_info(&method, &query, &headers, None);

    let mut params = Params::default();
    let id_param = params.push(id.clone());
    let mut where_sql = format!("{ALIAS}.\"id\" = {id_param}");
    if let Some(rule) = rule_sql(&c, RuleAction::Delete, &auth, &request, &mut params)? {
        where_sql = format!("{where_sql} AND {rule}");
    }

    let mut client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(describe_pool_error(e)))?;
    let tx = client.transaction().await.map_err(db_error)?;
    tx.query_opt(
        &format!(
            "SELECT 1 FROM {} {ALIAS} WHERE {where_sql} FOR UPDATE OF {ALIAS}",
            table_ref(&c.name)
        ),
        &params.as_refs(),
    )
    .await
    .map_err(db_error)?
    .ok_or_else(ApiError::not_found)?;

    let mut visited = HashSet::new();
    delete_with_references(&tx, &registry, c.clone(), id, 0, &mut visited).await?;
    tx.commit().await.map_err(db_error)?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Deletes a record after handling the relation fields pointing to it:
/// cascade-delete fields delete the referencing records, other fields drop the id,
/// and required fields that would become empty block the delete.
fn delete_with_references<'b>(
    tx: &'b Transaction<'_>,
    registry: &'b Registry,
    c: Arc<Collection>,
    id: String,
    depth: usize,
    visited: &'b mut HashSet<(String, String)>,
) -> BoxFuture<'b, Result<(), ApiError>> {
    Box::pin(async move {
        if !visited.insert((c.id.clone(), id.clone())) {
            return Ok(());
        }
        if depth > MAX_CASCADE_DEPTH {
            return Err(ApiError::bad_request(
                "Failed to delete record: too many cascading deletes.",
            ));
        }
        let blocked = || {
            ApiError::bad_request(
                "Failed to delete record. Make sure that the record is not part of a required relation reference.",
            )
        };

        for (other, field_name) in registry.references_to(&c.id) {
            let Some(field) = other.field(&field_name) else {
                continue;
            };
            let FieldKind::Relation(options) = &field.kind else {
                continue;
            };
            let table = table_ref(&other.name);
            let column = quote_ident(&field.name);
            let (matches, single_left) = if field.is_multi() {
                (
                    format!("{column} ? $1::text"),
                    format!("{column} ? $1::text AND jsonb_array_length({column}) = 1"),
                )
            } else {
                (
                    format!("{column} = $1::text"),
                    format!("{column} = $1::text"),
                )
            };

            if options.cascade_delete {
                let rows = tx
                    .query(&format!("SELECT id FROM {table} WHERE {matches}"), &[&id])
                    .await
                    .map_err(db_error)?;
                for row in rows {
                    let ref_id: String = row.get(0);
                    delete_with_references(tx, registry, other.clone(), ref_id, depth + 1, visited)
                        .await?;
                }
                continue;
            }

            if options.required {
                let blocked_rows = tx
                    .query_opt(
                        &format!(
                            "SELECT 1 FROM {table} WHERE {single_left} AND id <> $1::text LIMIT 1"
                        ),
                        &[&id],
                    )
                    .await
                    .map_err(db_error)?;
                if blocked_rows.is_some() {
                    return Err(blocked());
                }
            }
            let update = if field.is_multi() {
                format!("UPDATE {table} SET {column} = {column} - $1::text WHERE {matches}")
            } else {
                format!("UPDATE {table} SET {column} = '' WHERE {matches}")
            };
            tx.execute(&update, &[&id]).await.map_err(db_error)?;
        }

        tx.execute(
            &format!("DELETE FROM {} WHERE id = $1::text", table_ref(&c.name)),
            &[&id],
        )
        .await
        .map_err(db_error)?;
        Ok(())
    })
}

/// Verifies that all referenced records exist.
async fn check_relations(
    client: &impl GenericClient,
    registry: &Registry,
    c: &Collection,
    values: &Values,
    errors: &mut Errors,
) -> Result<(), ApiError> {
    for field in &c.fields {
        let FieldKind::Relation(options) = &field.kind else {
            continue;
        };
        let ids = values::as_strings(values.get(&field.name).unwrap_or(&Value::Null));
        if ids.is_empty() {
            continue;
        }
        let target_name = if options.collection_id == c.id {
            c.name.clone()
        } else {
            match registry.by_id(&options.collection_id) {
                Some(t) => t.name.clone(),
                None => {
                    errors.invalid(&field.name, "The related collection doesn't exist.");
                    continue;
                }
            }
        };
        let json_ids = Value::Array(ids.iter().cloned().map(Value::String).collect()).to_string();
        let found: i64 = client
            .query_one(
                &format!(
                    "SELECT count(*) FROM {} WHERE id IN (SELECT jsonb_array_elements_text($1::text::jsonb))",
                    table_ref(&target_name)
                ),
                &[&json_ids],
            )
            .await
            .map_err(db_error)?
            .get(0);
        if found != ids.len() as i64 {
            errors.invalid(
                &field.name,
                "Failed to find all relation records with the provided ids.",
            );
        }
    }
    Ok(())
}

/// Maps unique violations to field errors (like PocketBase's "Value must be unique.").
fn write_error(err: tokio_postgres::Error, c: &Collection, message: &str) -> ApiError {
    let Some(db) = err.as_db_error() else {
        return db_error(err);
    };
    if *db.code() != tokio_postgres::error::SqlState::UNIQUE_VIOLATION {
        return db_error(err);
    }
    let constraint = db.constraint().unwrap_or_default();
    let mut errors = Errors::new();
    if constraint.ends_with("_pkey") {
        errors.invalid("id", "Value must be unique.");
    } else {
        for raw in &c.indexes {
            if let Ok(index) = Index::parse(raw) {
                if index.name == constraint {
                    for (column, _) in &index.columns {
                        errors.invalid(column.clone(), "Value must be unique.");
                    }
                }
            }
        }
    }
    if errors.is_empty() {
        errors.invalid("id", "Value must be unique.");
    }
    ApiError::invalid_data(message, &errors)
}
