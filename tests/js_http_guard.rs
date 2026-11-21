//! `hooks.guardHttp`: with it on, what a hook sends with `$http.send` or `$filesystem.fileFromURL`
//! can't reach this server or a private network, except the hosts in `hooks.allowHosts`; with it
//! off (the default) hooks reach everything as before. See `setup_flow.rs` for how to point the
//! tests at a database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;
use slonbase::config::HooksConfig;

const HOOKS: &str = r#"
routerAdd("GET", "/api/guard/send", (e) => {
  try {
    const res = $http.send({ url: e.request.url.query().get("url") });
    e.json(200, { status: res.statusCode, json: res.json });
  } catch (err) {
    e.json(200, { error: String(err) });
  }
});
routerAdd("GET", "/api/guard/count", (e) => {
  try {
    e.json(200, { count: $app.countRecords("things") });
  } catch (err) {
    e.json(200, { error: String(err) });
  }
});
routerAdd("GET", "/api/guard/download", (e) => {
  try {
    const file = $filesystem.fileFromURL(e.request.url.query().get("url"));
    e.json(200, { name: file.name });
  } catch (err) {
    e.json(200, { error: String(err) });
  }
});
"#;

async fn app_with_guard(
    tdb: &TestDb,
    guard_http: bool,
    allow_hosts: &[&str],
) -> (Arc<App>, tempfile::TempDir, tempfile::TempDir) {
    let hooks_dir = tempfile::tempdir().unwrap();
    std::fs::write(hooks_dir.path().join("main.pb.js"), HOOKS).unwrap();
    let mut ext = slonbase::Extensions::default();
    slonbase::js::load(
        &HooksConfig {
            dir: hooks_dir.path().to_path_buf(),
            pool_size: 2,
            watch: false,
            guard_http,
            allow_hosts: allow_hosts.iter().map(|h| h.to_string()).collect(),
            ..Default::default()
        },
        &mut ext,
    )
    .await
    .unwrap();
    let (app, _token, dir) = ready_app_with(tdb, ext).await;
    (app, dir, hooks_dir)
}

/// A server on this machine that answers `{"x": 1}`.
async fn local_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let router = axum::Router::new().route(
            "/data",
            axum::routing::get(|| async { axum::Json(json!({ "x": 1 })) }),
        );
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{address}/data")
}

async fn get(app: &Arc<App>, path: &str, url: &str) -> Value {
    let (status, res) = request(app, "GET", &format!("{path}?url={url}"), &[], None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    res
}

#[tokio::test]
async fn hooks_reach_private_addresses_unless_they_are_guarded() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let url = local_server().await;

    // off, the default: as before
    let (app, _dir, _hooks) = app_with_guard(&tdb, false, &[]).await;
    let res = get(&app, "/api/guard/send", &url).await;
    assert_eq!(res, json!({ "status": 200, "json": { "x": 1 } }));
    drop(app);
    tdb.drop_db().await;

    // on: the same call is refused, for a request and for a download
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, _dir, _hooks) = app_with_guard(&tdb, true, &[]).await;
    for path in ["/api/guard/send", "/api/guard/download"] {
        let res = get(&app, path, &url).await;
        let error = res["error"].as_str().unwrap_or_default();
        assert!(error.contains("not a public address"), "{path}: {res}");
    }
    tdb.drop_db().await;

    // on, with the host allowed: reached
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, _dir, _hooks) = app_with_guard(&tdb, true, &["127.0.0.1"]).await;
    let res = get(&app, "/api/guard/send", &url).await;
    assert_eq!(res, json!({ "status": 200, "json": { "x": 1 } }));
    tdb.drop_db().await;
}

/// With `hooks.dbWaitSeconds`, a database call from a hook that waits for a lock fails in the
/// hook instead of holding its runtime until the lock is released.
#[tokio::test]
async fn a_database_call_from_a_hook_waits_only_as_long_as_allowed() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let hooks_dir = tempfile::tempdir().unwrap();
    std::fs::write(hooks_dir.path().join("main.pb.js"), HOOKS).unwrap();
    let mut ext = slonbase::Extensions::default();
    slonbase::js::load(
        &HooksConfig {
            dir: hooks_dir.path().to_path_buf(),
            pool_size: 2,
            watch: false,
            db_wait_seconds: 1,
            ..Default::default()
        },
        &mut ext,
    )
    .await
    .unwrap();
    let (app, token, _dir) = ready_app_with(&tdb, ext).await;
    let auth = format!("Bearer {token}");
    let (status, res) = request(
        &app,
        "POST",
        "/api/collections",
        &[("authorization", auth.as_str())],
        Some(json!({ "name": "things", "fields": [{ "name": "title", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // not locked: counted
    let res = get(&app, "/api/guard/count", "").await;
    assert_eq!(res, json!({ "count": 0 }));

    // locked elsewhere: the hook is told, after about a second
    let (blocker, connection) = tokio_postgres::connect(&tdb.url, tokio_postgres::NoTls)
        .await
        .unwrap();
    let connection = tokio::spawn(async move {
        let _ = connection.await;
    });
    blocker
        .batch_execute("BEGIN; LOCK TABLE slonbase.things IN ACCESS EXCLUSIVE MODE")
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let res = get(&app, "/api/guard/count", "").await;
    let error = res["error"].as_str().unwrap_or_default();
    assert!(error.contains("hooks.db_wait_seconds"), "{res}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    blocker.batch_execute("ROLLBACK").await.unwrap();
    connection.abort();
    tdb.drop_db().await;
}
