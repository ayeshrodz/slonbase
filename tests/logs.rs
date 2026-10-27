//! Request logs and the logs API. See `setup_flow.rs` for how to point the tests at a
//! database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

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

fn encode(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Waits for log entries matching `filter` (they're written in the background).
async fn wait_for(app: &Arc<App>, token: &str, filter: &str, count: usize) -> Vec<Value> {
    for _ in 0..60 {
        let (status, list) = admin(
            app,
            token,
            "GET",
            // the polling requests themselves are logged too
            &format!(
                "/api/logs?perPage=100&sort=-@rowid&filter={}",
                encode(&format!("({filter}) && data.url !~ '/api/logs'"))
            ),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{list}");
        let items = list["items"].as_array().unwrap().clone();
        if items.len() >= count {
            return items;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no {count} log entries matching {filter}");
}

#[tokio::test]
async fn request_logs() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let (app, token, _dir) = ready_app(&tdb).await;

    // the logs API is for superusers
    let (status, _) = request(&app, "GET", "/api/logs", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    request(
        &app,
        "GET",
        "/api/collections/nope/records?x=1",
        &[
            ("user-agent", "test-agent"),
            ("referer", "https://example.com/"),
        ],
        None,
    )
    .await;
    admin(&app, &token, "GET", "/api/collections", None).await;
    request(&app, "GET", "/api/health", &[], None).await;
    raw_request(&app, "GET", "/_/", &[], vec![]).await;

    let errors = wait_for(&app, &token, "level = 8 && data.url ~ 'nope'", 1).await;
    let entry = &errors[0];
    assert_eq!(entry["message"], "GET /api/collections/nope/records?x=1");
    let data = &entry["data"];
    assert_eq!(data["type"], "request");
    assert_eq!(data["status"], 404);
    assert_eq!(data["method"], "GET");
    assert_eq!(data["auth"], "");
    assert_eq!(data["error"], "Missing collection context.");
    assert_eq!(data["userAgent"], "test-agent");
    assert_eq!(data["referer"], "https://example.com/");
    assert!(data["execTime"].as_f64().unwrap() >= 0.0);
    assert!(entry["created"].as_str().unwrap().ends_with('Z'));

    let superuser = wait_for(
        &app,
        &token,
        "data.auth = '_superusers' && data.url = '/api/collections'",
        1,
    )
    .await;
    assert_eq!(superuser[0]["level"], 0);
    assert_eq!(superuser[0]["data"]["status"], 200);
    assert!(
        superuser[0]["data"].get("authId").is_none(),
        "off by default"
    );

    // health checks and dashboard files aren't logged
    let (_, all) = admin(&app, &token, "GET", "/api/logs?perPage=200", None).await;
    for item in all["items"].as_array().unwrap() {
        let url = item["data"]["url"].as_str().unwrap_or_default();
        assert!(url != "/api/health" && !url.starts_with("/_/"), "{url}");
    }

    // view, stats, paging without totals
    let id = entry["id"].as_str().unwrap();
    let (status, one) = admin(&app, &token, "GET", &format!("/api/logs/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["id"], id);
    let (status, _) = admin(&app, &token, "GET", "/api/logs/missing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, stats) = admin(
        &app,
        &token,
        "GET",
        &format!(
            "/api/logs/stats?filter={}",
            encode("data.auth != '_superusers'")
        ),
        None,
    )
    .await;
    let bucket = &stats[0];
    assert!(
        bucket["date"].as_str().unwrap().ends_with(":00:00.000Z"),
        "{stats}"
    );
    assert!(bucket["total"].as_i64().unwrap() >= 1);
    let (_, page) = admin(&app, &token, "GET", "/api/logs?perPage=1&skipTotal=1", None).await;
    assert_eq!(page["totalItems"], -1);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let (status, _) = admin(&app, &token, "GET", "/api/logs?filter=nope%20%3D", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // settings: IPs from a trusted proxy header, auth ids, minimum level
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({
            "trustedProxy": { "headers": ["X-Forwarded-For"] },
            "logs": { "logAuthId": true },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    request(
        &app,
        "GET",
        "/api/collections/proxied/records",
        &[("x-forwarded-for", "203.0.113.7")],
        None,
    )
    .await;
    let proxied = wait_for(&app, &token, "data.url ~ 'proxied'", 1).await;
    assert_eq!(
        proxied[0]["data"]["userIP"], "203.0.113.7",
        "{}",
        proxied[0]
    );
    admin(&app, &token, "GET", "/api/collections?marker=1", None).await;
    let with_id = wait_for(&app, &token, "data.url ~ 'marker=1'", 1).await;
    assert_eq!(with_id[0]["data"]["authId"].as_str().unwrap().len(), 15);

    admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "logs": { "minLevel": 8, "logIP": false } })),
    )
    .await;
    admin(&app, &token, "GET", "/api/collections?marker=2", None).await;
    request(&app, "GET", "/api/collections/noip/records", &[], None).await;
    let noip = wait_for(&app, &token, "data.url ~ 'noip'", 1).await;
    assert!(noip[0]["data"].get("userIP").is_none());
    let (_, info) = admin(
        &app,
        &token,
        "GET",
        &format!(
            "/api/logs?filter={}",
            encode("data.url ~ 'marker=2' && data.url !~ '/api/logs'")
        ),
        None,
    )
    .await;
    assert_eq!(info["totalItems"], 0, "below the minimum level");

    // maxDataSize: entries keep the attributes that fit, and are marked
    let (status, _) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "logs": { "maxDataSize": -1 } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "logs": { "minLevel": 0, "maxDataSize": 200 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["logs"]["maxDataSize"], 200);
    let long = "x".repeat(500);
    request(
        &app,
        "GET",
        &format!("/api/collections/long/records?pad={long}"),
        &[],
        None,
    )
    .await;
    let cut = wait_for(&app, &token, "data.__pb_truncated__ = true", 1).await;
    let data = &cut[0]["data"];
    assert!(data.get("url").is_none(), "{data}");
    assert_eq!(
        data["method"], "GET",
        "the attributes before url are kept: {data}"
    );
    assert!(
        serde_json::to_string(data).unwrap().len() <= 200 + r#","__pb_truncated__":true"#.len(),
        "{data}"
    );
    // small entries are left as they are
    admin(&app, &token, "GET", "/api/collections?marker=3", None).await;
    let small = wait_for(&app, &token, "data.url ~ 'marker=3'", 1).await;
    assert!(small[0]["data"].get("__pb_truncated__").is_none());

    // maxDays 0 turns logging off
    admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "logs": { "maxDays": 0 } })),
    )
    .await;
    request(&app, "GET", "/api/collections/off/records", &[], None).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    let (_, off) = admin(
        &app,
        &token,
        "GET",
        &format!(
            "/api/logs?filter={}",
            encode("data.url ~ '/off/' && data.url !~ '/api/logs'")
        ),
        None,
    )
    .await;
    assert_eq!(off["totalItems"], 0);

    // old entries are deleted; DELETE /api/logs deletes everything
    let pool = tdb.pool();
    pool.get()
        .await
        .unwrap()
        .execute(
            "INSERT INTO _slonbase.logs (id, created, level, message) VALUES ('old0000000000aa', now() - interval '10 days', 0, 'old')",
            &[],
        )
        .await
        .unwrap();
    slonbase::logs::cleanup(&pool, 5).await.unwrap();
    let (status, _) = admin(&app, &token, "GET", "/api/logs/old0000000000aa", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = admin(&app, &token, "DELETE", "/api/logs", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, empty) = admin(&app, &token, "GET", "/api/logs", None).await;
    assert_eq!(empty["totalItems"], 0);

    drop(app);
    tdb.drop_db().await;
}
