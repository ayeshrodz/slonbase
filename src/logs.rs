//! Logs, in PocketBase's format: one entry per request (level 0, or 8 for errors) and the
//! app's warnings and errors, kept for the settings' `logs.maxDays`.
//!
//! Entries are queued in memory and written by a background task in batches (a single
//! `INSERT ... SELECT unnest(...)`), into an unlogged table, so logging adds no database
//! round trip to requests. When the queue is full (the database is down or slow), new
//! entries are dropped rather than slowing requests down.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::app::App;
use crate::auth::RequestAuth;
use crate::collections::model::Collection;
use crate::error::{ApiError, ErrorInfo};
use crate::filter::{Params, RequestInfo, Scope, compile_sort, to_sql};

pub const LEVEL_DEBUG: i32 = -4;
pub const LEVEL_INFO: i32 = 0;
pub const LEVEL_WARN: i32 = 4;
pub const LEVEL_ERROR: i32 = 8;

const QUEUE: usize = 10_000;
/// Longest message kept, in bytes (as PocketBase).
const MAX_MESSAGE: usize = 8000;
/// The `data` limit when `logs.maxDataSize` is 0 (as PocketBase).
const DEFAULT_MAX_DATA: usize = 16 << 10;
/// Marks an entry whose `data` was cut (PocketBase's name, which its tools recognize).
pub const TRUNCATED: &str = "__pb_truncated__";
const BATCH: usize = 500;
const FLUSH_EVERY: Duration = Duration::from_millis(500);
const ALIAS: &str = "t";

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub created: DateTime<Utc>,
    pub level: i32,
    pub message: String,
    pub data: Value,
}

impl Entry {
    pub fn new(level: i32, message: impl Into<String>, data: Value) -> Self {
        Self {
            id: crate::ids::random_id(crate::ids::DEFAULT_ID_LENGTH),
            created: Utc::now(),
            level,
            message: message.into(),
            data,
        }
    }
}

/// The queue of entries waiting to be written.
#[derive(Clone)]
pub struct Logger {
    sender: mpsc::Sender<Entry>,
    /// `logs.maxDataSize`
    max_data_size: Arc<AtomicI64>,
}

impl Logger {
    /// Starts the writer task.
    pub fn start(pool: Pool, max_data_size: i64) -> Self {
        let (sender, receiver) = mpsc::channel(QUEUE);
        tokio::spawn(write_loop(pool, receiver));
        Self {
            sender,
            max_data_size: Arc::new(AtomicI64::new(max_data_size)),
        }
    }

    /// Applies a changed `logs.maxDataSize`.
    pub fn set_max_data_size(&self, max_data_size: i64) {
        self.max_data_size.store(max_data_size, Ordering::Relaxed);
    }

    pub fn push(&self, mut entry: Entry) {
        let limit = match self.max_data_size.load(Ordering::Relaxed) {
            n if n <= 0 => DEFAULT_MAX_DATA,
            n => usize::try_from(n).unwrap_or(usize::MAX),
        };
        fit(&mut entry, limit);
        // full: dropped (see the module docs)
        let _ = self.sender.try_send(entry);
    }
}

/// Keeps an entry within the limits, as PocketBase does: the message to [`MAX_MESSAGE`]
/// bytes, and `data` serialized to `max_data` bytes, by keeping the attributes that fit
/// whole, in order, and adding [`TRUNCATED`].
fn fit(entry: &mut Entry, max_data: usize) {
    if entry.message.len() > MAX_MESSAGE {
        let mut end = MAX_MESSAGE;
        while !entry.message.is_char_boundary(end) {
            end -= 1;
        }
        entry.message.truncate(end);
    }
    let size = serde_json::to_vec(&entry.data).map_or(0, |json| json.len());
    if size <= max_data {
        return;
    }
    let mut kept = Map::new();
    if let Value::Object(data) = std::mem::take(&mut entry.data) {
        // "{" and, between attributes, ","
        let mut size = 1;
        for (key, value) in data {
            let attribute = serde_json::to_vec(&key).map_or(0, |k| k.len())
                + 1
                + serde_json::to_vec(&value).map_or(0, |v| v.len())
                + usize::from(!kept.is_empty());
            if size + attribute > max_data {
                break;
            }
            size += attribute;
            kept.insert(key, value);
        }
    }
    kept.insert(TRUNCATED.into(), Value::Bool(true));
    entry.data = Value::Object(kept);
}

async fn write_loop(pool: Pool, mut receiver: mpsc::Receiver<Entry>) {
    while let Some(first) = receiver.recv().await {
        let mut batch = vec![first];
        let deadline = tokio::time::Instant::now() + FLUSH_EVERY;
        while batch.len() < BATCH {
            tokio::select! {
                entry = receiver.recv() => match entry {
                    Some(entry) => batch.push(entry),
                    None => break,
                },
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        if let Err(err) = write(&pool, &batch).await {
            // not through tracing at warn level, which would log it again
            eprintln!(
                "slonbase: failed to write {} log entries: {err:#}",
                batch.len()
            );
        }
    }
}

async fn write(pool: &Pool, batch: &[Entry]) -> anyhow::Result<()> {
    let client = pool.get().await.map_err(crate::db::describe_pool_error)?;
    let ids: Vec<&str> = batch.iter().map(|e| e.id.as_str()).collect();
    let created: Vec<DateTime<Utc>> = batch.iter().map(|e| e.created).collect();
    let levels: Vec<i32> = batch.iter().map(|e| e.level).collect();
    let messages: Vec<&str> = batch.iter().map(|e| e.message.as_str()).collect();
    let data: Vec<&Value> = batch.iter().map(|e| &e.data).collect();
    client
        .execute(
            "INSERT INTO _slonbase.logs (id, created, level, message, data)
             SELECT * FROM unnest($1::text[], $2::timestamptz[], $3::int4[], $4::text[], $5::jsonb[])
             ON CONFLICT (id) DO NOTHING",
            &[&ids, &created, &levels, &messages, &data],
        )
        .await?;
    Ok(())
}

/// Deletes entries older than `max_days`.
pub async fn cleanup(pool: &Pool, max_days: u32) -> anyhow::Result<()> {
    let client = pool.get().await.map_err(crate::db::describe_pool_error)?;
    client
        .execute(
            "DELETE FROM _slonbase.logs WHERE created < now() - $1::int * interval '1 day'",
            &[&(max_days.max(1) as i32)],
        )
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// request logs

/// Filled in while a request is handled (by the auth extractor).
#[derive(Debug, Clone, Default)]
pub struct LogContext(Arc<Mutex<Caller>>);

#[derive(Debug, Clone, Default)]
struct Caller {
    collection: String,
    id: String,
}

impl LogContext {
    pub fn set_auth(&self, auth: &RequestAuth) {
        if let Some(record) = auth.any_record() {
            let mut caller = self.0.lock().expect("log context lock");
            caller.collection = record.collection.name.clone();
            caller.id = record.id().to_string();
        }
    }
}

/// Middleware logging each request once its response is ready.
pub async fn middleware(State(app): State<Arc<App>>, mut request: Request, next: Next) -> Response {
    let Ok(core) = app.core() else {
        return next.run(request).await;
    };
    let settings = core.settings();
    if settings.logs.max_days == 0 {
        return next.run(request).await;
    }

    let started = Instant::now();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let url = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| path.clone());
    let (user_agent, referer) = {
        // scoped: a reference to the request can't be held across the await below
        let header = |name: header::HeaderName| -> String {
            request
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        (header(header::USER_AGENT), header(header::REFERER))
    };
    let remote_ip = crate::ip::remote_ip(request.extensions());
    let user_ip = crate::ip::user_ip(request.headers(), remote_ip, &settings.trusted_proxy);
    let context = LogContext::default();
    request.extensions_mut().insert(context.clone());

    let response = next.run(request).await;

    let status = response.status();
    // static dashboard files and successful health checks would mostly be noise
    if status.is_success() && (path.starts_with("/_/") || path == "/api/health" || path == "/") {
        return response;
    }
    let level = if status.as_u16() >= 400 {
        LEVEL_ERROR
    } else {
        LEVEL_INFO
    };
    if level < settings.logs.min_level {
        return response;
    }

    let caller = context.0.lock().expect("log context lock").clone();
    let mut data = Map::new();
    data.insert("type".into(), "request".into());
    data.insert("method".into(), method.clone().into());
    data.insert("url".into(), url.clone().into());
    data.insert("status".into(), status.as_u16().into());
    data.insert(
        "execTime".into(),
        json!((started.elapsed().as_secs_f64() * 1e6).round() / 1e3),
    );
    data.insert("auth".into(), caller.collection.into());
    if settings.logs.log_auth_id {
        data.insert("authId".into(), caller.id.into());
    }
    let ip = |ip: Option<std::net::IpAddr>| ip.map(|ip| ip.to_string()).unwrap_or_default();
    if settings.logs.log_ip {
        data.insert("remoteIP".into(), ip(remote_ip).into());
        data.insert("userIP".into(), ip(user_ip).into());
    }
    data.insert("userAgent".into(), user_agent.into());
    data.insert("referer".into(), referer.into());
    if let Some(error) = response.extensions().get::<ErrorInfo>() {
        data.insert("error".into(), error.message.clone().into());
        if let Some(details) = &error.details {
            data.insert("details".into(), details.clone().into());
        }
    }
    core.logs.push(Entry::new(
        level,
        format!("{method} {url}"),
        Value::Object(data),
    ));
    response
}

// ---------------------------------------------------------------------------
// app logs (warnings and errors from tracing)

static APP_LOGGER: OnceLock<Mutex<Option<Logger>>> = OnceLock::new();

/// Sends the app's warnings and errors to `logger` (from now on).
pub fn capture_app_logs(logger: &Logger) {
    *APP_LOGGER
        .get_or_init(Default::default)
        .lock()
        .expect("app logger lock") = Some(logger.clone());
}

/// A `tracing` layer forwarding warnings and errors to the logs (installed by `main`).
pub struct AppLogsLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for AppLogsLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => LEVEL_ERROR,
            tracing::Level::WARN => LEVEL_WARN,
            _ => return,
        };
        let Some(logger) = APP_LOGGER
            .get()
            .and_then(|l| l.lock().ok().and_then(|l| l.clone()))
        else {
            return;
        };
        let mut visitor = Fields::default();
        event.record(&mut visitor);
        visitor
            .data
            .insert("target".into(), event.metadata().target().into());
        logger.push(Entry::new(
            level,
            visitor.message,
            Value::Object(visitor.data),
        ));
    }
}

#[derive(Default)]
struct Fields {
    message: String,
    data: Map<String, Value>,
}

impl tracing::field::Visit for Fields {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.data.insert(field.name().into(), value.into());
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let value = format!("{value:?}");
        if field.name() == "message" {
            self.message = value;
        } else {
            self.data.insert(field.name().into(), value.into());
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.data.insert(field.name().into(), value.into());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.data.insert(field.name().into(), value.into());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.data.insert(field.name().into(), value.into());
    }
}

// ---------------------------------------------------------------------------
// the API (superusers)

/// The logs table described as a collection, so filters and sorts compile like records'.
static LOGS: LazyLock<Collection> = LazyLock::new(|| {
    serde_json::from_value(json!({
        "id": "_logs",
        "name": "_logs",
        "type": "base",
        "fields": [
            { "name": "id", "type": "text" },
            { "name": "created", "type": "date" },
            { "name": "level", "type": "number" },
            { "name": "message", "type": "text" },
            { "name": "data", "type": "json" },
        ],
    }))
    .expect("valid logs collection")
});

fn bad_filter(e: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(format!("Invalid filter parameters: {e}."))
}

fn db_error(e: tokio_postgres::Error) -> ApiError {
    ApiError::internal(e.into())
}

const COLUMNS: &str = "id, to_char(created AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.MS\"Z\"'), level, message, data";

fn row_json(row: &tokio_postgres::Row) -> Value {
    json!({
        "id": row.get::<_, String>(0),
        "created": row.get::<_, String>(1),
        "level": row.get::<_, i32>(2),
        "message": row.get::<_, String>(3),
        "data": row.get::<_, Value>(4),
    })
}

/// The WHERE clause of a `filter` query parameter.
fn filter_sql(query: &HashMap<String, String>, params: &mut Params) -> Result<String, ApiError> {
    let Some(filter) = query.get("filter").filter(|f| !f.trim().is_empty()) else {
        return Ok("TRUE".into());
    };
    let request = RequestInfo::new("GET");
    let scope = Scope {
        collection: &LOGS,
        alias: ALIAS,
        request: &request,
        allow_hidden: true,
    };
    to_sql(filter, &scope, params).map_err(bad_filter)
}

/// `GET /api/logs` (`page`, `perPage`, `sort`, `filter`, `skipTotal`)
pub async fn list(
    State(app): State<Arc<App>>,
    Query(query): Query<HashMap<String, String>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let page = query
        .get("page")
        .and_then(|p| p.parse::<i64>().ok())
        .filter(|p| *p >= 1)
        .unwrap_or(1);
    let per_page = query
        .get("perPage")
        .and_then(|p| p.parse::<i64>().ok())
        .filter(|p| *p >= 1)
        .unwrap_or(30)
        .min(500);
    let skip_total = query
        .get("skipTotal")
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "t"));

    let mut params = Params::default();
    let condition = filter_sql(&query, &mut params)?;
    let sort = query
        .get("sort")
        .cloned()
        .unwrap_or_else(|| "-@rowid".into());
    let order = compile_sort(&sort, &LOGS, ALIAS, true).map_err(bad_filter)?;

    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let rows = client
        .query(
            &format!(
                "SELECT {COLUMNS} FROM _slonbase.logs {ALIAS} WHERE {condition} ORDER BY {} LIMIT {per_page} OFFSET {}",
                order.join(", "),
                (page - 1).saturating_mul(per_page)
            ),
            &params.as_refs(),
        )
        .await
        .map_err(db_error)?;
    let items: Vec<Value> = rows.iter().map(row_json).collect();

    let (total_items, total_pages) = if skip_total {
        (-1, -1)
    } else {
        let total: i64 = client
            .query_one(
                &format!("SELECT count(*) FROM _slonbase.logs {ALIAS} WHERE {condition}"),
                &params.as_refs(),
            )
            .await
            .map_err(db_error)?
            .get(0);
        (total, (total + per_page - 1) / per_page)
    };
    Ok(Json(json!({
        "page": page,
        "perPage": per_page,
        "totalItems": total_items,
        "totalPages": total_pages,
        "items": items,
    })))
}

/// `GET /api/logs/{id}`
pub async fn view(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let row = client
        .query_opt(
            &format!("SELECT {COLUMNS} FROM _slonbase.logs WHERE id = $1"),
            &[&id],
        )
        .await
        .map_err(db_error)?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(row_json(&row)))
}

/// `GET /api/logs/stats` (`filter`): entries per hour.
pub async fn stats(
    State(app): State<Arc<App>>,
    Query(query): Query<HashMap<String, String>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let mut params = Params::default();
    let condition = filter_sql(&query, &mut params)?;
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let rows = client
        .query(
            &format!(
                // truncated in UTC, whatever the session's time zone
                "SELECT to_char(date_trunc('hour', {ALIAS}.created AT TIME ZONE 'UTC'),
                                'YYYY-MM-DD HH24:00:00.000\"Z\"'),
                        count(*)
                 FROM _slonbase.logs {ALIAS} WHERE {condition} GROUP BY 1 ORDER BY 1"
            ),
            &params.as_refs(),
        )
        .await
        .map_err(db_error)?;
    Ok(Json(Value::Array(
        rows.iter()
            .map(|row| json!({ "date": row.get::<_, String>(0), "total": row.get::<_, i64>(1) }))
            .collect(),
    )))
}

/// `DELETE /api/logs`: deletes all entries.
pub async fn truncate(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    client
        .batch_execute("TRUNCATE _slonbase.logs")
        .await
        .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_fit_the_limits() {
        let mut entry = Entry::new(
            0,
            "é".repeat(5000),
            json!({ "a": 1, "b": "x".repeat(50), "c": 3 }),
        );
        fit(&mut entry, 1000);
        assert_eq!(entry.message.len(), 8000, "cut on a character boundary");
        assert_eq!(
            entry.data,
            json!({ "a": 1, "b": "x".repeat(50), "c": 3 }),
            "within the limit"
        );

        // the attributes that fit whole, in order: `{"a":1` and `,"b":"x..."`
        let both = r#"{"a":1,"b":""#.len() + 50 + 1;
        for (limit, expected) in [
            (
                both,
                json!({ "a": 1, "b": "x".repeat(50), TRUNCATED: true }),
            ),
            (both - 1, json!({ "a": 1, TRUNCATED: true })),
        ] {
            let mut entry = Entry::new(0, "m", json!({ "a": 1, "b": "x".repeat(50), "c": 3 }));
            fit(&mut entry, limit);
            assert_eq!(entry.data, expected, "{limit}");
        }
        let mut entry = Entry::new(0, "m", json!({ "big": "x".repeat(100) }));
        fit(&mut entry, 10);
        assert_eq!(entry.data, json!({ TRUNCATED: true }));
    }
}
