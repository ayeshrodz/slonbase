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

use super::access::{
    email_visible_sql, manage_rule_sql, output_record, output_select, pick_fields, rule_sql,
};
use super::expand;
use super::files::{self, RecordBody};
use super::store::{ALIAS, all_fields, row_values, select_list, visible_fields};
use super::values::{self, Changes, Values};
use crate::app::App;
use crate::auth::RequestAuth;
use crate::collections::indexes::Index;
use crate::collections::model::{
    Collection, FIELD_EMAIL, FIELD_PASSWORD, FIELD_TOKEN_KEY, FIELD_VERIFIED, FieldKind, RuleAction,
};
use crate::collections::schema::table_ref;
use crate::collections::store::Registry;
use crate::core::Core;
use crate::db::TextQuery;
use crate::db::describe_pool_error;
use crate::error::ApiError;
use crate::filter::{Params, RequestInfo, Scope, compile_sort, quote_ident, to_sql};
use crate::hooks::{Change, Hooks, RecordRequestEvent, RecordsListEvent};
use crate::realtime::Action;
use crate::validation::Errors;

const DEFAULT_PER_PAGE: i64 = 30;
const MAX_PER_PAGE: i64 = 1000;
const MAX_CASCADE_DEPTH: usize = 32;

fn find_collection(registry: &Registry, name: &str) -> Result<Arc<Collection>, ApiError> {
    registry
        .get(name)
        .cloned()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "Missing collection context."))
}

pub(crate) fn request_info(
    method: &Method,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    body: Option<&Map<String, Value>>,
    auth: &RequestAuth,
    registry: &Arc<Registry>,
) -> RequestInfo {
    let mut info = RequestInfo::new(method.as_str());
    info.registry = Some(registry.clone());
    info.auth = auth.info();
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

/// Checks only that the action is allowed at all (non-null rule or superuser).
pub(crate) fn ensure_allowed(
    c: &Collection,
    action: RuleAction,
    auth: &RequestAuth,
) -> Result<(), ApiError> {
    if c.is_view() && !matches!(action, RuleAction::List | RuleAction::View) {
        return Err(ApiError::bad_request("View collections are read-only."));
    }
    if !auth.is_superuser() && c.rule(action).is_none() {
        return Err(ApiError::superusers_only());
    }
    Ok(())
}

fn db_error(err: tokio_postgres::Error) -> ApiError {
    ApiError::internal(err.into())
}

fn bad_filter(e: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(format!("Invalid filter parameters: {e}."))
}

/// Applies the `expand` query parameter to response records.
async fn expand_records(
    core: &Arc<Core>,
    auth: &RequestAuth,
    request: &RequestInfo,
    c: &Arc<Collection>,
    records: &mut [Value],
    query: &HashMap<String, String>,
) -> Result<(), ApiError> {
    let Some(raw) = query.get("expand") else {
        return Ok(());
    };
    let tree = expand::Tree::parse(raw);
    let ctx = expand::Context {
        core,
        auth,
        request,
    };
    expand::apply(&ctx, c.clone(), records, &tree).await
}

/// Runs the [`Hooks::on_record_enrich`] handlers on records of `c` about to be sent to a
/// client, in place.
pub(crate) async fn enrich(
    core: &Arc<Core>,
    request: &RequestInfo,
    c: &Collection,
    records: &mut [Value],
) -> Result<(), ApiError> {
    let hook = &core.hooks.on_record_enrich;
    if records.is_empty() || !hook.has_handlers(Some(c)) {
        return Ok(());
    }
    // superusers see every field, whatever the handlers hide (like PocketBase)
    let superuser = request.auth.as_ref().is_some_and(|a| {
        a.collection_name == crate::collections::model::SUPERUSERS_COLLECTION_NAME
    });
    let (collection, request) = (Arc::new(c.clone()), Arc::new(request.clone()));
    for record in records.iter_mut() {
        let Value::Object(values) = std::mem::take(record) else {
            continue;
        };
        let original = superuser.then(|| values.clone());
        let e = crate::hooks::RecordEnrichEvent {
            core: core.clone(),
            collection: collection.clone(),
            record: values,
            request: request.clone(),
        };
        let mut e = hook.trigger(Some(c), e, |e| async { Ok(e) }).await?;
        for (key, value) in original.into_iter().flatten() {
            if c.field(&key).is_some() {
                e.record.entry(key).or_insert(value);
            }
        }
        *record = Value::Object(e.record);
    }
    Ok(())
}

/// A single record response, enriched and with `expand` applied.
pub(crate) async fn respond(
    core: &Arc<Core>,
    auth: &RequestAuth,
    request: &RequestInfo,
    c: &Arc<Collection>,
    record: Value,
    query: &HashMap<String, String>,
) -> Result<Value, ApiError> {
    let mut records = [record];
    enrich(core, request, c, &mut records).await?;
    expand_records(core, auth, request, c, &mut records, query).await?;
    let [record] = records;
    Ok(record)
}

/// `GET /api/collections/{collection}/records`
pub async fn list(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    method: Method,
    headers: HeaderMap,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = find_collection(&core.registry(), &collection)?;
    let request = request_info(&method, &query, &headers, None, &auth, &core.registry());
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

    // the count only uses the WHERE parameters (unused ones would fail to prepare)
    let count_params = params.clone();
    let email_visible = email_visible_sql(&c, &auth, &request, &mut params)?;

    let fields = visible_fields(&c, superuser);
    let table = table_ref(&c.name);
    let items_sql = format!(
        "SELECT {} FROM {table} {ALIAS} WHERE {where_sql} ORDER BY {} LIMIT {per_page} OFFSET {offset}",
        output_select(&fields, &email_visible),
        order.join(", ")
    );
    let count_sql = format!("SELECT count(*) FROM {table} {ALIAS} WHERE {where_sql}");

    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(describe_pool_error(e)))?;
    // pipelined on the same connection
    let (rows, total) = if skip_total {
        (
            client
                .query_text(&items_sql, &params)
                .await
                .map_err(db_error)?,
            -1,
        )
    } else {
        let (rows, count) = tokio::try_join!(
            client.query_text(&items_sql, &params),
            client.query_text_one(&count_sql, &count_params)
        )
        .map_err(db_error)?;
        (rows, count.get::<_, i64>(0))
    };

    drop(client);
    let items: Vec<Value> = rows
        .iter()
        .map(|row| output_record(&c, &fields, row, &email_visible))
        .collect();
    let total_pages = if total < 0 {
        -1
    } else {
        (total + per_page - 1) / per_page
    };
    let e = RecordsListEvent {
        core: core.clone(),
        collection: c.clone(),
        records: items,
        page,
        per_page,
        total_items: total,
        total_pages,
        auth,
        request: Arc::new(request),
        response: None,
    };
    let mut e = core
        .hooks
        .on_records_list_request
        .trigger(Some(&c), e, |e| async { Ok(e) })
        .await?;
    if let Some(response) = e.response {
        return Ok(response);
    }
    enrich(&core, &e.request, &c, &mut e.records).await?;
    expand_records(&core, &e.auth, &e.request, &c, &mut e.records, &query).await?;
    let items: Vec<Value> = e
        .records
        .into_iter()
        .map(|item| pick_fields(item, query.get("fields")))
        .collect();

    Ok(Json(json!({
        "page": e.page,
        "perPage": e.per_page,
        "totalItems": e.total_items,
        "totalPages": e.total_pages,
        "items": items,
    }))
    .into_response())
}

/// `GET /api/collections/{collection}/records/{id}`
pub async fn view(
    State(app): State<Arc<App>>,
    Path((collection, id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    method: Method,
    headers: HeaderMap,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = find_collection(&core.registry(), &collection)?;
    let request = request_info(&method, &query, &headers, None, &auth, &core.registry());

    let mut params = Params::default();
    let id_param = params.push(id);
    let mut where_sql = format!("{ALIAS}.\"id\" = {id_param}");
    if let Some(rule) = rule_sql(&c, RuleAction::View, &auth, &request, &mut params)? {
        where_sql = format!("{where_sql} AND {rule}");
    }
    let email_visible = email_visible_sql(&c, &auth, &request, &mut params)?;

    let fields = visible_fields(&c, auth.is_superuser());
    let sql = format!(
        "SELECT {} FROM {} {ALIAS} WHERE {where_sql} LIMIT 1",
        output_select(&fields, &email_visible),
        table_ref(&c.name)
    );
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(describe_pool_error(e)))?;
    let row = client
        .query_text_opt(&sql, &params)
        .await
        .map_err(db_error)?
        .ok_or_else(ApiError::not_found)?;

    drop(client);
    let record = output_record(&c, &fields, &row, &email_visible);
    let e = RecordRequestEvent {
        core: core.clone(),
        collection: c.clone(),
        record: into_map(record),
        auth,
        request: Arc::new(request),
        response: None,
    };
    let e = core
        .hooks
        .on_record_view_request
        .trigger(Some(&c), e, |e| async { Ok(e) })
        .await?;
    record_response(&core, e, &query).await
}

/// Checks `passwordConfirm` for auth collections.
fn check_password_confirm(c: &Collection, body: &Map<String, Value>, errors: &mut Errors) {
    if !c.is_auth() {
        return;
    }
    let password = body
        .get(FIELD_PASSWORD)
        .and_then(Value::as_str)
        .unwrap_or_default();
    if password.is_empty() {
        return;
    }
    if body.get("passwordConfirm").and_then(Value::as_str) != Some(password) {
        errors.add(
            "passwordConfirm",
            "validation_values_mismatch",
            "Values don't match.",
        );
    }
}

/// Hashes the changed passwords into `values` and rotates the token key when needed.
async fn apply_secrets(
    c: &Collection,
    values: &mut Values,
    changes: &Changes,
    rotate_token_key: bool,
) -> Result<(), ApiError> {
    for (name, plain) in &changes.passwords {
        let field = c.field(name).expect("password field exists");
        let hash = values::hash_password(field, plain.clone())
            .await
            .map_err(ApiError::internal)?;
        values.insert(name.clone(), Value::String(hash));
    }
    if c.is_auth() && (rotate_token_key || !changes.passwords.is_empty()) {
        values.insert(
            FIELD_TOKEN_KEY.into(),
            Value::String(
                crate::ids::generate_from_pattern("[a-zA-Z0-9]{50}").expect("valid pattern"),
            ),
        );
    }
    Ok(())
}

/// Where a write runs: the caller, its request and the collections.
pub(crate) struct WriteContext<'a> {
    pub core: &'a Arc<Core>,
    pub registry: &'a Arc<Registry>,
    pub auth: &'a RequestAuth,
    pub request: &'a RequestInfo,
}

/// File changes of writes in a transaction: new files are discarded if it fails, replaced
/// and deleted files are removed once it commits.
#[derive(Default)]
pub(crate) struct FileEffects {
    written: Vec<files::NewFile>,
    replaced: Vec<(String, String, Vec<String>)>,
    deleted_records: Vec<(String, String)>,
    /// Record changes, for the after-success hooks.
    pub(crate) changes: Vec<Change>,
    /// Writes that failed, with their errors, for the after-error hooks.
    failed: Vec<(Change, ApiError)>,
}

impl FileEffects {
    /// Applies the file changes for the transaction's outcome and returns it.
    pub(crate) async fn settle<T>(
        self,
        core: &Arc<Core>,
        outcome: Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        match outcome {
            Err(err) => {
                files::discard(core, &self.written).await;
                // the failed writes, and the ones the rollback undid
                let undone = self.changes.into_iter().map(|c| (c, err.clone()));
                Hooks::failed(core, self.failed.into_iter().chain(undone).collect());
                Err(err)
            }
            Ok(value) => {
                for (collection_id, record_id, names) in &self.replaced {
                    files::remove_replaced(core, collection_id, record_id, names).await;
                }
                // the files of every deleted record (including cascades)
                for (collection_id, record_id) in &self.deleted_records {
                    if let Err(err) = core
                        .storage()
                        .delete_prefix(&files::record_prefix(collection_id, record_id))
                        .await
                    {
                        tracing::warn!(error = %format!("{err:#}"), "failed to delete record files");
                    }
                }
                Hooks::committed(core, self.changes);
                // writes that failed in a transaction that went on (an error caught in a
                // migration)
                Hooks::failed(core, self.failed);
                Ok(value)
            }
        }
    }
}

async fn transaction(client: &mut deadpool_postgres::Client) -> Result<Transaction<'_>, ApiError> {
    client.transaction().await.map_err(db_error)
}

async fn pooled(core: &Core) -> Result<deadpool_postgres::Client, ApiError> {
    core.pool
        .get()
        .await
        .map_err(|e| ApiError::internal(describe_pool_error(e)))
}

/// Fields a model hook never sees or changes.
const SECRET_FIELDS: [&str; 1] = [FIELD_TOKEN_KEY];

/// Runs a model hook (`on_record_create`, ...) on `record` and merges what its handlers
/// changed into `body`, so the change goes through the usual validation.
async fn model_hook(
    ctx: &WriteContext<'_>,
    hook: &crate::hooks::Hook<crate::hooks::RecordEvent>,
    c: &Collection,
    mut record: Map<String, Value>,
    body: &mut Map<String, Value>,
) -> Result<(), ApiError> {
    if !hook.has_handlers(Some(c)) {
        return Ok(());
    }
    for key in SECRET_FIELDS {
        record.remove(key);
    }
    let e = crate::hooks::RecordEvent {
        core: ctx.core.clone(),
        collection: Arc::new(c.clone()),
        record: record.clone(),
    };
    let e = hook.trigger(Some(c), e, |e| async { Ok(e) }).await?;
    for (key, value) in e.record {
        if SECRET_FIELDS.contains(&key.as_str()) || record.get(&key) == Some(&value) {
            continue;
        }
        if key == FIELD_PASSWORD {
            body.insert("passwordConfirm".into(), value.clone());
        }
        body.insert(key, value);
    }
    Ok(())
}

/// Runs the `on_record_validate` handlers on the validated values (changes are ignored).
async fn validate_hook(
    ctx: &WriteContext<'_>,
    c: &Collection,
    values: &Values,
) -> Result<(), ApiError> {
    let hook = &ctx.core.hooks.on_record_validate;
    if !hook.has_handlers(Some(c)) {
        return Ok(());
    }
    let mut record = values.clone();
    record.remove(FIELD_PASSWORD);
    for key in SECRET_FIELDS {
        record.remove(key);
    }
    let e = crate::hooks::RecordEvent {
        core: ctx.core.clone(),
        collection: Arc::new(c.clone()),
        record,
    };
    hook.trigger(Some(c), e, |e| async { Ok(e) }).await?;
    Ok(())
}

/// The record as a model hook sees it: `base` with the body's field values applied.
fn hook_record(c: &Collection, base: Values, body: &Map<String, Value>) -> Map<String, Value> {
    let mut record = base;
    for field in &c.fields {
        if let Some(value) = body.get(&field.name) {
            record.insert(field.name.clone(), value.clone());
        }
    }
    record.remove(FIELD_PASSWORD);
    if let Some(password) = body.get(FIELD_PASSWORD) {
        record.insert(FIELD_PASSWORD.into(), password.clone());
    }
    record
}

/// The record as execute hooks see it: the values without secrets and passwords.
fn hook_view(c: &Collection, values: &Values) -> Map<String, Value> {
    let mut record = values.clone();
    for field in c.fields.iter().filter(|f| f.is_secret()) {
        record.remove(&field.name);
    }
    record
}

/// Applies what execute handlers changed in the record to `values`, checked like the
/// request's values (files, passwords and the id can't change there).
fn apply_execute_changes(
    c: &Collection,
    values: &mut Values,
    given: &Map<String, Value>,
    returned: Map<String, Value>,
    create: bool,
    failed: &str,
) -> Result<(), ApiError> {
    let mut body = Map::new();
    for (key, value) in returned {
        let Some(field) = c.field(&key) else {
            continue;
        };
        let fixed =
            field.is_secret() || field.is_primary_key() || matches!(field.kind, FieldKind::File(_));
        if !fixed && given.get(&key) != Some(&value) {
            body.insert(key, value);
        }
    }
    if body.is_empty() {
        return Ok(());
    }
    let mut errors = Errors::new();
    let changes = values::apply_body(c, values, &body, create, &mut errors);
    values::validate(c, values, &changes, create, &mut errors);
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }
    Ok(())
}

/// Runs an execute hook (`on_record_create_execute`, ...) around `write`, the write itself.
/// Handlers' `next` runs it, so code after `next` runs after the write, in the transaction,
/// and sees what `write` returns as written. `write` gets the record as the handlers left
/// it. A handler that doesn't call `next` cancels the write.
async fn execute_hook<T, F, Fut>(
    core: &Arc<Core>,
    hook: &crate::hooks::Hook<crate::hooks::RecordEvent>,
    c: &Collection,
    record: Map<String, Value>,
    write: F,
) -> Result<T, ApiError>
where
    F: FnOnce(Map<String, Value>) -> Fut,
    Fut: Future<Output = Result<(T, Map<String, Value>), ApiError>>,
{
    if !hook.has_handlers(Some(c)) {
        return write(record).await.map(|(out, _)| out);
    }
    // the handlers run as their own future ('static, so it can't hold the transaction):
    // `next` hands the record over to the write here and waits for it
    let (start, started) = tokio::sync::oneshot::channel::<Map<String, Value>>();
    let (finish, finished) =
        tokio::sync::oneshot::channel::<Result<Map<String, Value>, ApiError>>();
    let e = crate::hooks::RecordEvent {
        core: core.clone(),
        collection: Arc::new(c.clone()),
        record,
    };
    let hooked = hook.trigger(Some(c), e, move |mut e| async move {
        let _ = start.send(std::mem::take(&mut e.record));
        e.record = finished
            .await
            .map_err(|_| ApiError::internal(anyhow::anyhow!("the write was dropped")))??;
        Ok(e)
    });
    let writing = async {
        let record = started.await.ok()?;
        let result = write(record).await;
        let _ = finish.send(match &result {
            Ok((_, written)) => Ok(written.clone()),
            Err(err) => Err(err.clone()),
        });
        Some(result)
    };
    let (hooked, written) = tokio::join!(hooked, writing);
    match written {
        // a failed statement fails the transaction, whatever the handlers did with the error
        Some(Err(err)) => Err(err),
        Some(Ok((out, _))) => hooked.map(|_| out),
        None => {
            hooked?;
            Err(ApiError::bad_request("The write was cancelled by a hook."))
        }
    }
}

/// Whether a failed write is reported to the after-error hooks: not when the caller may not
/// write the record at all (as in PocketBase, where these fail before the save).
fn reported(err: &ApiError) -> bool {
    !matches!(err.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND)
}

/// Creates a record in `tx` and returns it as the caller sees it; a failure is reported to
/// the after-error hooks.
pub(crate) async fn create_in(
    ctx: &WriteContext<'_>,
    tx: &Transaction<'_>,
    c: &Collection,
    body: Map<String, Value>,
    uploads: Vec<files::Upload>,
    effects: &mut FileEffects,
) -> Result<Value, ApiError> {
    let attempted = ctx
        .core
        .hooks
        .on_record_after_create_error
        .has_handlers(Some(c))
        .then(|| hook_record(c, values::defaults(c), &body));
    let result = create_checked(ctx, tx, c, body, uploads, effects).await;
    if let (Err(err), Some(values)) = (&result, attempted) {
        if reported(err) {
            failed_change(effects, ctx.registry, c, Action::Create, values, err);
        }
    }
    result
}

/// Updates a record in `tx` and returns it as the caller sees it; a failure is reported to
/// the after-error hooks.
pub(crate) async fn update_in(
    ctx: &WriteContext<'_>,
    tx: &Transaction<'_>,
    c: &Collection,
    id: &str,
    body: Map<String, Value>,
    uploads: Vec<files::Upload>,
    effects: &mut FileEffects,
) -> Result<Value, ApiError> {
    let attempted = ctx
        .core
        .hooks
        .on_record_after_update_error
        .has_handlers(Some(c))
        .then(|| {
            let mut base = Values::new();
            base.insert("id".into(), json!(id));
            hook_record(c, base, &body)
        });
    let result = update_checked(ctx, tx, c, id, body, uploads, effects).await;
    if let (Err(err), Some(values)) = (&result, attempted) {
        if reported(err) {
            failed_change(effects, ctx.registry, c, Action::Update, values, err);
        }
    }
    result
}

/// Deletes a record (with its cascades) in `tx`; a failure is reported to the after-error
/// hooks.
pub(crate) async fn delete_in(
    ctx: &WriteContext<'_>,
    tx: &Transaction<'_>,
    c: &Arc<Collection>,
    id: &str,
    effects: &mut FileEffects,
) -> Result<(), ApiError> {
    let result = delete_checked(ctx, tx, c, id, effects).await;
    if let Err(err) = &result {
        if reported(err)
            && ctx
                .core
                .hooks
                .on_record_after_delete_error
                .has_handlers(Some(c))
        {
            let mut values = Values::new();
            values.insert("id".into(), json!(id));
            failed_change(effects, ctx.registry, c, Action::Delete, values, err);
        }
    }
    result
}

fn failed_change(
    effects: &mut FileEffects,
    registry: &Registry,
    c: &Collection,
    action: Action,
    mut values: Values,
    err: &ApiError,
) {
    values.remove(FIELD_PASSWORD);
    if let Some(collection) = registry.by_id(&c.id) {
        let change = Change {
            collection: collection.clone(),
            action,
            values,
        };
        effects.failed.push((change, err.clone()));
    }
}

/// Creates a record in `tx` and returns it as the caller sees it.
async fn create_checked(
    ctx: &WriteContext<'_>,
    tx: &Transaction<'_>,
    c: &Collection,
    mut body: Map<String, Value>,
    uploads: Vec<files::Upload>,
    effects: &mut FileEffects,
) -> Result<Value, ApiError> {
    let (core, auth, request) = (ctx.core, ctx.auth, ctx.request);
    ensure_allowed(c, RuleAction::Create, auth)?;
    let failed = "Failed to create record.";

    // only superusers and managers (with an empty manage rule) can create verified records
    let manage_all = c.auth_options().is_some_and(|a| {
        a.manage_rule
            .as_deref()
            .is_some_and(|r| r.trim().is_empty())
    });
    if c.is_auth() && !auth.is_superuser() && !manage_all {
        body.remove(FIELD_VERIFIED);
    }
    let record = hook_record(c, values::defaults(c), &body);
    model_hook(ctx, &core.hooks.on_record_create, c, record, &mut body).await?;

    let mut values = values::defaults(c);
    let mut errors = Errors::new();
    let changes = values::apply_body(c, &mut values, &body, true, &mut errors);
    values::finalize(c, &mut values, true, &request.now, &mut errors);
    let file_plan = files::apply(c, &mut values, &Values::new(), &body, uploads, &mut errors);
    values::validate(c, &values, &changes, true, &mut errors);
    check_password_confirm(c, &body, &mut errors);
    if errors.is_empty() {
        check_relations(tx, ctx.registry, c, &values, &mut errors).await?;
    }
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }
    validate_hook(ctx, c, &values).await?;
    apply_secrets(c, &mut values, &changes, false).await?;
    files::write(core, &file_plan).await?;
    effects.written.extend(file_plan.new);

    let given = hook_view(c, &values);
    let hook = &core.hooks.on_record_create_execute;
    let (record, values) = execute_hook(ctx.core, hook, c, given.clone(), |returned| async move {
        apply_execute_changes(c, &mut values, &given, returned, true, failed)?;
        let record = insert_record(ctx, tx, c, &values, failed).await?;
        let written = hook_view(c, &values);
        Ok(((record, values), written))
    })
    .await?;
    record_change(effects, ctx.registry, c, Action::Create, values);
    Ok(record)
}

/// The write of a create: inserts the record, checks the create rule against it and queues
/// its realtime event.
async fn insert_record(
    ctx: &WriteContext<'_>,
    tx: &Transaction<'_>,
    c: &Collection,
    values: &Values,
    failed: &str,
) -> Result<Value, ApiError> {
    let (auth, request) = (ctx.auth, ctx.request);
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
    let email_visible = email_visible_sql(c, auth, request, &mut params)?;
    let fields = visible_fields(c, auth.is_superuser());
    let sql = format!(
        "INSERT INTO {} AS {ALIAS} ({}) VALUES ({}) RETURNING {}",
        table_ref(&c.name),
        columns.join(", "),
        placeholders.join(", "),
        output_select(&fields, &email_visible)
    );
    let row = tx
        .query_text_one(&sql, &params)
        .await
        .map_err(|e| write_error(e, c, failed))?;

    // the create rule is checked against the inserted record
    let mut rule_params = Params::default();
    let id = values
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let id_param = rule_params.push(id);
    if let Some(rule) = rule_sql(c, RuleAction::Create, auth, request, &mut rule_params)? {
        let check = format!(
            "SELECT 1 FROM {} {ALIAS} WHERE {ALIAS}.\"id\" = {id_param} AND {rule}",
            table_ref(&c.name)
        );
        if tx
            .query_text_opt(&check, &rule_params)
            .await
            .map_err(db_error)?
            .is_none()
        {
            // the caller's transaction is rolled back
            return Err(ApiError::bad_request(failed));
        }
    }
    Ok(output_record(c, &fields, &row, &email_visible))
}

/// Updates a record in `tx` and returns it as the caller sees it.
async fn update_checked(
    ctx: &WriteContext<'_>,
    tx: &Transaction<'_>,
    c: &Collection,
    id: &str,
    mut body: Map<String, Value>,
    uploads: Vec<files::Upload>,
    effects: &mut FileEffects,
) -> Result<Value, ApiError> {
    let (core, auth, request) = (ctx.core, ctx.auth, ctx.request);
    ensure_allowed(c, RuleAction::Update, auth)?;
    let failed = "Failed to update record.";

    // the rule is checked against the current record (with @request.body available)
    let mut params = Params::default();
    let id_param = params.push(id.to_string());
    let mut where_sql = format!("{ALIAS}.\"id\" = {id_param}");
    if let Some(rule) = rule_sql(c, RuleAction::Update, auth, request, &mut params)? {
        where_sql = format!("{where_sql} AND {rule}");
    }

    let stored = all_fields(c);
    if core.hooks.on_record_update.has_handlers(Some(c)) {
        // read without the row lock, so a handler writing through the record service
        // can't wait on this transaction
        let row = tx
            .query_text_opt(
                &format!(
                    "SELECT {} FROM {} {ALIAS} WHERE {where_sql}",
                    select_list(&stored),
                    table_ref(&c.name)
                ),
                &params,
            )
            .await
            .map_err(db_error)?
            .ok_or_else(ApiError::not_found)?;
        let record = hook_record(c, row_values(&stored, &row), &body);
        model_hook(ctx, &core.hooks.on_record_update, c, record, &mut body).await?;
    }
    let current = tx
        .query_text_opt(
            &format!(
                "SELECT {} FROM {} {ALIAS} WHERE {where_sql} FOR UPDATE OF {ALIAS}",
                select_list(&stored),
                table_ref(&c.name)
            ),
            &params,
        )
        .await
        .map_err(db_error)?
        .ok_or_else(ApiError::not_found)?;
    let mut values = row_values(&stored, &current);
    let original = values.clone();
    let mut errors = Errors::new();

    // auth records: email, verified and passwords need the right to manage the record
    let mut email_changed = false;
    if c.is_auth() {
        let can_manage =
            auth.is_superuser() || {
                let mut manage_params = Params::default();
                let id_param = manage_params.push(id.to_string());
                match manage_rule_sql(c, request, &mut manage_params)? {
                None => false,
                Some(rule) => tx
                    .query_text_opt(&format!(
                            "SELECT 1 FROM {} {ALIAS} WHERE {ALIAS}.\"id\" = {id_param} AND {rule}",
                            table_ref(&c.name)
                        ), &manage_params)
                    .await
                    .map_err(db_error)?
                    .is_some(),
            }
            };

        let current_email = values
            .get(FIELD_EMAIL)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(new_email) = body.get(FIELD_EMAIL).and_then(Value::as_str) {
            email_changed = !new_email.eq_ignore_ascii_case(&current_email);
            if email_changed && !can_manage {
                errors.invalid(
                    FIELD_EMAIL,
                    "The email can only be changed with an email change request.",
                );
            }
        }
        if !can_manage {
            body.remove(FIELD_VERIFIED);
            let changing_password = body
                .get(FIELD_PASSWORD)
                .and_then(Value::as_str)
                .is_some_and(|p| !p.is_empty());
            if changing_password {
                let old = body
                    .get("oldPassword")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let hash = values
                    .get(FIELD_PASSWORD)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if old.is_empty() {
                    errors.add("oldPassword", "validation_required", "Cannot be blank.");
                } else if !values::verify_password(old, hash).await {
                    errors.add(
                        "oldPassword",
                        "validation_invalid_old_password",
                        "Missing or invalid old password.",
                    );
                }
            }
        }
    }

    let changes = values::apply_body(c, &mut values, &body, false, &mut errors);
    values::finalize(c, &mut values, false, &request.now, &mut errors);
    let file_plan = files::apply(c, &mut values, &original, &body, uploads, &mut errors);
    values::validate(c, &values, &changes, false, &mut errors);
    check_password_confirm(c, &body, &mut errors);
    if errors.is_empty() {
        check_relations(tx, ctx.registry, c, &values, &mut errors).await?;
    }
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }
    validate_hook(ctx, c, &values).await?;
    apply_secrets(c, &mut values, &changes, email_changed).await?;
    files::write(core, &file_plan).await?;
    effects.written.extend(file_plan.new);
    effects
        .replaced
        .push((c.id.clone(), id.to_string(), file_plan.removed));

    let given = hook_view(c, &values);
    let hook = &core.hooks.on_record_update_execute;
    let (record, values) = execute_hook(ctx.core, hook, c, given.clone(), |returned| async move {
        apply_execute_changes(c, &mut values, &given, returned, false, failed)?;
        let record = update_record(ctx, tx, c, id, &values, failed).await?;
        let written = hook_view(c, &values);
        Ok(((record, values), written))
    })
    .await?;
    record_change(effects, ctx.registry, c, Action::Update, values);
    Ok(record)
}

/// The write of an update: updates the row and queues its realtime event.
async fn update_record(
    ctx: &WriteContext<'_>,
    tx: &Transaction<'_>,
    c: &Collection,
    id: &str,
    values: &Values,
    failed: &str,
) -> Result<Value, ApiError> {
    let (auth, request) = (ctx.auth, ctx.request);
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
    let id_param = params.push(id.to_string());
    let email_visible = email_visible_sql(c, auth, request, &mut params)?;
    let fields = visible_fields(c, auth.is_superuser());
    let sql = format!(
        "UPDATE {} AS {ALIAS} SET {} WHERE {ALIAS}.\"id\" = {id_param} RETURNING {}",
        table_ref(&c.name),
        sets.join(", "),
        output_select(&fields, &email_visible)
    );
    let row = tx
        .query_text_one(&sql, &params)
        .await
        .map_err(|e| write_error(e, c, failed))?;
    Ok(output_record(c, &fields, &row, &email_visible))
}

fn record_change(
    effects: &mut FileEffects,
    registry: &Registry,
    c: &Collection,
    action: Action,
    values: Values,
) {
    if let Some(collection) = registry.by_id(&c.id) {
        effects.changes.push(Change {
            collection: collection.clone(),
            action,
            values,
        });
    }
}

/// Deletes a record (with its cascades) in `tx`.
async fn delete_checked(
    ctx: &WriteContext<'_>,
    tx: &Transaction<'_>,
    c: &Arc<Collection>,
    id: &str,
    effects: &mut FileEffects,
) -> Result<(), ApiError> {
    let (auth, request) = (ctx.auth, ctx.request);
    ensure_allowed(c, RuleAction::Delete, auth)?;
    let mut params = Params::default();
    let id_param = params.push(id.to_string());
    let mut where_sql = format!("{ALIAS}.\"id\" = {id_param}");
    if let Some(rule) = rule_sql(c, RuleAction::Delete, auth, request, &mut params)? {
        where_sql = format!("{where_sql} AND {rule}");
    }
    let hooks = &ctx.core.hooks;
    let mut current = Map::new();
    if hooks.on_record_delete.has_handlers(Some(c))
        || hooks.on_record_delete_execute.has_handlers(Some(c))
    {
        let stored = all_fields(c);
        let row = tx
            .query_text_opt(
                &format!(
                    "SELECT {} FROM {} {ALIAS} WHERE {where_sql}",
                    select_list(&stored),
                    table_ref(&c.name)
                ),
                &params,
            )
            .await
            .map_err(db_error)?
            .ok_or_else(ApiError::not_found)?;
        let mut record = row_values(&stored, &row);
        record.remove(FIELD_PASSWORD);
        current = hook_view(c, &record);
        // changes are ignored: the record is deleted
        model_hook(ctx, &hooks.on_record_delete, c, record, &mut Map::new()).await?;
    }
    tx.query_text_opt(
        &format!(
            "SELECT 1 FROM {} {ALIAS} WHERE {where_sql} FOR UPDATE OF {ALIAS}",
            table_ref(&c.name)
        ),
        &params,
    )
    .await
    .map_err(db_error)?
    .ok_or_else(ApiError::not_found)?;

    if c.id == crate::collections::model::SUPERUSERS_COLLECTION_ID {
        // serializes concurrent deletes, so two can't remove the last two superusers
        tx.batch_execute(&format!(
            "LOCK TABLE {} IN SHARE ROW EXCLUSIVE MODE",
            table_ref(&c.name)
        ))
        .await
        .map_err(db_error)?;
        let count: i64 = tx
            .query_one(&format!("SELECT count(*) FROM {}", table_ref(&c.name)), &[])
            .await
            .map_err(db_error)?
            .get(0);
        if count <= 1 {
            return Err(ApiError::bad_request(
                "You can't delete the only existing superuser.",
            ));
        }
    }

    // the write; changes to the record are ignored
    let hook = &hooks.on_record_delete_execute;
    execute_hook(ctx.core, hook, c, current.clone(), |_| async move {
        let mut visited = HashSet::new();
        delete_with_references(
            ctx.core,
            tx,
            ctx.registry,
            c.clone(),
            id.to_string(),
            0,
            &mut visited,
            &mut effects.changes,
        )
        .await?;
        effects.deleted_records.extend(visited);
        Ok(((), current))
    })
    .await
}

/// The record a request hook sees for an update or delete: the current record, if the
/// caller's rule for the action allows it (404 otherwise).
async fn current_for_hook(
    core: &Core,
    c: &Collection,
    id: &str,
    action: RuleAction,
    auth: &RequestAuth,
    request: &RequestInfo,
) -> Result<Values, ApiError> {
    let client = pooled(core).await?;
    current_in(&client, c, id, action, auth, request).await
}

/// The record a hook sees for an update or delete request, read with `client` (a batch's
/// transaction sees the batch's earlier writes): 404 when the rule doesn't allow it.
pub(crate) async fn current_in(
    client: &impl TextQuery,
    c: &Collection,
    id: &str,
    action: RuleAction,
    auth: &RequestAuth,
    request: &RequestInfo,
) -> Result<Values, ApiError> {
    let mut params = Params::default();
    let id_param = params.push(id.to_string());
    let mut where_sql = format!("{ALIAS}.\"id\" = {id_param}");
    if let Some(rule) = rule_sql(c, action, auth, request, &mut params)? {
        where_sql = format!("{where_sql} AND {rule}");
    }
    let fields = visible_fields(c, true);
    let row = client
        .query_text_opt(
            &format!(
                "SELECT {} FROM {} {ALIAS} WHERE {where_sql} LIMIT 1",
                select_list(&fields),
                table_ref(&c.name)
            ),
            &params,
        )
        .await
        .map_err(db_error)?
        .ok_or_else(ApiError::not_found)?;
    Ok(row_values(&fields, &row))
}

pub(crate) fn into_map(record: Value) -> Map<String, Value> {
    match record {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// The response of a create, update or view request: the hook's response, or the record
/// with `expand` and `fields` applied.
async fn record_response(
    core: &Arc<Core>,
    e: RecordRequestEvent,
    query: &HashMap<String, String>,
) -> Result<Response, ApiError> {
    if let Some(response) = e.response {
        return Ok(response);
    }
    let record = respond(
        core,
        &e.auth,
        &e.request,
        &e.collection,
        Value::Object(e.record),
        query,
    )
    .await?;
    Ok(Json(pick_fields(record, query.get("fields"))).into_response())
}

/// `POST /api/collections/{collection}/records`
pub async fn create(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    method: Method,
    headers: HeaderMap,
    auth: RequestAuth,
    input: RecordBody,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let e = create_request(&core, &collection, &query, &method, &headers, auth, input).await?;
    record_response(&core, e, &query).await
}

/// Creates a record as a create request does, request hooks included.
pub(crate) async fn create_request(
    core: &Arc<Core>,
    collection: &str,
    query: &HashMap<String, String>,
    method: &Method,
    headers: &HeaderMap,
    auth: RequestAuth,
    mut input: RecordBody,
) -> Result<RecordRequestEvent, ApiError> {
    let registry = core.registry();
    let c = find_collection(&registry, collection)?;
    ensure_allowed(&c, RuleAction::Create, &auth)?;
    input.normalize_form_values(&c);
    let request = Arc::new(request_info(
        method,
        query,
        headers,
        Some(&input.data),
        &auth,
        &registry,
    ));
    let e = RecordRequestEvent {
        core: core.clone(),
        collection: c.clone(),
        record: input.data,
        auth,
        request,
        response: None,
    };
    let uploads = input.uploads;
    core.hooks
        .on_record_create_request
        .trigger(Some(&c), e, move |mut e| async move {
            let body = std::mem::take(&mut e.record);
            let ctx = WriteContext {
                core: &e.core,
                registry: &registry,
                auth: &e.auth,
                request: &e.request,
            };
            let mut effects = FileEffects::default();
            let outcome = async {
                let mut client = pooled(ctx.core).await?;
                let tx = transaction(&mut client).await?;
                let record =
                    create_in(&ctx, &tx, &e.collection, body, uploads, &mut effects).await?;
                tx.commit().await.map_err(db_error)?;
                Ok(record)
            }
            .await;
            let record = effects.settle(&e.core, outcome).await?;
            e.record = into_map(record);
            Ok(e)
        })
        .await
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
    mut input: RecordBody,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let registry = core.registry();
    let c = find_collection(&registry, &collection)?;
    input.normalize_form_values(&c);
    let request = Arc::new(request_info(
        &method,
        &query,
        &headers,
        Some(&input.data),
        &auth,
        &registry,
    ));
    let hook = &core.hooks.on_record_update_request;
    // hooks see the current record with the submitted data applied
    let (record, current) = if hook.has_handlers(Some(&c)) {
        ensure_allowed(&c, RuleAction::Update, &auth)?;
        let current = current_for_hook(&core, &c, &id, RuleAction::Update, &auth, &request).await?;
        let mut record = current.clone();
        record.extend(input.data);
        (record, Some(current))
    } else {
        (input.data, None)
    };
    let e = RecordRequestEvent {
        core: core.clone(),
        collection: c.clone(),
        record,
        auth,
        request,
        response: None,
    };
    let uploads = input.uploads;
    let e = hook
        .trigger(Some(&c), e, move |mut e| async move {
            let mut body = std::mem::take(&mut e.record);
            if let Some(current) = current {
                // only what the request or the hooks changed
                body.retain(|k, v| current.get(k) != Some(v));
            }
            let ctx = WriteContext {
                core: &e.core,
                registry: &registry,
                auth: &e.auth,
                request: &e.request,
            };
            let mut effects = FileEffects::default();
            let outcome = async {
                let mut client = pooled(ctx.core).await?;
                let tx = transaction(&mut client).await?;
                let record =
                    update_in(&ctx, &tx, &e.collection, &id, body, uploads, &mut effects).await?;
                tx.commit().await.map_err(db_error)?;
                Ok(record)
            }
            .await;
            let record = effects.settle(&e.core, outcome).await?;
            e.record = into_map(record);
            Ok(e)
        })
        .await?;
    record_response(&core, e, &query).await
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
    let request = Arc::new(request_info(
        &method, &query, &headers, None, &auth, &registry,
    ));
    let hook = &core.hooks.on_record_delete_request;
    let record = if hook.has_handlers(Some(&c)) {
        ensure_allowed(&c, RuleAction::Delete, &auth)?;
        current_for_hook(&core, &c, &id, RuleAction::Delete, &auth, &request).await?
    } else {
        Map::new()
    };
    let e = RecordRequestEvent {
        core: core.clone(),
        collection: c.clone(),
        record,
        auth,
        request,
        response: None,
    };
    let e = hook
        .trigger(Some(&c), e, move |e| async move {
            let ctx = WriteContext {
                core: &e.core,
                registry: &registry,
                auth: &e.auth,
                request: &e.request,
            };
            let mut effects = FileEffects::default();
            let outcome = async {
                let mut client = pooled(ctx.core).await?;
                let tx = transaction(&mut client).await?;
                delete_in(&ctx, &tx, &e.collection, &id, &mut effects).await?;
                tx.commit().await.map_err(db_error)?;
                Ok(())
            }
            .await;
            effects.settle(&e.core, outcome).await?;
            Ok(e)
        })
        .await?;
    Ok(e.response
        .unwrap_or_else(|| StatusCode::NO_CONTENT.into_response()))
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Deletes a record after handling the relation fields pointing to it:
/// cascade-delete fields delete the referencing records, other fields drop the id,
/// and required fields that would become empty block the delete.
#[allow(clippy::too_many_arguments)]
fn delete_with_references<'b>(
    core: &'b Arc<Core>,
    tx: &'b Transaction<'_>,
    registry: &'b Registry,
    c: Arc<Collection>,
    id: String,
    depth: usize,
    visited: &'b mut HashSet<(String, String)>,
    changes: &'b mut Vec<Change>,
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
                    // a cascaded delete runs the model hooks too (an error cancels it all)
                    let hooks = &core.hooks;
                    let mut current = Map::new();
                    if hooks.on_record_delete.has_handlers(Some(&other))
                        || hooks.on_record_delete_execute.has_handlers(Some(&other))
                    {
                        let stored = all_fields(&other);
                        let found = tx
                            .query_opt(
                                &format!(
                                    "SELECT {} FROM {table} {ALIAS} WHERE {ALIAS}.id = $1::text",
                                    select_list(&stored)
                                ),
                                &[&ref_id],
                            )
                            .await
                            .map_err(db_error)?;
                        if let Some(found) = found {
                            let mut record = row_values(&stored, &found);
                            record.remove(FIELD_PASSWORD);
                            for key in SECRET_FIELDS {
                                record.remove(key);
                            }
                            current = record.clone();
                            let e = crate::hooks::RecordEvent {
                                core: core.clone(),
                                collection: other.clone(),
                                record,
                            };
                            core.hooks
                                .on_record_delete
                                .trigger(Some(&other), e, |e| async { Ok(e) })
                                .await?;
                        }
                    }
                    let hook = &hooks.on_record_delete_execute;
                    let cascade = other.clone();
                    let (visited, changes) = (&mut *visited, &mut *changes);
                    execute_hook(core, hook, &other, current.clone(), |_| async move {
                        delete_with_references(
                            core,
                            tx,
                            registry,
                            cascade,
                            ref_id,
                            depth + 1,
                            visited,
                            changes,
                        )
                        .await?;
                        Ok(((), current))
                    })
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
            let other_fields = all_fields(&other);
            let returning = select_list(&other_fields);
            let update = if field.is_multi() {
                format!(
                    "UPDATE {table} AS {ALIAS} SET {column} = {column} - $1::text WHERE {matches} RETURNING {returning}"
                )
            } else {
                format!(
                    "UPDATE {table} AS {ALIAS} SET {column} = '' WHERE {matches} RETURNING {returning}"
                )
            };
            for row in tx.query(&update, &[&id]).await.map_err(db_error)? {
                let values = row_values(&other_fields, &row);
                changes.push(Change {
                    collection: other.clone(),
                    action: Action::Update,
                    values,
                });
            }
        }

        let fields = all_fields(&c);
        let deleted = tx
            .query_opt(
                &format!(
                    "DELETE FROM {} AS {ALIAS} WHERE {ALIAS}.\"id\" = $1::text RETURNING {}",
                    table_ref(&c.name),
                    select_list(&fields)
                ),
                &[&id],
            )
            .await
            .map_err(db_error)?;
        if let Some(row) = deleted {
            let values = row_values(&fields, &row);
            changes.push(Change {
                collection: c.clone(),
                action: Action::Delete,
                values,
            });
            if c.is_auth() {
                tx.execute(
                    &format!(
                        r#"DELETE FROM {} WHERE "collectionRef" = $1 AND "recordRef" = $2"#,
                        crate::collections::system::EXTERNAL_AUTHS_TABLE
                    ),
                    &[&c.id, &id],
                )
                .await
                .map_err(db_error)?;
                tx.execute(
                    "DELETE FROM _slonbase.otps WHERE collection_id = $1 AND record_id = $2",
                    &[&c.id, &id],
                )
                .await
                .map_err(db_error)?;
                tx.execute(
                    "DELETE FROM _slonbase.mfas WHERE collection_id = $1 AND record_id = $2",
                    &[&c.id, &id],
                )
                .await
                .map_err(db_error)?;
                tx.execute(
                    "DELETE FROM _slonbase.auth_origins WHERE collection_id = $1 AND record_id = $2",
                    &[&c.id, &id],
                )
                .await
                .map_err(db_error)?;
            }
        }
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
                    errors.add(
                        &field.name,
                        "validation_missing_rel_collection",
                        "Relation connection is missing or cannot be accessed.",
                    );
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
            errors.add(
                &field.name,
                "validation_missing_rel_records",
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
        errors.add("id", "validation_not_unique", "Value must be unique.");
    } else if constraint == crate::collections::schema::email_index_name(&c.id) {
        errors.add("email", "validation_not_unique", "Value must be unique.");
    } else if constraint == crate::collections::schema::token_key_index_name(&c.id) {
        errors.add("tokenKey", "validation_not_unique", "Value must be unique.");
    } else {
        for raw in &c.indexes {
            if let Ok(index) = Index::parse(raw) {
                if index.name == constraint {
                    for (column, _) in &index.columns {
                        errors.add(
                            column.clone(),
                            "validation_not_unique",
                            "Value must be unique.",
                        );
                    }
                }
            }
        }
    }
    if errors.is_empty() {
        errors.add("id", "validation_not_unique", "Value must be unique.");
    }
    ApiError::invalid_data(message, &errors)
}

/// `DELETE /api/collections/{collection}/truncate` (superusers): deletes every record,
/// with the same cascades, realtime events and file cleanup as deleting them one by one.
pub async fn truncate(
    State(app): State<Arc<App>>,
    Path(collection): Path<String>,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let registry = core.registry();
    let c = find_collection(&registry, &collection)?;
    if c.id == crate::collections::model::SUPERUSERS_COLLECTION_ID {
        return Err(ApiError::bad_request(
            "The superusers collection can't be truncated.",
        ));
    }
    if c.is_view() {
        return Err(ApiError::bad_request("View collections are read-only."));
    }

    let mut client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(describe_pool_error(e)))?;
    let tx = client.transaction().await.map_err(db_error)?;
    let ids: Vec<String> = tx
        .query(
            &format!("SELECT id FROM {} FOR UPDATE", table_ref(&c.name)),
            &[],
        )
        .await
        .map_err(db_error)?
        .iter()
        .map(|row| row.get(0))
        .collect();
    let mut visited = HashSet::new();
    let mut changes = Vec::new();
    for id in ids {
        delete_with_references(
            &core,
            &tx,
            &registry,
            c.clone(),
            id,
            0,
            &mut visited,
            &mut changes,
        )
        .await?;
    }
    tx.commit().await.map_err(db_error)?;
    Hooks::committed(&core, changes);

    for (collection_id, record_id) in &visited {
        if let Err(err) = core
            .storage()
            .delete_prefix(&files::record_prefix(collection_id, record_id))
            .await
        {
            tracing::warn!(error = %format!("{err:#}"), "failed to delete record files");
        }
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}
