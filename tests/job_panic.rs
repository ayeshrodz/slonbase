//! A job handler that panics, through the whole queue path: the run is recorded as failed, and
//! the worker keeps running other jobs. See `setup_flow.rs` for how to point the tests at a
//! database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::Value;

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

#[tokio::test]
async fn a_job_that_panics_fails_its_run_and_leaves_the_worker_running() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let mut extensions = slonbase::Extensions::default();
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

    let (status, res) = admin(&app, &token, "POST", "/api/crons/explodes", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    assert!(
        eventually(async || runs(&app, &token, "?status=error").await["total"] == 1).await,
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

    // the worker is still there: the next job runs
    let (status, res) = admin(&app, &token, "POST", "/api/crons/fine", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    assert!(
        eventually(async || runs(&app, &token, "?kind=cron").await["ok"] == 1).await,
        "a job after the panic runs"
    );
    // and the panicking one is not claimed again
    tokio::time::sleep(Duration::from_millis(600)).await;
    let after = runs(&app, &token, "?status=error").await;
    assert_eq!(after["errors"], 1, "{after}");
    assert_eq!(after["items"].as_array().unwrap().len(), 1, "{after}");
    tdb.drop_db().await;
}
