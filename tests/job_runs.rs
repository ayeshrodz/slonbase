//! The history of what jobs did (`_slonbase.job_runs`) and how the queue keeps it: runs of a
//! job never overlap, a run an instance left behind doesn't hold up the next, a job waiting to
//! try again leaves its worker to others, and every installation prunes on its own.
//! See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

const NEVER: &str = "0 0 29 2 *";

async fn call(
    app: &Arc<App>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    request(app, method, uri, &[("authorization", auth.as_str())], body).await
}

async fn db(tdb: &TestDb) -> tokio_postgres::Client {
    let (client, conn) = tokio_postgres::connect(&tdb.url, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(conn);
    client
}

async fn eventually(mut check: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..200 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// A job that runs `statement` (the SQL console is on in these apps); it never runs on its
/// schedule.
async fn sql_job(app: &Arc<App>, token: &str, name: &str, statement: &str, retries: i64) -> String {
    let (status, job) = call(
        app,
        token,
        "POST",
        "/api/crons",
        Some(json!({
            "name": name, "expression": NEVER, "type": "sql",
            "config": { "statement": statement },
            "retries": retries, "retryDelaySeconds": 300,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{job}");
    job["id"].as_str().unwrap().to_string()
}

async fn run_now(app: &Arc<App>, token: &str, id: &str) {
    let (status, _) = call(app, token, "POST", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// The runs in the job's history, newest first.
async fn history(app: &Arc<App>, token: &str, id: &str) -> Vec<Value> {
    let (_, res) = call(
        app,
        token,
        "GET",
        &format!("/api/jobs/runs?kind=cron&job={id}"),
        None,
    )
    .await;
    res["items"].as_array().cloned().unwrap_or_default()
}

/// Waits until the queue has nothing waiting or running for the job.
async fn queue_idle(client: &tokio_postgres::Client, id: &str) -> bool {
    eventually(async || {
        client
            .query_one(
                "SELECT count(*) FROM _slonbase.jobs
                 WHERE ref = $1 AND state IN ('queued', 'running')
                   AND payload->>'trigger' = 'manual'
                   AND NOT (attempt >= max_attempts AND locked_until < now())",
                &[&id],
            )
            .await
            .unwrap()
            .get::<_, i64>(0)
            == 0
    })
    .await
}

#[tokio::test]
async fn runs_that_pass_the_check_together_are_one_run() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, _token, _dir) = ready_app(&tdb).await;
    let core = app.core().unwrap();
    let client = db(&tdb).await;
    // eight workers, each with a queued job of its own that it is running
    let mut queued = Vec::new();
    for _ in 0..8 {
        let row = client
            .query_one(
                "INSERT INTO _slonbase.jobs (kind, ref, state, attempt, max_attempts, locked_by, locked_until)
                 VALUES ('cron.user', 'user:race', 'running', 1, 1, 'test', now() + interval '1 hour')
                 RETURNING id",
                &[],
            )
            .await
            .unwrap();
        queued.push(row.get::<_, i64>(0));
    }
    let barrier = Arc::new(tokio::sync::Barrier::new(queued.len()));
    let mut tasks = Vec::new();
    for queue_job in queued {
        let (core, barrier) = (core.clone(), barrier.clone());
        tasks.push(tokio::spawn(async move {
            // (all of them are past the starting line before any of them goes on)
            barrier.wait().await;
            slonbase::jobs::start_exclusive(&core, "cron", "user:race", "manual", queue_job)
                .await
                .unwrap()
                .is_some()
        }));
    }
    let mut started = 0;
    for task in tasks {
        started += task.await.unwrap() as i32;
    }
    assert_eq!(started, 1, "more than one run of the job started at once");
    let rows: i64 = client
        .query_one(
            "SELECT count(*) FROM _slonbase.job_runs WHERE job = 'user:race' AND status = 'running'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn a_job_started_twice_at_once_runs_once() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let client = db(&tdb).await;
    let id = sql_job(&app, &token, "Slow", "SELECT pg_sleep(3)", 0).await;
    tokio::join!(run_now(&app, &token, &id), run_now(&app, &token, &id));
    assert!(queue_idle(&client, &id).await);
    let runs = history(&app, &token, &id).await;
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0]["status"], "ok", "{runs:?}");
}

#[tokio::test]
async fn a_run_left_by_an_earlier_version_doesnt_hold_up_the_job() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let client = db(&tdb).await;
    let id = sql_job(&app, &token, "Quick", "SELECT 1", 0).await;
    // a run that 2.6 started and an upgrade stopped: it has no queued job to say it ended
    client
        .execute(
            "INSERT INTO _slonbase.job_runs (kind, job, trigger, started, status)
             VALUES ('cron', $1, 'schedule', now() - interval '10 minutes', 'running')",
            &[&id],
        )
        .await
        .unwrap();
    run_now(&app, &token, &id).await;
    assert!(
        eventually(async || {
            history(&app, &token, &id)
                .await
                .iter()
                .any(|r| r["trigger"] == "manual" && r["status"] == "ok")
        })
        .await,
        "{:?}",
        history(&app, &token, &id).await
    );
    // and the history says what happened to the old one
    let runs = history(&app, &token, &id).await;
    let old = runs.iter().find(|r| r["trigger"] == "schedule").unwrap();
    assert_eq!(old["status"], "error", "{runs:?}");
    assert!(
        old["detail"].as_str().unwrap().contains("Interrupted"),
        "{old}"
    );
}

#[tokio::test]
async fn a_run_whose_instance_stopped_doesnt_hold_up_the_job() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let client = db(&tdb).await;
    let id = sql_job(&app, &token, "Quick", "SELECT 1", 0).await;
    // a run whose queued job is on its last attempt and whose lease ran out
    let queued: i64 = client
        .query_one(
            "INSERT INTO _slonbase.jobs (kind, ref, state, attempt, max_attempts, locked_by, locked_until, started)
             VALUES ('cron.user', $1, 'running', 1, 1, 'gone', now() - interval '5 minutes', now() - interval '6 minutes')
             RETURNING id",
            &[&id],
        )
        .await
        .unwrap()
        .get(0);
    client
        .execute(
            "INSERT INTO _slonbase.job_runs (kind, job, trigger, started, status, queue_job)
             VALUES ('cron', $1, 'schedule', now() - interval '6 minutes', 'running', $2)",
            &[&id, &queued],
        )
        .await
        .unwrap();
    run_now(&app, &token, &id).await;
    assert!(
        eventually(async || {
            history(&app, &token, &id)
                .await
                .iter()
                .any(|r| r["trigger"] == "manual" && r["status"] == "ok")
        })
        .await,
        "{:?}",
        history(&app, &token, &id).await
    );
}

#[tokio::test]
async fn a_run_that_is_going_still_holds_up_the_next() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let client = db(&tdb).await;
    let id = sql_job(&app, &token, "Quick", "SELECT 1", 0).await;
    // a run whose queued job holds its lease
    let queued: i64 = client
        .query_one(
            "INSERT INTO _slonbase.jobs (kind, ref, state, attempt, max_attempts, locked_by, locked_until, started)
             VALUES ('cron.user', $1, 'running', 1, 1, 'elsewhere', now() + interval '1 hour', now())
             RETURNING id",
            &[&id],
        )
        .await
        .unwrap()
        .get(0);
    client
        .execute(
            "INSERT INTO _slonbase.job_runs (kind, job, trigger, status, queue_job)
             VALUES ('cron', $1, 'schedule', 'running', $2)",
            &[&id, &queued],
        )
        .await
        .unwrap();
    run_now(&app, &token, &id).await;
    // the run that was asked for is dropped, not run next to the one that is going
    assert!(
        eventually(async || {
            client
                .query_one(
                    "SELECT count(*) FROM _slonbase.jobs
                     WHERE ref = $1 AND payload->>'trigger' = 'manual' AND state = 'done'",
                    &[&id],
                )
                .await
                .unwrap()
                .get::<_, i64>(0)
                == 1
        })
        .await
    );
    let runs = history(&app, &token, &id).await;
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0]["status"], "running", "{runs:?}");
}

#[tokio::test]
async fn a_job_waiting_to_try_again_leaves_its_worker_to_others() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    // more jobs failing and waiting five minutes than the installation has workers
    let mut failing = Vec::new();
    for n in 0..4 {
        failing.push(sql_job(&app, &token, &format!("Fails {n}"), "SELECT 1/0", 1).await);
    }
    for id in &failing {
        run_now(&app, &token, id).await;
    }
    for id in &failing {
        assert!(
            eventually(async || {
                history(&app, &token, id).await.first().is_some_and(|r| {
                    r["detail"]
                        .as_str()
                        .is_some_and(|d| d.contains("will be tried again"))
                })
            })
            .await,
            "{:?}",
            history(&app, &token, id).await
        );
    }
    // an unrelated job runs now, not after them
    let quick = sql_job(&app, &token, "Quick", "SELECT 1", 0).await;
    run_now(&app, &token, &quick).await;
    assert!(
        eventually(async || {
            history(&app, &token, &quick)
                .await
                .first()
                .is_some_and(|r| r["status"] == "ok")
        })
        .await,
        "{:?}",
        history(&app, &token, &quick).await
    );
    // and the ones waiting are still waiting, not run again or finished
    for id in &failing {
        let runs = history(&app, &token, id).await;
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_eq!(runs[0]["status"], "running", "{runs:?}");
    }
}

#[tokio::test]
async fn every_installation_prunes_on_its_own() {
    let (Some(first), Some(second)) = (TestDb::create().await, TestDb::create().await) else {
        return;
    };
    let (app_a, _, _dir_a) = ready_app(&first).await;
    let (app_b, _, _dir_b) = ready_app(&second).await;
    let (core_a, core_b) = (app_a.core().unwrap(), app_b.core().unwrap());
    let (client_a, client_b) = (db(&first).await, db(&second).await);
    for client in [&client_a, &client_b] {
        client
            .execute(
                "INSERT INTO _slonbase.jobs (kind, state, finished)
                 VALUES ('old.test', 'done', now() - interval '8 days')",
                &[],
            )
            .await
            .unwrap();
    }
    // the maintenance tick of each, one a minute, for an hour: they take turns
    for _ in 0..60 {
        slonbase::jobs::prune_hourly(&core_a).await;
        slonbase::jobs::prune_hourly(&core_b).await;
    }
    for (name, client) in [("first", &client_a), ("second", &client_b)] {
        let left: i64 = client
            .query_one(
                "SELECT count(*) FROM _slonbase.jobs WHERE kind = 'old.test'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(left, 0, "the {name} installation was not pruned in an hour");
    }
}
