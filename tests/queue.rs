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
    with_cleanup_job_in(tdb, "").await
}

/// [`with_cleanup_job`] with this configuration file.
async fn with_cleanup_job_in(
    tdb: &TestDb,
    toml: &str,
) -> (Arc<App>, String, tempfile::TempDir, String) {
    let (app, token, dir) = ready_app_with_file(tdb, toml).await;
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

/// How many connections to the test's database are open, other than this test's own.
async fn open_connections(client: &tokio_postgres::Client) -> i64 {
    client
        .query_one(
            // client backends only: an autovacuum worker on this database isn't a connection
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND pid <> pg_backend_pid()
               AND backend_type = 'client backend'",
            &[],
        )
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn the_reserve_is_the_jobs_while_they_work_and_the_apis_otherwise() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    // 4 connections: 3 for the API, 1 reserved for jobs (and no job of its own, so the
    // workers only work when this test says so)
    let (app, _token, _dir) = ready_app_with_file(&tdb, "[database]\nmax_connections = 4\n").await;
    let core = app.core().unwrap();
    let client = db(&tdb).await;
    let db = core.db();
    assert_eq!(db.reserved(), 1);
    assert_eq!(core.pool.status().max_size, 3);
    let now = Duration::from_millis(500);
    // (the workers sleep: nothing is due)
    tokio::time::sleep(Duration::from_millis(300)).await;

    // no job running: the API has all four, and no more
    let mut held = Vec::new();
    for n in 0..4 {
        let got = tokio::time::timeout(Duration::from_secs(5), db.get()).await;
        held.push(got.unwrap_or_else(|_| panic!("connection {n}")).unwrap());
    }
    assert!(tokio::time::timeout(now, db.get()).await.is_err());
    assert!(db.status().size <= 4);
    drop(held);

    // a worker working: the API has its three, the jobs the fourth
    let working = db.working();
    let mut held = Vec::new();
    for n in 0..3 {
        let got = tokio::time::timeout(Duration::from_secs(5), db.get()).await;
        held.push(got.unwrap_or_else(|_| panic!("connection {n}")).unwrap());
    }
    assert!(
        tokio::time::timeout(now, db.get()).await.is_err(),
        "the API took a reserved connection while a job was running"
    );
    let job = tokio::time::timeout(now, db.for_jobs()).await;
    let job = job.expect("a job waited for the API").unwrap();
    // the budget holds: never more than the 4 (beside the 2 listeners)
    assert!(db.status().size <= 4, "{:?}", db.status());
    assert!(open_connections(&client).await <= 4 + 2);

    // a second job while the first holds the reserve: one of the API's, after a short wait
    drop(held.pop());
    let started = std::time::Instant::now();
    let second = tokio::time::timeout(Duration::from_secs(5), db.for_jobs()).await;
    let second = second.expect("a second job waited for ever").unwrap();
    assert!(started.elapsed() >= slonbase::connections::JOB_WAIT);
    assert!(db.status().size <= 4);
    drop((job, second, held, working));

    // the workers done: the API has all four again
    let mut held = Vec::new();
    for n in 0..4 {
        let got = tokio::time::timeout(Duration::from_secs(5), db.get()).await;
        held.push(
            got.unwrap_or_else(|_| panic!("connection {n} after the jobs"))
                .unwrap(),
        );
    }
    assert!(open_connections(&client).await <= 4 + 2);
    drop(held);
}

#[tokio::test]
async fn a_waiting_request_or_job_takes_whichever_connection_frees_first() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, _token, _dir) = ready_app_with_file(&tdb, "[database]\nmax_connections = 4\n").await;
    let core = app.core().unwrap();
    let db = core.db();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let soon = Duration::from_secs(1);

    // no job running, the API's three and the reserve's one in use: a request waiting for a
    // connection gets the reserve's as soon as it is free, not only one of the API's
    let mut api = Vec::new();
    for _ in 0..3 {
        api.push(core.pool.get().await.unwrap());
    }
    let reserved = db.get().await.unwrap();
    let waiting = tokio::spawn({
        let core = core.clone();
        async move { core.db().get().await.map(drop) }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(reserved);
    let got = tokio::time::timeout(soon, waiting).await;
    got.expect("the request didn't take the freed reserved connection")
        .unwrap()
        .unwrap();

    // a request begun while a job runs, the API's three in use: it gets the reserved connection
    // as soon as the job is done
    let working = db.working();
    let waiting = tokio::spawn({
        let core = core.clone();
        async move { core.db().get().await.map(drop) }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !waiting.is_finished(),
        "a request took the reserve while a job ran"
    );
    drop(working);
    let got = tokio::time::timeout(soon, waiting).await;
    got.expect("the request didn't take the reserve once the job was done")
        .unwrap()
        .unwrap();

    // a job running and past its wait, the API's three still in use: it gets the reserved
    // connection as soon as the other job lets it go
    let working = db.working();
    let first = db.for_jobs().await.unwrap();
    let second = tokio::spawn({
        let core = core.clone();
        async move { core.db().for_jobs().await.map(drop) }
    });
    tokio::time::sleep(slonbase::connections::JOB_WAIT + Duration::from_millis(500)).await;
    drop(first);
    let got = tokio::time::timeout(soon, second).await;
    got.expect("the job didn't take the freed reserved connection")
        .unwrap()
        .unwrap();
    drop((working, api));
}

#[tokio::test]
async fn no_reserve_when_it_is_turned_off() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, _token, _dir) = ready_app_with_file(&tdb, "[database]\njob_connections = 0\n").await;
    let core = app.core().unwrap();
    assert_eq!(core.db().reserved(), 0);
    assert_eq!(core.pool.status().max_size, 16);
    // jobs take the API's connections, as before 2.7
    let _job = core.db().for_jobs().await.unwrap();
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
