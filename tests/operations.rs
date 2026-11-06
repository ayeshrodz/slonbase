//! Operations (2.0): the jobs' history (backups and cron jobs), whether backups are encrypted,
//! the cron jobs' next runs, the file storage's whereabouts, and the security and log
//! settings doing what they say: CORS, the upload limit, HTTPS only, maintenance, paths left
//! out of the logs, slow requests and the log's size.
//! See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;
use slonbase::error::ApiError;

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

async fn settings(app: &Arc<App>, token: &str, patch: Value) {
    let (status, res) = admin(app, token, "PATCH", "/api/settings", Some(patch)).await;
    assert_eq!(status, StatusCode::OK, "{res}");
}

async fn eventually(mut check: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..150 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    false
}

async fn runs(app: &Arc<App>, token: &str, query: &str) -> Value {
    let (status, res) = admin(app, token, "GET", &format!("/api/jobs/runs{query}"), None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    res
}

#[tokio::test]
async fn jobs_history_backups_and_crons() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let mut extensions = slonbase::Extensions::default();
    extensions
        .hooks
        .cron("report", "0 2 * * *", |_| async { Ok(()) })
        .unwrap();
    extensions
        .hooks
        .cron("flaky", "15 4 1 1 *", |_| async {
            Err(ApiError::bad_request("upstream returned 502"))
        })
        .unwrap();
    let (app, token, _dir) = ready_app_configured(&tdb, extensions, |o| {
        o.backups_encryption_key = Some("correct horse battery".into())
    })
    .await;

    // only superusers see the history
    let (status, _) = request(&app, "GET", "/api/jobs/runs", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(runs(&app, &token, "").await["total"], 0);

    // a manual backup, recorded with what it made
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/backups",
        Some(json!({ "name": "manual.zip" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let history = runs(&app, &token, "?kind=backup").await;
    let run = &history["items"][0];
    assert_eq!(run["job"], "manual");
    assert_eq!(run["trigger"], "manual");
    assert_eq!(run["status"], "ok");
    assert_eq!(run["meta"]["key"], "manual.zip");
    assert_eq!(run["meta"]["encrypted"], true);
    assert!(run["meta"]["size"].as_u64().unwrap() > 0);
    assert!(run["durationMs"].as_i64().is_some());

    // backups say whether they're encrypted, and whether they're automatic
    let (_, list) = admin(&app, &token, "GET", "/api/backups", None).await;
    assert_eq!(list[0]["key"], "manual.zip");
    assert_eq!(list[0]["encrypted"], true);
    assert_eq!(list[0]["auto"], false);

    // the cron jobs: when they run next, in words, and how they last went
    settings(&app, &token, json!({ "backups": { "cron": "0 3 * * *" } })).await;
    for id in ["report", "flaky", "__pbAutoBackup__"] {
        let (status, res) = admin(&app, &token, "POST", &format!("/api/crons/{id}"), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{id}: {res}");
    }
    assert!(
        eventually(async || runs(&app, &token, "?kind=cron").await["total"] == 2).await,
        "the cron runs are recorded"
    );
    let (_, crons) = admin(&app, &token, "GET", "/api/crons", None).await;
    let job = |id: &str| {
        crons
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == id)
            .unwrap()
            .clone()
    };
    let backup = job("__pbAutoBackup__");
    assert_eq!(backup["description"], "every day at 03:00");
    assert!(
        backup["next"].as_str().unwrap().ends_with("03:00:00.000Z"),
        "{backup}"
    );
    assert_eq!(backup["lastRun"]["job"], "auto");
    assert_eq!(backup["lastRun"]["status"], "ok");
    assert!(
        backup["lastRun"]["meta"]["key"]
            .as_str()
            .unwrap()
            .starts_with("auto_backup_")
    );
    assert_eq!(job("report")["lastRun"]["status"], "ok");
    assert_eq!(job("report")["description"], "every day at 02:00");
    let flaky = job("flaky");
    assert_eq!(flaky["lastRun"]["status"], "error");
    assert!(
        flaky["lastRun"]["detail"]
            .as_str()
            .unwrap()
            .contains("upstream returned 502"),
        "{flaky}"
    );

    // filtered
    let failed = runs(&app, &token, "?status=error").await;
    assert_eq!(failed["items"].as_array().unwrap().len(), 1);
    assert_eq!(failed["items"][0]["job"], "flaky");
    let all = runs(&app, &token, "").await;
    assert_eq!(all["total"], 4);
    assert_eq!(all["errors"], 1);
    tdb.drop_db().await;
}

#[tokio::test]
async fn storage_info() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, dir) = ready_app(&tdb).await;
    let (status, _) = request(&app, "GET", "/api/settings/storage", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // a file and a thumbnail on disk
    let (_, info) = admin(&app, &token, "GET", "/api/settings/storage", None).await;
    assert_eq!(info["kind"], "local");
    let root = std::path::PathBuf::from(info["path"].as_str().unwrap());
    assert!(
        root.starts_with(std::fs::canonicalize(dir.path()).unwrap()),
        "{info}"
    );
    std::fs::create_dir_all(root.join("pbc_1/rec1/thumbs_a.png")).unwrap();
    std::fs::write(root.join("pbc_1/rec1/a.png"), vec![0u8; 1000]).unwrap();
    std::fs::write(
        root.join("pbc_1/rec1/thumbs_a.png/10x10_a.png"),
        vec![0u8; 200],
    )
    .unwrap();

    assert!(
        eventually(async || {
            let (_, info) =
                admin(&app, &token, "GET", "/api/settings/storage?refresh=1", None).await;
            info["usage"]["bytes"] == 1200
        })
        .await
    );
    let (_, info) = admin(&app, &token, "GET", "/api/settings/storage", None).await;
    let usage = &info["usage"];
    assert_eq!(usage["files"], 2);
    assert_eq!(usage["thumbBytes"], 200);
    assert_eq!(usage["collections"][0]["id"], "pbc_1");
    assert!(info["disk"]["free"].as_u64().unwrap() > 0, "{info}");

    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/settings/storage/clear-thumbs",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["bytes"], 200);
    assert!(!root.join("pbc_1/rec1/thumbs_a.png").exists());
    assert!(root.join("pbc_1/rec1/a.png").exists(), "the file stays");
    tdb.drop_db().await;
}

#[tokio::test]
async fn security_settings() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let get = async |uri: &str, headers: &[(&str, &str)]| {
        raw_request(&app, "GET", uri, headers, vec![]).await
    };

    // by default, like PocketBase, pages anywhere may call the API
    let (status, headers, _) = get("/api/health", &[("origin", "https://app.example.com")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["access-control-allow-origin"], "*");
    let (status, headers, _) = raw_request(
        &app,
        "OPTIONS",
        "/api/collections/posts/records",
        &[
            ("origin", "https://app.example.com"),
            ("access-control-request-method", "POST"),
            (
                "access-control-request-headers",
                "authorization, content-type",
            ),
        ],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(headers["access-control-allow-origin"], "*");
    assert!(
        headers["access-control-allow-methods"]
            .to_str()
            .unwrap()
            .contains("PATCH")
    );
    assert_eq!(
        headers["access-control-allow-headers"],
        "authorization, content-type"
    );
    // the dashboard isn't concerned
    let (_, headers, _) = get("/_/", &[("origin", "https://app.example.com")]).await;
    assert!(headers.get("access-control-allow-origin").is_none());

    // only some origins
    settings(
        &app,
        &token,
        json!({ "security": { "allowedOrigins": ["https://app.example.com"] } }),
    )
    .await;
    let (_, headers, _) = get("/api/health", &[("origin", "https://app.example.com")]).await;
    assert_eq!(
        headers["access-control-allow-origin"],
        "https://app.example.com"
    );
    assert!(
        headers["vary"].to_str().unwrap().contains("Origin"),
        "{headers:?}"
    );
    let (status, headers, _) = get("/api/health", &[("origin", "https://evil.example.com")]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "answered, but browsers won't let the page read it"
    );
    assert!(headers.get("access-control-allow-origin").is_none());
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(
            json!({ "security": { "allowedOrigins": ["app.example.com/path"], "maxUploadMb": 0 } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        res["data"]["security"]["allowedOrigins"]["0"].is_object(),
        "{res}"
    );
    assert!(res["data"]["security"]["maxUploadMb"].is_object(), "{res}");

    // the upload limit
    settings(&app, &token, json!({ "security": { "maxUploadMb": 1 } })).await;
    let (content_type, body) = multipart(&[], &[("file", "big.bin", &vec![7u8; 1_100_000])]);
    let (status, _, _) = raw_request(
        &app,
        "POST",
        "/api/collections/posts/records",
        &[("content-type", content_type.as_str())],
        body,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    // HTTPS only, when the application URL is https
    settings(&app, &token, json!({ "security": { "hsts": true } })).await;
    let (_, headers, _) = get("/api/health", &[]).await;
    assert!(headers.get("strict-transport-security").is_none(), "http");
    settings(
        &app,
        &token,
        json!({ "meta": { "appURL": "https://api.example.com" } }),
    )
    .await;
    let (_, headers, _) = get("/api/health", &[]).await;
    assert!(
        headers["strict-transport-security"]
            .to_str()
            .unwrap()
            .starts_with("max-age=31536000")
    );

    // maintenance: superusers only
    settings(
        &app,
        &token,
        json!({ "security": { "maintenance": true, "maintenanceMessage": "Back at 14:00 UTC." } }),
    )
    .await;
    let (status, res) = request(
        &app,
        "GET",
        "/api/collections/_superusers/records",
        &[],
        None,
    )
    .await;
    assert_ne!(status, StatusCode::SERVICE_UNAVAILABLE, "{res}");
    let (status, res) = request(&app, "GET", "/api/collections", &[], None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res["message"], "Back at 14:00 UTC.");
    let (status, _, _) = get("/api/health", &[]).await;
    assert_eq!(status, StatusCode::OK, "health checks get through");
    let (status, _) = admin(&app, &token, "GET", "/api/collections", None).await;
    assert_eq!(status, StatusCode::OK, "superusers get through");
    let (status, login) = request(
        &app,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        &[],
        Some(json!({ "identity": ADMIN_EMAIL, "password": ADMIN_PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "superusers can sign in: {login}");
    settings(
        &app,
        &token,
        json!({ "security": { "maintenance": false } }),
    )
    .await;
    let (status, _) = request(&app, "GET", "/api/collections", &[], None).await;
    assert_ne!(status, StatusCode::SERVICE_UNAVAILABLE);
    tdb.drop_db().await;
}

#[tokio::test]
async fn log_settings() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let count = async |filter: &str| -> i64 {
        let (_, res) = admin(
            &app,
            &token,
            "GET",
            &format!(
                "/api/logs?perPage=1&filter={}",
                form_urlencoded::byte_serialize(filter.as_bytes()).collect::<String>()
            ),
            None,
        )
        .await;
        res["totalItems"].as_i64().unwrap_or(-1)
    };

    // paths left out
    settings(
        &app,
        &token,
        json!({ "logs": { "excludePaths": ["/api/collections/nothing"] } }),
    )
    .await;
    for _ in 0..3 {
        request(&app, "GET", "/api/collections/nothing/records", &[], None).await;
        request(&app, "GET", "/api/collections/other/records", &[], None).await;
    }
    assert!(
        eventually(async || count("data.url ~ '/other/'").await == 3).await,
        "the others are logged"
    );
    assert_eq!(count("data.url ~ '/nothing/'").await, 0);

    // only errors, but slow requests all the same
    settings(
        &app,
        &token,
        json!({ "logs": { "minLevel": 8, "slowRequestMs": 1, "excludePaths": [] } }),
    )
    .await;
    for _ in 0..3 {
        request(
            &app,
            "GET",
            "/api/collections/posts/records?perPage=500",
            &[],
            None,
        )
        .await;
    }
    // a request at or over 1 ms is logged as a warning, marked slow
    assert!(eventually(async || count("data.slow = true && level = 4").await >= 1).await);

    // the log's size
    settings(
        &app,
        &token,
        json!({ "logs": { "minLevel": 0, "slowRequestMs": 0, "maxRows": 2 } }),
    )
    .await;
    for _ in 0..5 {
        request(&app, "GET", "/api/collections/x/records", &[], None).await;
    }
    assert!(eventually(async || count("").await >= 5).await);
    slonbase::logs::cleanup(&tdb.pool(), 5, 2).await.unwrap();
    assert_eq!(count("").await, 2);
    tdb.drop_db().await;
}
