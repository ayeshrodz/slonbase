//! The job queue (`_slonbase.jobs`): a scheduled job is queued once, background work doesn't
//! use the API's connections, and a job left by an instance that stopped is picked up.
//! See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

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

/// An app with an `events` collection (three records) and a job that deletes the old ones.
async fn with_cleanup_job(tdb: &TestDb) -> (Arc<App>, String, tempfile::TempDir, String) {
    let (app, token, dir) = ready_app(tdb).await;
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "events", "fields": [{ "name": "title", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    for title in ["old a", "keep", "old b"] {
        let (status, _) = call(
            &app,
            &token,
            "POST",
            "/api/collections/events/records",
            Some(json!({ "title": title })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, job) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(json!({
            "name": "Old events", "expression": "* * * * *", "type": "cleanup",
            "config": { "collection": "events", "filter": "title ~ 'old'" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{job}");
    let id = job["id"].as_str().unwrap().to_string();
    (app, token, dir, id)
}

async fn states(client: &tokio_postgres::Client) -> Vec<(String, String, i32)> {
    client
        .query(
            "SELECT state, error, attempt FROM _slonbase.jobs ORDER BY id",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect()
}

#[tokio::test]
async fn a_scheduled_job_is_queued_once_a_minute_and_run() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, _token, _dir, id) = with_cleanup_job(&tdb).await;
    let core = app.core().unwrap();
    let client = db(&tdb).await;

    // several instances notice the same minute: one row
    let minute = || chrono::Utc::now().format("%H:%M").to_string();
    let before = minute();
    for _ in 0..3 {
        slonbase::hooks::run_scheduled_crons(&core).await;
    }
    let same_minute = before == minute();
    let queued: i64 = client
        .query_one(
            "SELECT count(*) FROM _slonbase.jobs WHERE kind = 'cron.user' AND ref = $1",
            &[&id],
        )
        .await
        .unwrap()
        .get(0);
    if same_minute {
        assert_eq!(queued, 1);
    }
    // a worker runs it, and the queue says so
    assert!(
        eventually(async || states(&client).await.iter().all(|s| s.0 == "done")).await,
        "{:?}",
        states(&client).await
    );
    let runs: i64 = client
        .query_one(
            "SELECT count(*) FROM _slonbase.job_runs WHERE job = $1 AND status = 'ok'",
            &[&id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(runs, queued, "each queued run is in the history");
}

#[tokio::test]
async fn background_jobs_dont_use_the_apis_connections() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir, id) = with_cleanup_job(&tdb).await;
    let core = app.core().unwrap();
    let client = db(&tdb).await;

    // a job due in a moment (and a run now, so the workers look and see when it is due)
    client
        .execute(
            "INSERT INTO _slonbase.jobs (kind, ref, payload, run_after)
             VALUES ('cron.user', $1, '{\"trigger\":\"manual\"}', now() + interval '2 seconds')",
            &[&id],
        )
        .await
        .unwrap();
    let (status, _) = call(&app, &token, "POST", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        eventually(async || states(&client)
            .await
            .iter()
            .filter(|s| s.0 == "done")
            .count()
            == 1)
        .await
    );
    // new records for the second run, then every API connection is taken
    for title in ["old c", "old d"] {
        call(
            &app,
            &token,
            "POST",
            "/api/collections/events/records",
            Some(json!({ "title": title })),
        )
        .await;
    }
    let mut held = Vec::new();
    for _ in 0..core.pool.status().max_size {
        held.push(core.pool.get().await.unwrap());
    }
    assert_eq!(core.pool.status().available, 0);

    // the job still runs to the end: it reads, deletes and records on the workers' connections
    assert!(
        eventually(async || states(&client).await.iter().all(|s| s.0 == "done")).await,
        "{:?}",
        states(&client).await
    );
    let left: i64 = client
        .query_one(
            "SELECT count(*) FROM slonbase.events WHERE title LIKE 'old%'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(left, 0, "the second run deleted its records");
    let ok: i64 = client
        .query_one(
            "SELECT count(*) FROM _slonbase.job_runs WHERE job = $1 AND status = 'ok'",
            &[&id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(ok, 2);
    drop(held);
}

#[tokio::test]
async fn a_job_left_by_an_instance_that_stopped_is_picked_up() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir, id) = with_cleanup_job(&tdb).await;
    let client = db(&tdb).await;

    // two jobs another instance was running when it stopped (their leases ran out): one has an
    // attempt left, the other doesn't
    for max_attempts in [2, 1] {
        client
            .execute(
                "INSERT INTO _slonbase.jobs
                     (kind, ref, payload, state, attempt, max_attempts, locked_by, locked_until, started)
                 VALUES ('cron.user', $1, '{\"trigger\":\"manual\"}', 'running', 1, $2, 'gone',
                         now() - interval '1 minute', now() - interval '5 minutes')",
                &[&id, &max_attempts],
            )
            .await
            .unwrap();
    }
    // (anything queued makes the workers look)
    let (status, _) = call(&app, &token, "POST", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert!(
        eventually(async || states(&client)
            .await
            .iter()
            .all(|s| s.0 != "running" && s.0 != "queued"))
        .await,
        "{:?}",
        states(&client).await
    );
    let all = states(&client).await;
    assert_eq!(
        (all[0].0.as_str(), all[0].2),
        ("done", 2),
        "run again: {all:?}"
    );
    assert_eq!(all[1].0, "failed", "{all:?}");
    assert!(all[1].1.contains("Interrupted"), "{all:?}");
    assert_eq!(all[2].0, "done", "{all:?}");
}
