//! The main installation's command center for its projects (`/api/projects/overview` and
//! `/api/projects/{name}/settings|restart`): every installation's state, traffic and
//! configuration in one answer, and the controls the main installation's superusers have over
//! each project. Only the main installation serves these routes; a project manages itself
//! from its own dashboard.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Map, Value, json};

use crate::app::{App, Mode};
use crate::auth::RequestAuth;
use crate::core::Core;
use crate::error::ApiError;
use crate::extract::JsonObject;

/// How long one installation's numbers may take before the overview answers without them.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(4);

fn require_superuser(auth: &RequestAuth) -> Result<(), ApiError> {
    if auth.is_superuser() {
        Ok(())
    } else {
        Err(ApiError::superusers_only())
    }
}

/// Tables up to this many rows (PostgreSQL's estimate) are counted exactly; larger ones are
/// taken at the estimate, so a large installation doesn't scan its tables every few seconds.
const EXACT_COUNT_LIMIT: f32 = 200_000.0;

/// The records of each collection's table: exact for small tables, PostgreSQL's estimate for
/// large ones. `(collection name, records, estimated)`, views (which have no table) left out,
/// and the system collections (the superusers, linked sign-in accounts) too unless asked for.
pub(crate) async fn table_counts(
    client: &deadpool_postgres::Object,
    with_system: bool,
) -> anyhow::Result<Vec<(String, i64, bool)>> {
    let tables = client
        .query(
            "SELECT c.relname, c.reltuples FROM pg_class c
             JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'slonbase' AND c.relkind IN ('r', 'p') AND NOT c.relispartition
               AND ($1 OR c.relname NOT IN (SELECT data->>'name' FROM _slonbase.collections
                                            WHERE data->>'system' = 'true'))
             ORDER BY 1",
            &[&with_system],
        )
        .await?;
    let mut out = Vec::new();
    let mut exact = Vec::new();
    for t in &tables {
        let (name, tuples): (String, f32) = (t.get(0), t.get(1));
        if tuples > EXACT_COUNT_LIMIT {
            out.push((name, tuples as i64, true));
        } else {
            exact.push(name);
        }
    }
    if !exact.is_empty() {
        // one statement, so the count of a hundred collections is one round trip
        let sql = exact
            .iter()
            .map(|name| {
                format!(
                    "SELECT '{}'::text AS t, count(*) AS n FROM slonbase.{}",
                    name.replace('\'', "''"),
                    crate::filter::compile::quote_ident(name)
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        for row in client.query(&sql, &[]).await? {
            out.push((row.get(0), row.get(1), false));
        }
    }
    Ok(out)
}

/// The records in the collections' tables, without the system collections (as the dashboard's
/// Overview counts them): exact for small tables, PostgreSQL's estimate for large ones (`true`
/// when any number is an estimate).
async fn count_records(client: &deadpool_postgres::Object) -> anyhow::Result<(i64, bool)> {
    let counts = table_counts(client, false).await?;
    Ok((
        counts.iter().map(|(_, n, _)| n).sum(),
        counts.iter().any(|(_, _, estimated)| *estimated),
    ))
}

/// A running installation's numbers: its database, pool, data, traffic and configuration.
async fn snapshot(core: &Arc<Core>) -> anyhow::Result<Value> {
    let client = core
        .db()
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    // the round trip of a query on a connection in hand, not the wait for a free one
    let started = Instant::now();
    client.simple_query("SELECT 1").await?;
    let latency_ms = (started.elapsed().as_secs_f64() * 10_000.0).round() / 10.0;
    let db = client
        .query_one(
            "SELECT current_setting('server_version'), pg_database_size(current_database()),
                    0::bigint,
                    (SELECT count(*) FROM _slonbase.collections WHERE (data->>'system') IS DISTINCT FROM 'true')",
            &[],
        )
        .await?;
    let (records, estimated) = count_records(&client).await?;
    // the last 24 hours of requests: counts, server errors (as the Health page counts them),
    // and latency, of the requests the server answered (not the dashboard's own calls)
    let traffic = client
        .query_one(
            &format!(
                "WITH l AS (
                    SELECT level, data,
                           CASE WHEN data->>'execTime' ~ '^[0-9]{{1,15}}(\\.[0-9]{{1,15}})?$' THEN (data->>'execTime')::float8 END AS ms
                    FROM _slonbase.logs WHERE created > now() - interval '24 hours'
                 )
                 SELECT count(*) FILTER (WHERE {counted}),
                        count(*) FILTER (WHERE level >= 8 AND (data->>'type' IS DISTINCT FROM 'request'
                            OR CASE WHEN data->>'status' ~ '^[0-9]{{1,9}}$' THEN (data->>'status')::int >= 500 END)),
                        COALESCE(avg(ms) FILTER (WHERE {counted}), 0)::float8,
                        COALESCE(percentile_cont(0.95) WITHIN GROUP (ORDER BY ms) FILTER (WHERE {counted}), 0)::float8
                 FROM l",
                counted = crate::logs::COUNTED_REQUESTS
            ),
            &[],
        )
        .await?;
    let per_minute = crate::logs::counted_requests_by_period(&client, 60, 60).await?;
    let per_hour = crate::logs::counted_requests_by_period(&client, 3600, 24).await?;
    drop(client);
    let pool = core.db().status();
    let settings = core.settings();
    let round = |v: f64| (v * 10.0).round() / 10.0;
    Ok(json!({
        "db": {
            "latencyMs": latency_ms,
            "version": db.get::<_, String>(0).split_whitespace().next().unwrap_or_default(),
            "sizeBytes": db.get::<_, i64>(1),
        },
        "pool": {
            "size": pool.size,
            "available": pool.available,
            "inUse": pool.size.saturating_sub(pool.available),
            "max": pool.max_size,
        },
        "records": records,
        "recordsEstimated": estimated,
        "collections": db.get::<_, i64>(3),
        "realtimeClients": core.realtime.client_count(),
        "traffic": {
            "requests": traffic.get::<_, i64>(0),
            "errors": traffic.get::<_, i64>(1),
            "avgMs": round(traffic.get(2)),
            "p95Ms": round(traffic.get(3)),
            "perMinute": per_minute,
            "perHour": per_hour,
        },
        "config": {
            "appName": settings.meta.app_name,
            "appURL": settings.meta.app_url,
            "smtp": settings.smtp.enabled,
            "smtpHost": if settings.smtp.enabled { settings.smtp.host.clone() } else { String::new() },
            "s3": settings.s3.enabled,
            "backupsCron": settings.backups.cron,
            "backupsKeep": settings.backups.cron_max_keep,
            "backupsS3": settings.backups.s3.enabled,
            "backupsEncrypted": core.backup_key.is_some(),
            "rateLimits": settings.rate_limits.enabled,
            "logsDays": settings.logs.max_days,
            "hideControls": settings.meta.hide_controls,
            "superuserIPs": settings.superuser_ips.len(),
            "settingsEncrypted": core.secrets_key().is_some(),
        },
    }))
}

/// An installation's entry: what it is, and its numbers when it's running.
async fn entry(mut base: Value, app: &Arc<App>) -> Value {
    let (status, detail) = match app.mode() {
        Mode::Ready { .. } => ("ready", String::new()),
        Mode::Starting { status } => ("starting", status),
        Mode::Setup(_) => ("setup", String::new()),
    };
    base["status"] = json!(status);
    base["statusDetail"] = json!(detail);
    if let Mode::Ready { core } = app.mode() {
        match tokio::time::timeout(SNAPSHOT_TIMEOUT, snapshot(&core)).await {
            Ok(Ok(numbers)) => {
                if let (Value::Object(base), Value::Object(numbers)) = (&mut base, numbers) {
                    base.extend(numbers);
                }
            }
            Ok(Err(err)) => {
                base["status"] = json!("degraded");
                base["statusDetail"] = json!(format!("{err:#}"));
            }
            Err(_) => {
                base["status"] = json!("degraded");
                base["statusDetail"] = json!("its numbers took longer than 4 seconds");
            }
        }
    }
    base
}

/// `GET /api/projects/overview`: the main installation and every project, side by side.
pub(crate) async fn overview(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    require_superuser(&auth)?;
    app.core()?;
    let main_url = app
        .config()
        .effective
        .database
        .url
        .clone()
        .unwrap_or_default();
    let main_server = crate::projects::server_of(&main_url);
    let main_db = main_url
        .parse::<tokio_postgres::Config>()
        .ok()
        .and_then(|c| c.get_dbname().map(str::to_string))
        .unwrap_or_default();
    let main = json!({
        "name": "main",
        "main": true,
        "path": "/",
        "hosts": [],
        "external": false,
        "server": main_server,
        "database": main_db,
        "created": "",
        "role": null,
    });
    let projects = app.projects.list();
    let mut pending = vec![entry(main, &app)];
    for project in &projects {
        let mut base = project.overview_json();
        if base["server"].is_null() {
            // a database slonbase created, on the main installation's server
            base["server"] = json!(main_server);
        }
        pending.push(entry(base, &project.app));
    }
    let installations = futures_util::future::join_all(pending).await;
    Ok(Json(json!({
        "generated": crate::datetime::format(&chrono::Utc::now()),
        "items": installations,
    })))
}

fn project_core(app: &App, name: &str) -> Result<Arc<Core>, ApiError> {
    let project = app.projects.get(name).ok_or_else(ApiError::not_found)?;
    project
        .app
        .core()
        .map_err(|e| ApiError::unavailable(format!("The project isn't running: {}", e.message)))
}

/// `GET /api/projects/{name}/settings`: a project's settings (passwords and secrets left out,
/// as its own `GET /api/settings` sends them).
pub(crate) async fn settings(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_superuser(&auth)?;
    let core = project_core(&app, &name)?;
    // a viewer sees what is on, not the infrastructure behind it (as in `GET /api/settings`)
    Ok(Json(if auth.role == Some(crate::roles::Role::Viewer) {
        core.settings().viewer_json()
    } else {
        core.settings().public_json()
    }))
}

/// `PATCH /api/projects/{name}/settings`: changes a project's settings, merged and validated
/// as its own `PATCH /api/settings` does, and announced to every instance serving it.
pub(crate) async fn update_settings(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    Path(name): Path<String>,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    require_superuser(&auth)?;
    let core = project_core(&app, &name)?;
    let saved = crate::settings::apply_patch(&core, Map::from_iter(body)).await?;
    tracing::info!(project = %name, sections = ?saved.public_json().as_object().map(|o| o.len()), "changed a project's settings from the main installation");
    Ok(Json(saved.public_json()))
}

/// `POST /api/projects/{name}/restart`: restarts a project on this instance (its hooks are
/// loaded again, its connections opened afresh).
pub(crate) async fn restart(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    Path(name): Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    require_superuser(&auth)?;
    if app.projects.get(&name).is_none() {
        return Err(ApiError::not_found());
    }
    app.projects
        .restart(&name)
        .await
        .map_err(ApiError::internal)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
