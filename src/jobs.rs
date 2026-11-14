//! What jobs did (`_slonbase.job_runs`): each automatic or manual backup and each cron job run,
//! with when it started and finished, how it ended and what it made. The dashboard shows them
//! as the backups' and the cron jobs' history (`GET /api/jobs/runs`). Recording never stops a
//! job: if the database can't take the row, the job still runs and the failure is logged.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app::App;
use crate::auth::RequestAuth;
use crate::core::Core;
use crate::error::ApiError;

/// Runs kept per job.
const KEEP: i64 = 500;

pub const BACKUP: &str = "backup";
pub const CRON: &str = "cron";

/// A run being recorded.
pub struct Run {
    core: Arc<Core>,
    id: Option<i64>,
}

/// Records that `job` (of `kind`) started, by `trigger` (`schedule`, `manual` or `api`).
pub async fn start(core: &Arc<Core>, kind: &str, job: &str, trigger: &str) -> Run {
    start_queued(core, kind, job, trigger, None).await
}

/// The run that the queued job `queue_job` started on an earlier attempt, if it did.
pub(crate) async fn resume(core: &Arc<Core>, queue_job: i64) -> Option<Run> {
    let client = core.db().get().await.ok()?;
    let row = client
        .query_opt(
            "SELECT id FROM _slonbase.job_runs WHERE queue_job = $1 AND status = 'running'
             ORDER BY id DESC LIMIT 1",
            &[&queue_job],
        )
        .await
        .ok()??;
    Some(Run {
        core: core.clone(),
        id: Some(row.get(0)),
    })
}

/// [`start`] for a run that belongs to a queued job, so a later attempt can [`resume`] it.
pub(crate) async fn start_queued(
    core: &Arc<Core>,
    kind: &str,
    job: &str,
    trigger: &str,
    queue_job: Option<i64>,
) -> Run {
    let id = async {
        let client = core
            .db()
            .get()
            .await
            .map_err(crate::db::describe_pool_error)?;
        let row = client
            .query_one(
                "INSERT INTO _slonbase.job_runs (kind, job, trigger, queue_job)
                 VALUES ($1, $2, $3, $4) RETURNING id",
                &[&kind, &job, &trigger, &queue_job],
            )
            .await?;
        anyhow::Ok(row.get::<_, i64>(0))
    }
    .await;
    let id = match id {
        Ok(id) => Some(id),
        Err(err) => {
            tracing::warn!(job = %job, error = %format!("{err:#}"), "failed to record a job's start");
            None
        }
    };
    Run {
        core: core.clone(),
        id,
    }
}

impl Run {
    /// The run's id in the history (none when it couldn't be recorded).
    pub(crate) fn id(&self) -> Option<i64> {
        self.id
    }

    /// Notes where a run that goes on stands (a failed attempt that will be tried again).
    pub(crate) async fn note(&self, detail: &str) {
        let Some(id) = self.id else { return };
        if let Ok(client) = self.core.db().get().await {
            let _ = client
                .execute(
                    "UPDATE _slonbase.job_runs SET detail = $2 WHERE id = $1",
                    &[&id, &detail.chars().take(4000).collect::<String>()],
                )
                .await;
        }
    }

    /// Records how the run ended: `Ok` with what it made, or `Err` with why it failed.
    pub async fn finish(self, result: Result<Value, String>) {
        let Some(id) = self.id else {
            return;
        };
        let (status, detail, meta) = match result {
            Ok(meta) => ("ok", String::new(), meta),
            Err(detail) => ("error", detail, json!({})),
        };
        let done = async {
            let client = self
                .core
                .db()
                .get()
                .await
                .map_err(crate::db::describe_pool_error)?;
            client
                .execute(
                    "UPDATE _slonbase.job_runs SET finished = now(), status = $2, detail = $3, meta = $4
                     WHERE id = $1",
                    &[&id, &status, &detail.chars().take(4000).collect::<String>(), &meta],
                )
                .await?;
            anyhow::Ok(())
        };
        if let Err(err) = done.await {
            tracing::warn!(error = %format!("{err:#}"), "failed to record a job's end");
        }
    }
}

/// A run's recorded result as the API shows it. Runs recorded before the history stopped keeping
/// the content of a called URL's answer have it under `response`; it is not shown.
fn run_meta(mut meta: Value) -> Value {
    if let Some(object) = meta.as_object_mut() {
        object.remove("response");
    }
    meta
}

/// [`prune`] on one call in 60 (the maintenance tick runs every minute).
pub(crate) async fn prune_hourly(core: &Core) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static TICKS: AtomicU32 = AtomicU32::new(0);
    if !TICKS.fetch_add(1, Ordering::Relaxed).is_multiple_of(60) {
        return;
    }
    if let Err(err) = crate::queue::prune(core).await {
        tracing::debug!(error = %format!("{err:#}"), "failed to prune the job queue");
    }
    for kind in [BACKUP, CRON] {
        if let Err(err) = close_orphans(core, kind, None).await {
            tracing::debug!(error = %format!("{err:#}"), "failed to close interrupted runs");
        }
    }
    if let Err(err) = prune(core).await {
        tracing::debug!(error = %format!("{err:#}"), "failed to prune the jobs' history");
    }
}

/// Closes the runs whose queued job ended without them (its instance stopped on the last
/// attempt): they would otherwise count as running.
pub(crate) async fn close_orphans(core: &Core, kind: &str, job: Option<&str>) -> anyhow::Result<()> {
    let client = core
        .db()
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    client
        .execute(
            "UPDATE _slonbase.job_runs r SET status = 'error', finished = now(),
                 detail = 'Interrupted: the instance running it stopped.'
             WHERE r.kind = $1 AND ($2::text IS NULL OR r.job = $2) AND r.status = 'running'
               AND r.queue_job IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM _slonbase.jobs j
                               WHERE j.id = r.queue_job AND j.state IN ('queued', 'running'))",
            &[&kind, &job],
        )
        .await?;
    Ok(())
}

/// Keeps the last runs of each job, and closes runs left `running` by an instance that
/// stopped (after a day).
pub(crate) async fn prune(core: &Core) -> anyhow::Result<()> {
    let client = core
        .db()
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    client
        .batch_execute(&format!(
            "DELETE FROM _slonbase.job_runs r USING (
                 SELECT id FROM (
                     SELECT id, row_number() OVER (PARTITION BY kind, job ORDER BY started DESC) AS n
                     FROM _slonbase.job_runs
                 ) ranked WHERE n > {KEEP}
             ) old WHERE r.id = old.id;
             UPDATE _slonbase.job_runs SET status = 'error', finished = now(),
                 detail = 'Interrupted: the instance running it stopped.'
             WHERE status = 'running' AND started < now() - interval '1 day';"
        ))
        .await?;
    Ok(())
}

fn run_json(row: &tokio_postgres::Row) -> Value {
    let started: chrono::DateTime<chrono::Utc> = row.get("started");
    let finished: Option<chrono::DateTime<chrono::Utc>> = row.get("finished");
    json!({
        "id": row.get::<_, i64>("id").to_string(),
        "kind": row.get::<_, String>("kind"),
        "job": row.get::<_, String>("job"),
        "trigger": row.get::<_, String>("trigger"),
        "started": crate::datetime::format(&started),
        "finished": finished.map(|f| crate::datetime::format(&f)),
        "durationMs": finished.map(|f| (f - started).num_milliseconds()),
        "status": row.get::<_, String>("status"),
        "detail": row.get::<_, String>("detail"),
        "meta": run_meta(row.get::<_, Value>("meta")),
    })
}

/// The last run of each job of `kind`.
pub(crate) async fn last_runs(
    client: &deadpool_postgres::Object,
    kind: &str,
) -> anyhow::Result<std::collections::HashMap<String, Value>> {
    let rows = client
        .query(
            "SELECT DISTINCT ON (job) * FROM _slonbase.job_runs WHERE kind = $1
             ORDER BY job, started DESC",
            &[&kind],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, String>("job"), run_json(r)))
        .collect())
}

#[derive(Deserialize)]
pub struct RunsQuery {
    kind: Option<String>,
    job: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
}

/// `GET /api/jobs/runs?kind=&job=&status=&limit=`: the latest runs, newest first (at most
/// 200), with each job's totals.
pub async fn runs(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    Query(q): Query<RunsQuery>,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let client = core
        .db()
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let empty = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
    let (kind, job, status) = (empty(&q.kind), empty(&q.job), empty(&q.status));
    let rows = client
        .query(
            "SELECT * FROM _slonbase.job_runs
             WHERE ($1::text IS NULL OR kind = $1) AND ($2::text IS NULL OR job = $2)
               AND ($3::text IS NULL OR status = $3)
             ORDER BY started DESC, id DESC LIMIT $4",
            &[&kind, &job, &status, &limit],
        )
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    let totals = client
        .query_one(
            "SELECT count(*), count(*) FILTER (WHERE status = 'ok'),
                    count(*) FILTER (WHERE status = 'error')
             FROM _slonbase.job_runs
             WHERE ($1::text IS NULL OR kind = $1) AND ($2::text IS NULL OR job = $2)",
            &[&kind, &job],
        )
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    Ok(Json(json!({
        "items": rows.iter().map(run_json).collect::<Vec<_>>(),
        "total": totals.get::<_, i64>(0),
        "ok": totals.get::<_, i64>(1),
        "errors": totals.get::<_, i64>(2),
    })))
}
