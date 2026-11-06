//! The dashboard's SQL console (`/api/sql`) and cron jobs (`/api/crons`).
//!
//! The console runs SQL a superuser enters, with the database role's privileges. On PostgreSQL
//! that can reach beyond slonbase's data (a superuser role can read server files or run
//! programs with `COPY ... PROGRAM`), so it's off unless the server is started with
//! `--sql-console`, and it only reads unless it's also started with `--sql-console-writes`:
//!
//! - **Read-only** (the default): one statement, which must be a read (`SELECT`, `WITH`, `SHOW`,
//!   `EXPLAIN`, `VALUES`, `TABLE`), in a `READ ONLY` transaction. The statement is prepared
//!   first, which PostgreSQL refuses for more than one, so `SELECT 1; COMMIT; DELETE ...` can't
//!   end the transaction and write.
//! - **Writes**: any statement or script, in one transaction that commits.
//!
//! Either way the rows come through a cursor, at most `maxRows` (5,000) and 16 MB of them, so a
//! large table is never loaded whole; statements stop after 30 s (5 s waiting for a lock); two
//! console queries run at a time per process; and each query gets a connection of its own that
//! is closed afterwards, so nothing it sets (`SET`, temporary tables, cursors) outlives it.

use std::sync::{Arc, LazyLock};
use std::time::Instant;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value, json};
use tokio_postgres::SimpleQueryMessage;

use crate::app::App;
use crate::auth::RequestAuth;
use crate::error::ApiError;
use crate::extract::JsonObject;

/// Rows a query returns unless it asks for fewer or more (`maxRows`).
const DEFAULT_ROWS: usize = 1000;
/// The most rows a query can ask for.
const MAX_ROWS: usize = 5000;
/// The most bytes of values a result holds; past it, the rows stop there (`truncated`).
const MAX_RESULT_BYTES: usize = 16 << 20;
const MAX_QUERY_LENGTH: usize = 20_000;
const STATEMENT_TIMEOUT_SECONDS: u32 = 30;
const LOCK_TIMEOUT_SECONDS: u32 = 5;
/// Statements the read-only console runs.
const READ_KEYWORDS: &[&str] = &["SELECT", "WITH", "SHOW", "EXPLAIN", "VALUES", "TABLE"];
/// Reads whose rows come through a cursor (`SHOW` and `EXPLAIN` can't be declared as one, and
/// answer a few rows anyway).
const CURSOR_KEYWORDS: &[&str] = &["SELECT", "WITH", "VALUES", "TABLE"];
const CURSOR_PREFIX: &str = "DECLARE slonbase_console NO SCROLL CURSOR FOR ";

/// Console queries running at once in this process.
static RUNNING: LazyLock<tokio::sync::Semaphore> = LazyLock::new(|| tokio::sync::Semaphore::new(2));

fn disabled() -> ApiError {
    ApiError::forbidden(
        "The SQL console is disabled. Start slonbase with --sql-console \
         (or SLONBASE_SQL_CONSOLE=true) to enable it.",
    )
}

/// `GET /api/sql`: whether the console is on, whether it may write, and its limits.
pub async fn info(State(app): State<Arc<App>>, auth: RequestAuth) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    Ok(Json(json!({
        "enabled": core.sql_console,
        "writes": core.sql_console && core.sql_console_writes,
        "defaultRows": DEFAULT_ROWS,
        "maxRows": MAX_ROWS,
        "maxQueryLength": MAX_QUERY_LENGTH,
        "timeoutSeconds": STATEMENT_TIMEOUT_SECONDS,
        "readStatements": READ_KEYWORDS,
    })))
}

/// The first keyword of a statement, upper-cased, after leading spaces, comments and brackets.
fn first_keyword(sql: &str) -> String {
    let mut rest = sql;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '(');
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, r)| r);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, r)| r);
        } else {
            break;
        }
    }
    rest.chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Line and column (from 1) of a character position (from 1) in `sql`.
fn line_column(sql: &str, position: usize) -> (usize, usize) {
    let before: String = sql.chars().take(position.saturating_sub(1)).collect();
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

/// PostgreSQL's error as the console shows it: the message, where in the query, and its hint.
/// `offset` is how many characters were put before the query (the cursor's declaration).
fn query_error(e: &tokio_postgres::Error, sql: &str, offset: usize) -> ApiError {
    let Some(db) = e.as_db_error() else {
        return ApiError::bad_request(format!("Failed to execute query. Raw error:\n{e}"));
    };
    let mut message = format!("Failed to execute query. Raw error:\n{}", db.message());
    let mut data = serde_json::Map::new();
    if let Some(tokio_postgres::error::ErrorPosition::Original(position)) = db.position() {
        let position = (*position as usize).saturating_sub(offset);
        if position > 0 {
            let (line, column) = line_column(sql, position);
            message.push_str(&format!(" (line {line}, column {column})"));
            data.insert("line".into(), json!(line));
            data.insert("column".into(), json!(column));
        }
    }
    if let Some(hint) = db.hint() {
        message.push_str(&format!("\nHint: {hint}"));
    }
    data.insert("code".into(), json!(db.code().code()));
    let mut err = ApiError::bad_request(message);
    err.data = data;
    err
}

/// The rows of a simple-protocol answer: the last statement's columns and rows, stopped at
/// `max_rows` and `MAX_RESULT_BYTES` (`truncated`), and the rows the last command affected.
struct Collected {
    columns: Vec<String>,
    rows: Vec<Value>,
    truncated: bool,
    affected: u64,
}

fn collect(messages: Vec<SimpleQueryMessage>, max_rows: usize) -> Collected {
    let mut out = Collected {
        columns: vec![],
        rows: vec![],
        truncated: false,
        affected: 0,
    };
    let mut bytes = 0usize;
    for message in messages {
        match message {
            SimpleQueryMessage::RowDescription(description) => {
                // the last statement's result is the one shown
                out.rows.clear();
                out.truncated = false;
                bytes = 0;
                out.columns = description.iter().map(|c| c.name().to_string()).collect();
            }
            SimpleQueryMessage::Row(row) => {
                if out.truncated {
                    continue;
                }
                let size: usize = (0..row.len()).map(|i| row.get(i).map_or(4, str::len)).sum();
                if out.rows.len() >= max_rows || bytes + size > MAX_RESULT_BYTES {
                    out.truncated = true;
                    continue;
                }
                bytes += size;
                out.rows.push(Value::Array(
                    (0..row.len())
                        .map(|i| row.get(i).map_or(Value::Null, |v| Value::String(v.into())))
                        .collect(),
                ));
            }
            SimpleQueryMessage::CommandComplete(count) => out.affected = count,
            _ => {}
        }
    }
    out
}

/// `POST /api/sql` (`{"query", "maxRows"?}`), superusers only.
pub async fn run(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    if !core.sql_console {
        return Err(disabled());
    }
    let query = body
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if query.is_empty() || query.chars().count() > MAX_QUERY_LENGTH {
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
    let max_rows = body
        .get("maxRows")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_ROWS, |n| (n as usize).clamp(1, MAX_ROWS));
    let writes = core.sql_console_writes;
    let keyword = first_keyword(&query);
    let is_read = READ_KEYWORDS.contains(&keyword.as_str());
    if !writes && !is_read {
        return Err(ApiError::forbidden(format!(
            "The SQL console is read-only: it runs {} statements. Start slonbase with \
             --sql-console-writes (or SLONBASE_SQL_CONSOLE_WRITES=true) to change data.",
            READ_KEYWORDS.join(", ")
        )));
    }
    let Ok(_permit) = RUNNING.try_acquire() else {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Two console queries are already running. Try again when one finishes.",
        ));
    };

    let mut client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let outcome = run_on(&mut client, &core, &query, &keyword, writes, max_rows).await;
    // the query may have changed the session (SET, temporary tables): close the connection
    // rather than hand it to another request
    let _ = deadpool_postgres::Object::take(client);
    outcome.map(Json)
}

async fn run_on(
    client: &mut deadpool_postgres::Object,
    core: &crate::core::Core,
    query: &str,
    keyword: &str,
    writes: bool,
    max_rows: usize,
) -> Result<Value, ApiError> {
    let failed = |e: tokio_postgres::Error| query_error(&e, query, 0);
    client
        .batch_execute(&format!(
            "SET search_path = slonbase, public; \
             SET statement_timeout = '{STATEMENT_TIMEOUT_SECONDS}s'; \
             SET lock_timeout = '{LOCK_TIMEOUT_SECONDS}s'"
        ))
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    // one statement or several: PostgreSQL prepares only one (and the columns' types come with it)
    let statement = match client.prepare(query).await {
        Ok(statement) => Some(statement),
        Err(e) => {
            let several = e
                .as_db_error()
                .is_some_and(|d| d.message().contains("multiple commands"));
            if !several {
                return Err(failed(e));
            }
            if !writes {
                return Err(ApiError::forbidden(
                    "The SQL console is read-only: it runs one statement at a time. Start \
                     slonbase with --sql-console-writes (or SLONBASE_SQL_CONSOLE_WRITES=true) to \
                     run scripts.",
                ));
            }
            None
        }
    };
    let read_only = !writes || (statement.is_some() && READ_KEYWORDS.contains(&keyword));
    let types: Vec<String> = statement.as_ref().map_or(vec![], |s| {
        s.columns()
            .iter()
            .map(|c| c.type_().name().to_string())
            .collect()
    });

    let mut read_only = read_only;
    let mut through_cursor = statement.is_some() && CURSOR_KEYWORDS.contains(&keyword);
    let started = Instant::now();
    let collected = loop {
        let tx = client
            .build_transaction()
            .read_only(read_only)
            .start()
            .await
            .map_err(failed)?;
        let collected = if through_cursor {
            // through a cursor, so only the rows shown are read (the query is one statement:
            // the declaration can't carry another)
            let body = query.trim_end().trim_end_matches(';');
            if let Err(e) = tx.batch_execute(&format!("{CURSOR_PREFIX}{body}")).await {
                // `WITH x AS (DELETE ...) SELECT`: a write, which a cursor can't hold
                let modifies = e.as_db_error().is_some_and(|d| {
                    d.code() == &tokio_postgres::error::SqlState::FEATURE_NOT_SUPPORTED
                        && d.message().contains("data-modifying")
                });
                if !modifies {
                    return Err(query_error(&e, query, CURSOR_PREFIX.chars().count()));
                }
                tx.rollback().await.map_err(failed)?;
                if !writes {
                    return Err(ApiError::forbidden(
                        "The SQL console is read-only: this statement changes data. Start \
                         slonbase with --sql-console-writes (or SLONBASE_SQL_CONSOLE_WRITES=true) \
                         to change data.",
                    ));
                }
                (read_only, through_cursor) = (false, false);
                continue;
            }
            let fetched = tx
                .simple_query(&format!(
                    "FETCH FORWARD {} FROM slonbase_console",
                    max_rows + 1
                ))
                .await
                .map_err(failed)?;
            collect(fetched, max_rows)
        } else {
            collect(tx.simple_query(query).await.map_err(failed)?, max_rows)
        };
        if read_only {
            tx.rollback().await.map_err(failed)?;
        } else {
            tx.commit().await.map_err(failed)?;
            // a write, perhaps a TRUNCATE, which the realtime triggers don't see
            core.data_changed_everywhere(client).await;
        }
        break collected;
    };
    let exec_time = started.elapsed().as_millis() as u64;
    let columns: Vec<Value> = collected
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            json!({ "name": name, "type": types.get(i).cloned().unwrap_or_default(), "nullable": true })
        })
        .collect();
    Ok(json!({
        "execTime": exec_time,
        "affectedRows": if read_only { 0 } else { collected.affected },
        "columns": columns,
        "rows": collected.rows,
        "truncated": collected.truncated,
        "maxRows": max_rows,
        "readOnly": read_only,
    }))
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
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let mut last = crate::jobs::last_runs(&client, crate::jobs::CRON)
        .await
        .map_err(ApiError::internal)?;
    let backups = crate::jobs::last_runs(&client, crate::jobs::BACKUP)
        .await
        .map_err(ApiError::internal)?;
    let now = chrono::Utc::now();
    let describe = |id: &str, expression: &str, kind: &str, last: Option<Value>| {
        let next = crate::cron::Schedule::parse(expression)
            .and_then(|s| s.next_after(&now))
            .map(|t| crate::datetime::format(&t));
        json!({
            "id": id,
            "expression": expression,
            "description": crate::cron::Schedule::describe(expression),
            "next": next,
            "kind": kind,
            "lastRun": last,
        })
    };
    let cron = core.settings().backups.cron.trim().to_string();
    let mut jobs = Vec::new();
    if !cron.is_empty() {
        jobs.push(describe(
            AUTO_BACKUP_JOB,
            &cron,
            "backup",
            backups.get("auto").cloned(),
        ));
    }
    for job in core.hooks.crons() {
        jobs.push(describe(
            &job.id,
            &job.expression,
            "hook",
            last.remove(&job.id),
        ));
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
        tokio::spawn(crate::hooks::run_cron_recorded(core, id, run, "manual"));
        return Ok(axum::http::StatusCode::NO_CONTENT.into_response());
    }
    if id != AUTO_BACKUP_JOB || core.settings().backups.cron.trim().is_empty() {
        return Err(ApiError::not_found());
    }
    crate::backups::run_auto_backup_recorded(&core, "manual").await?;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_keywords() {
        assert_eq!(first_keyword("select 1"), "SELECT");
        assert_eq!(
            first_keyword("  -- a note\n  /* and */ (SELECT 1)"),
            "SELECT"
        );
        assert_eq!(
            first_keyword("with x as (select 1) select * from x"),
            "WITH"
        );
        assert_eq!(first_keyword("/* unterminated"), "");
        assert_eq!(first_keyword("delete from t"), "DELETE");
    }

    #[test]
    fn line_columns() {
        assert_eq!(line_column("SELECT x", 8), (1, 8));
        assert_eq!(line_column("SELECT\n  *\nFROM nope", 17), (3, 6));
    }
}
