//! The dashboard's SQL console (`POST /api/sql`) and cron jobs list (`/api/crons`).
//!
//! The console runs any SQL a superuser enters, with the database role's privileges. On
//! PostgreSQL that can reach beyond slonbase's data (a superuser role can read server
//! files or run programs with `COPY ... PROGRAM`), so it's off unless the server is started
//! with `--sql-console`.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value, json};
use tokio_postgres::SimpleQueryMessage;

use crate::app::App;
use crate::auth::RequestAuth;
use crate::error::ApiError;
use crate::extract::JsonObject;

const MAX_ROWS: usize = 1000;
const MAX_QUERY_LENGTH: usize = 5000;
const TIMEOUT: &str = "180s";
/// Queries starting with these run in a read-only transaction.
const READ_PREFIXES: &[&str] = &["SELECT", "WITH", "SHOW", "EXPLAIN", "VALUES", "TABLE"];

/// `POST /api/sql` (`{"query"}`), superusers only.
pub async fn run(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    if !core.sql_console {
        return Err(ApiError::forbidden(
            "The SQL console is disabled. Start slonbase with --sql-console \
             (or SLONBASE_SQL_CONSOLE=true) to enable it.",
        ));
    }
    let query = body
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if query.is_empty() || query.len() > MAX_QUERY_LENGTH {
        let mut errors = crate::validation::Errors::new();
        errors.invalid(
            "query",
            format!("Must be between 1 and {MAX_QUERY_LENGTH} characters."),
        );
        return Err(ApiError::invalid_data(
            "An error occurred while validating the submitted data.",
            &errors,
        ));
    }
    let upper = query.to_ascii_uppercase();
    let read_only = READ_PREFIXES.iter().any(|p| upper.starts_with(p));

    let failed = |e: tokio_postgres::Error| {
        let detail = e
            .as_db_error()
            .map(|d| d.message().to_string())
            .unwrap_or_else(|| e.to_string());
        ApiError::bad_request(format!("Failed to execute query. Raw error:\n{detail}"))
    };
    let mut client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    // column types, when the query is a single statement (prepared before the transaction,
    // which a failed prepare would abort)
    let types: Vec<String> = match client.prepare(&query).await {
        Ok(statement) => statement
            .columns()
            .iter()
            .map(|c| c.type_().name().to_string())
            .collect(),
        Err(_) => vec![],
    };
    let tx = client
        .build_transaction()
        .read_only(read_only)
        .start()
        .await
        .map_err(failed)?;
    tx.batch_execute(&format!("SET LOCAL statement_timeout = '{TIMEOUT}'"))
        .await
        .map_err(failed)?;

    let started = Instant::now();
    let messages = tx.simple_query(&query).await.map_err(failed)?;
    let exec_time = started.elapsed().as_millis() as u64;

    let mut columns = Vec::new();
    let mut rows = Vec::new();
    let mut affected = 0u64;
    for message in messages {
        match message {
            SimpleQueryMessage::RowDescription(description) => {
                // the last statement's result is the one shown
                rows.clear();
                columns = description
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        json!({
                            "name": c.name(),
                            "type": types.get(i).cloned().unwrap_or_default(),
                            "nullable": true,
                        })
                    })
                    .collect();
            }
            SimpleQueryMessage::Row(row) if rows.len() < MAX_ROWS => {
                rows.push(Value::Array(
                    (0..row.len())
                        .map(|i| row.get(i).map_or(Value::Null, |v| Value::String(v.into())))
                        .collect(),
                ));
            }
            SimpleQueryMessage::CommandComplete(count) => affected = count,
            _ => {}
        }
    }
    if read_only {
        tx.rollback().await.map_err(failed)?;
    } else {
        tx.commit().await.map_err(failed)?;
    }
    Ok(Json(json!({
        "execTime": exec_time,
        "affectedRows": if read_only { 0 } else { affected },
        "columns": columns,
        "rows": rows,
    })))
}

/// `GET /api/crons`: the registered cron jobs (none yet).
pub async fn crons(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    app.core()?;
    auth.require_superuser()?;
    Ok(Json(json!([])))
}

/// `POST /api/crons/{id}`: runs a cron job.
pub async fn run_cron(
    State(app): State<Arc<App>>,
    Path(_id): Path<String>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    app.core()?;
    auth.require_superuser()?;
    Err(ApiError::not_found())
}
