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
