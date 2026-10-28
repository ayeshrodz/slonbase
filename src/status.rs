//! `GET /api/system/status` (superusers): the installation's health in one response, for the
//! dashboard's Health page: this instance, the others sharing the database, PostgreSQL, the
//! file storage, backups, recent errors, and checks with what to do about them.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::app::App;
use crate::auth::RequestAuth;
use crate::error::ApiError;
use crate::storage::Storage;

/// A check: `ok`, `info`, `warn` or `error`, with what it found and where to fix it.
fn check(id: &str, status: &str, title: &str, detail: impl Into<String>, link: &str) -> Value {
    json!({ "id": id, "status": status, "title": title, "detail": detail.into(), "link": link })
}

/// This process's resident memory, from `/proc` (Linux), in bytes.
fn memory() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

fn db_error(e: tokio_postgres::Error) -> ApiError {
    ApiError::internal(e.into())
}

pub async fn status(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let settings = core.settings();
    let mut checks = Vec::new();

    // the database: a round trip, its version and size, the pool
    let started = Instant::now();
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    client.simple_query("SELECT 1").await.map_err(db_error)?;
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    let row = client
        .query_one(
            "SELECT current_setting('server_version'), pg_database_size(current_database()),
                    current_database(),
                    (SELECT COALESCE(max(version), 0) FROM _slonbase.migrations),
                    (SELECT count(*) FROM _slonbase.app_migrations)",
            &[],
        )
        .await
        .map_err(db_error)?;
    let (pg_version, db_size, db_name, migration, app_migrations): (String, i64, String, i32, i64) =
        (row.get(0), row.get(1), row.get(2), row.get(3), row.get(4));
    let instances: Vec<Value> = client
        .query(
            "SELECT id, to_char(seen AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.MS\"Z\"')
             FROM _slonbase.instances WHERE seen > now() - interval '60 seconds' ORDER BY id",
            &[],
        )
        .await
        .map_err(db_error)?
        .iter()
        .map(|r| {
            let id: String = r.get(0);
            json!({ "id": id, "lastSeen": r.get::<_, String>(1), "current": id == core.realtime.instance_id() })
        })
        .collect();
    let logs = client
        .query_one(
            "SELECT count(*) FILTER (WHERE data->>'type' = 'request'),
                    count(*) FILTER (WHERE level >= 8),
                    count(*) FILTER (WHERE level >= 4 AND level < 8)
             FROM _slonbase.logs WHERE created > now() - interval '24 hours'",
            &[],
        )
        .await
        .map_err(db_error)?;
    let (requests, errors, warnings): (i64, i64, i64) = (logs.get(0), logs.get(1), logs.get(2));
    drop(client);
    let pool = core.pool.status();
    let in_use = pool.size.saturating_sub(pool.available);

    checks.push(if latency_ms > 100.0 {
        check(
            "database",
            "warn",
            "The database is slow to answer",
            format!("A round trip took {latency_ms:.0} ms. Is PostgreSQL far away, or busy?"),
            "",
        )
    } else {
        check(
            "database",
            "ok",
            "Database",
            format!(
                "PostgreSQL {} answers in {latency_ms:.1} ms",
                pg_version.split(' ').next().unwrap_or_default()
            ),
            "",
        )
    });
    if pool.waiting > 0 {
        checks.push(check(
            "pool",
            "warn",
            "Requests are waiting for a database connection",
            format!(
                "{} waiting, all {} connections in use. Raise --database-max-connections, or check for slow queries.",
                pool.waiting, pool.max_size
            ),
            "#/logs",
        ));
    }

    // storage and instances
    let storage = core.storage();
    let local_files = matches!(storage.as_ref(), Storage::Local(_));
    if let Storage::Unavailable(message) = storage.as_ref() {
        checks.push(check(
            "storage",
            "error",
            "File storage is unavailable",
            message.clone(),
            "#/settings/storage",
        ));
    }
    if instances.len() > 1 && local_files {
        checks.push(check(
            "shared-storage",
            "warn",
            "Several instances store files locally",
            format!(
                "{} instances share this database. Unless they share the storage directory, a file uploaded through one isn't found by the others: use S3.",
                instances.len()
            ),
            "#/settings/storage",
        ));
    }

    // backups
    let backup_storage = crate::backups::storage(&core);
    let backups_in_s3 = matches!(backup_storage, Storage::S3(_));
    let mut backups: Vec<(String, u64, String)> = backup_storage
        .list_root()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|(key, _, _)| crate::backups::valid_name(key))
        .collect();
    backups.sort_by(|a, b| b.2.cmp(&a.2));
    let last = backups.first();
    let encrypted = core.backup_key.is_some();
    match last {
        None => checks.push(check(
            "backups",
            "warn",
            "No backups yet",
            if settings.backups.cron.trim().is_empty() {
                "Create one, and schedule them so there's always a recent one."
            } else {
                "The schedule hasn't made one yet."
            },
            "#/settings/backups",
        )),
        Some((_, _, modified)) => {
            let age = chrono::DateTime::parse_from_rfc3339(modified)
                .or_else(|_| {
                    chrono::DateTime::parse_from_str(
                        &format!("{modified} +0000"),
                        "%Y-%m-%d %H:%M:%S%.fZ %z",
                    )
                })
                .map(|t| chrono::Utc::now().signed_duration_since(t));
            match age {
                Ok(age) if age.num_days() >= 7 => checks.push(check(
                    "backups",
                    "warn",
                    "The last backup is old",
                    format!(
                        "It's {} days old. Schedule backups so there's always a recent one.",
                        age.num_days()
                    ),
                    "#/settings/backups",
                )),
                _ => checks.push(check(
                    "backups",
                    "ok",
                    "Backups",
                    "A recent backup exists",
                    "#/settings/backups",
                )),
            }
        }
    }
    if settings.backups.cron.trim().is_empty() {
        checks.push(check(
            "backup-schedule",
            "info",
            "Backups aren't scheduled",
            "Automatic backups (e.g. every night) keep a recent copy without anyone thinking of it.",
            "#/settings/backups",
        ));
    }
    if !backups_in_s3 && last.is_some() {
        checks.push(check(
            "backup-place",
            "info",
            "Backups are on this server",
            "A copy elsewhere (an S3 bucket) survives losing the server.",
            "#/settings/backups",
        ));
    }
    if !encrypted {
        checks.push(check(
            "backup-encryption",
            "info",
            "Backups aren't encrypted",
            "Set SLONBASE_BACKUPS_ENCRYPTION_KEY to encrypt them, so a leaked copy reveals nothing.",
            "",
        ));
    }

    // settings that matter in production
    if !settings.smtp.enabled {
        checks.push(check(
            "smtp",
            "warn",
            "Emails aren't sent",
            "SMTP is off, so verification, password reset and OTP emails are only written to the logs.",
            "#/settings/mail",
        ));
    }
    let app_url = settings.meta.app_url.trim();
    if app_url.contains("localhost") || app_url.contains("127.0.0.1") {
        checks.push(check(
            "app-url",
            "warn",
            "The application URL is a local address",
            format!("Links in emails point to {app_url}. Set the URL people use."),
            "#/settings/general",
        ));
    }
    if !settings.rate_limits.enabled {
        checks.push(check(
            "rate-limits",
            "warn",
            "Rate limits are off",
            "Nothing slows down password guessing or floods of requests.",
            "#/settings/security",
        ));
    }
    if core.sql_console {
        checks.push(check(
            "sql-console",
            "info",
            "The SQL console is on",
            "Superusers can run any SQL with the database role's privileges.",
            "#/sql",
        ));
    }
    if errors > 0 {
        checks.push(check(
            "errors",
            "warn",
            "Errors in the last 24 hours",
            format!("{errors} logged errors."),
            &format!("#/logs?filter={}", "level%20%3E%3D%208"),
        ));
    }

    let started = *crate::metrics::STARTED;
    let uptime = std::time::SystemTime::now()
        .duration_since(started)
        .unwrap_or_default()
        .as_secs();
    let started_at: chrono::DateTime<chrono::Utc> = started.into();
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "startedAt": crate::datetime::format(&started_at),
        "uptimeSeconds": uptime,
        "instance": {
            "id": core.realtime.instance_id(),
            "memoryBytes": memory(),
            "realtimeClients": core.realtime.client_count(),
            "realtimeEvents": crate::metrics::METRICS.realtime_events.load(std::sync::atomic::Ordering::Relaxed),
        },
        "instances": instances,
        "database": {
            "version": pg_version,
            "name": db_name,
            "sizeBytes": db_size,
            "latencyMs": (latency_ms * 10.0).round() / 10.0,
            "migration": migration,
            "appMigrations": app_migrations,
            "pool": { "size": pool.size, "inUse": in_use, "idle": pool.available, "max": pool.max_size, "waiting": pool.waiting },
        },
        "storage": { "kind": if local_files { "local" } else { "s3" }, "description": storage.describe() },
        "backups": {
            "count": backups.len(),
            "last": last.map(|(key, size, modified)| json!({ "key": key, "size": size, "modified": modified })),
            "encrypted": encrypted,
            "schedule": settings.backups.cron,
            "inS3": backups_in_s3,
        },
        "logs": { "requests24h": requests, "errors24h": errors, "warnings24h": warnings },
        "checks": checks,
    })))
}
