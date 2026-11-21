//! A job handler that panics, through the whole queue path: the run is recorded as failed, and
//! the worker keeps running other jobs. See `setup_flow.rs` for how to point the tests at a
//! database.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::Value;
use tokio::sync::Notify;

use common::*;
use slonbase::app::App;

async fn admin(
    app: &Arc<App>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    request(app, method, uri, &[("authorization", auth.as_str())], body).await
}

async fn runs(app: &Arc<App>, token: &str, query: &str) -> Value {
    let (status, res) = admin(app, token, "GET", &format!("/api/jobs/runs{query}"), None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    res
}

async fn eventually(mut check: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..250 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    false
}

/// The state and attempt of the queued job of a cron job, once the queue has one.
async fn queued(tdb: &TestDb, id: &str) -> Option<(String, i32)> {
    let client = tdb.pool().get().await.unwrap();
    client
        .query_opt(
            "SELECT state, attempt FROM _slonbase.jobs WHERE ref = $1 ORDER BY id DESC LIMIT 1",
            &[&id],
        )
        .await
        .unwrap()
        .map(|row| (row.get(0), row.get(1)))
}

#[tokio::test]
async fn a_job_that_panics_fails_its_run_and_leaves_the_worker_running() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    // the queue runs jobs on two workers: one is held busy for the whole test, so that the job
    // after the panic can only run on the worker that had the panic
    let holding = Arc::new(AtomicBool::new(false));
    let release = Arc::new(Notify::new());
    let mut extensions = slonbase::Extensions::default();
    {
        let (holding, release) = (holding.clone(), release.clone());
        extensions
            .hooks
            .cron("holds", "15 4 1 1 *", move |_| {
                let (holding, release) = (holding.clone(), release.clone());
                async move {
                    holding.store(true, Ordering::SeqCst);
                    release.notified().await;
                    Ok(())
                }
            })
            .unwrap();
    }
    extensions
        .hooks
        .cron("explodes", "15 4 1 1 *", |_| async {
            panic!("a handler that panics");
            #[allow(unreachable_code)]
            Ok(())
        })
        .unwrap();
    extensions
        .hooks
        .cron("fine", "15 4 1 1 *", |_| async { Ok(()) })
        .unwrap();
    let (app, token, _dir) = ready_app_configured(&tdb, extensions, |_| {}).await;

    let (status, res) = admin(&app, &token, "POST", "/api/crons/holds", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    assert!(
        eventually(async || holding.load(Ordering::SeqCst)).await,
        "one worker is held"
    );

    let (status, res) = admin(&app, &token, "POST", "/api/crons/explodes", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    assert!(
        eventually(async || runs(&app, &token, "?status=error").await["errors"] == 1).await,
        "the panic is recorded as a failed run"
    );
    let failed = runs(&app, &token, "?status=error").await;
    let run = &failed["items"][0];
    assert_eq!(run["kind"], "cron");
    assert_eq!(run["job"], "explodes");
    assert_eq!(run["trigger"], "manual");
    assert!(
        run["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("stopped unexpectedly"),
        "{run}"
    );

    // and the queued job itself ended as failed, after one attempt
    let mut state = None;
    for _ in 0..250 {
        state = queued(&tdb, "explodes").await;
        if state.as_ref().is_some_and(|(s, _)| s == "failed") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert_eq!(state, Some(("failed".to_string(), 1)));

    // the worker is still there: the next job runs on it (the other is held)
    let (status, res) = admin(&app, &token, "POST", "/api/crons/fine", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    assert!(
        eventually(async || runs(&app, &token, "?kind=cron").await["ok"] == 1).await,
        "a job after the panic runs"
    );
    assert_eq!(
        runs(&app, &token, "?status=error").await["errors"],
        1,
        "the panicking job is not claimed again"
    );

    release.notify_one();
    tdb.drop_db().await;
}
