//! `GET /api/system/status`: the Health page's data. See `setup_flow.rs` for how to point the
//! tests at a database.

mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::*;

#[tokio::test]
async fn reports_the_installation() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, _) = request(&app, "GET", "/api/system/status", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let auth = format!("Bearer {token}");
    let headers = [("authorization", auth.as_str())];
    let (status, body) = request(&app, "GET", "/api/system/status", &headers, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert!(body["database"]["latencyMs"].as_f64().unwrap() >= 0.0);
    assert!(body["database"]["sizeBytes"].as_i64().unwrap() > 0);
    assert!(body["database"]["migration"].as_i64().unwrap() >= 13);
    assert!(body["database"]["pool"]["max"].as_u64().unwrap() > 0);
    assert_eq!(body["storage"]["kind"], "local");
    assert_eq!(body["backups"]["count"], 0);
    assert_eq!(body["backups"]["encrypted"], false);
    let ids: Vec<&str> = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    for id in [
        "database",
        "backups",
        "smtp",
        "app-url",
        "backup-encryption",
    ] {
        assert!(ids.contains(&id), "{id}: {ids:?}");
    }
    let check = |id: &str| {
        body["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(check("database")["status"], "ok");
    assert_eq!(check("backups")["status"], "warn");

    // a backup and an application URL clear their checks
    request(
        &app,
        "PATCH",
        "/api/settings",
        &headers,
        Some(json!({ "meta": { "appURL": "https://app.example.com" } })),
    )
    .await;
    let (status, _) = request(&app, "POST", "/api/backups", &headers, Some(json!({}))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = request(&app, "GET", "/api/system/status", &headers, None).await;
    let checks = body["checks"].as_array().unwrap();
    assert!(!checks.iter().any(|c| c["id"] == "app-url"), "{body}");
    assert!(
        checks
            .iter()
            .any(|c| c["id"] == "backups" && c["status"] == "ok"),
        "{body}"
    );
    assert_eq!(body["backups"]["count"], 1);
    drop(app);
    tdb.drop_db().await;
}

// The errors check counts what needs looking at: server errors (5xx) and errors outside requests.
// Requests refused with 401 or 403 are logged at the error level too (PocketBase's format), but an
// expired session or a disabled SQL console isn't something to fix.
#[tokio::test]
async fn the_errors_check_ignores_refused_requests() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = format!("Bearer {token}");
    let headers = [("authorization", auth.as_str())];
    let client = tdb.pool().get().await.unwrap();
    let log = |id: &'static str, level: i32, data: serde_json::Value| {
        let client = &client;
        async move {
            client
                .execute(
                    "INSERT INTO _slonbase.logs (id, created, level, message, data) VALUES ($1, now(), $2, '', $3)",
                    &[&id, &level, &data],
                )
                .await
                .unwrap();
        }
    };
    let errors_check = |body: &serde_json::Value| {
        body["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "errors")
            .cloned()
    };

    log(
        "log00000000401",
        8,
        json!({ "type": "request", "status": 401 }),
    )
    .await;
    log(
        "log00000000403",
        8,
        json!({ "type": "request", "status": 403 }),
    )
    .await;
    let (_, body) = request(&app, "GET", "/api/system/status", &headers, None).await;
    assert!(errors_check(&body).is_none(), "{body}");

    log("log0000000hook", 8, json!({ "type": "hook" })).await;
    log(
        "log00000000500",
        8,
        json!({ "type": "request", "status": 500 }),
    )
    .await;
    let (_, body) = request(&app, "GET", "/api/system/status", &headers, None).await;
    let check = errors_check(&body).expect("an errors check");
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .starts_with("2 logged errors"),
        "{check}"
    );
    drop(client);
    drop(app);
    tdb.drop_db().await;
}
