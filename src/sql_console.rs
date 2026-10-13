//! The dashboard's SQL console (`POST /api/sql`) and cron jobs (`/api/crons`).
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

/// The automatic backup job's id (PocketBase's).
const AUTO_BACKUP_JOB: &str = "__pbAutoBackup__";

/// `GET /api/crons`: the scheduled jobs (the automatic backup, when configured).
pub async fn crons(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let cron = core.settings().backups.cron.trim().to_string();
    let mut jobs = Vec::new();
    if !cron.is_empty() {
        jobs.push(json!({ "id": AUTO_BACKUP_JOB, "expression": cron }));
    }
    for job in core.hooks.crons() {
        jobs.push(json!({ "id": job.id, "expression": job.expression }));
    }
    Ok(Json(Value::Array(jobs)))
}

/// `POST /api/crons/{id}`: runs a job now.
pub async fn run_cron(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    auth: RequestAuth,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    let core = app.core()?;
    auth.require_superuser()?;
    if let Some(job) = core.hooks.crons().iter().find(|j| j.id == id) {
        // in the background, like PocketBase
        let (run, core) = (job.run.clone(), core.clone());
        tokio::spawn(async move {
            if let Err(err) = run(core).await {
                tracing::error!(job = %id, error = %err.details.as_deref().unwrap_or(&err.message), "a cron job failed");
            }
        });
        return Ok(axum::http::StatusCode::NO_CONTENT.into_response());
    }
    if id != AUTO_BACKUP_JOB || core.settings().backups.cron.trim().is_empty() {
        return Err(ApiError::not_found());
    }
    crate::backups::run_auto_backup(&core).await?;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}
