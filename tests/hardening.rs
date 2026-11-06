//! 2.1: the default file size and thumbnails (`files`), lockout after failed sign-ins and the
//! interval between auth emails (`security`), and the rate-limit tester.
//! See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;
use slonbase::mail::MemoryMailer;

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

#[tokio::test]
async fn file_settings() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "docs", "createRule": "", "viewRule": "",
            "fields": [{ "name": "file", "type": "file", "thumbs": ["20x20"] }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let upload = async |size: usize| {
        let (content_type, body) = multipart(&[], &[("file", "a.png", &vec![1u8; size])]);
        let (status, _, bytes) = raw_request(
            &app,
            "POST",
            "/api/collections/docs/records",
            &[("content-type", content_type.as_str())],
            body,
        )
        .await;
        (status, serde_json::from_slice::<Value>(&bytes).unwrap())
    };

    // 5 MB by default
    let (status, _) = upload(2 << 20).await;
    assert_eq!(status, StatusCode::OK);
    let (status, res) = upload(6 << 20).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    // the setting moves it
    settings(
        &app,
        &token,
        json!({ "files": { "defaultMaxSize": 1 << 20 } }),
    )
    .await;
    let (status, res) = upload(2 << 20).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(
        res["data"]["file"]["message"]
            .as_str()
            .unwrap()
            .contains("1048576")
    );
    settings(
        &app,
        &token,
        json!({ "files": { "defaultMaxSize": 8 << 20 } }),
    )
    .await;
    let (status, _) = upload(6 << 20).await;
    assert_eq!(status, StatusCode::OK);
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "files": { "defaultMaxSize": -1 } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    tdb.drop_db().await;
}

#[tokio::test]
async fn lockout_and_email_interval() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let mailer = Arc::new(MemoryMailer::default());
    app.core().unwrap().set_mailer(mailer.clone());
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "users", "type": "auth", "createRule": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    for email in ["ada@example.com", "bob@example.com"] {
        let (status, res) = request(
            &app,
            "POST",
            "/api/collections/users/records",
            &[],
            Some(json!({ "email": email, "password": "right-password", "passwordConfirm": "right-password" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    let login = async |email: &str, password: &str| {
        request(
            &app,
            "POST",
            "/api/collections/users/auth-with-password",
            &[],
            Some(json!({ "identity": email, "password": password })),
        )
        .await
    };

    // off by default: failures are just failures
    for _ in 0..4 {
        assert_eq!(
            login("ada@example.com", "nope").await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        login("ada@example.com", "right-password").await.0,
        StatusCode::OK
    );

    // three failures lock the identity out, even for the right password
    settings(
        &app,
        &token,
        json!({ "security": { "loginLockout": { "attempts": 3, "minutes": 5 } } }),
    )
    .await;
    for _ in 0..3 {
        assert_eq!(
            login("ada@example.com", "nope").await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, res) = login("ada@example.com", "right-password").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{res}");
    assert!(res["message"].as_str().unwrap().contains("5 minute"));
    // case doesn't dodge it, others aren't affected
    assert_eq!(
        login("ADA@example.com", "right-password").await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        login("bob@example.com", "right-password").await.0,
        StatusCode::OK
    );
    // a success clears an identity's failures
    for _ in 0..2 {
        login("bob@example.com", "nope").await;
    }
    assert_eq!(
        login("bob@example.com", "right-password").await.0,
        StatusCode::OK
    );
    for _ in 0..2 {
        login("bob@example.com", "nope").await;
    }
    assert_eq!(
        login("bob@example.com", "right-password").await.0,
        StatusCode::OK
    );
    // the lockout ends with its window
    {
        let client = tdb.pool().get().await.unwrap();
        client
            .execute(
                "UPDATE _slonbase.rate_limits SET started = now() - interval '10 minutes' WHERE key LIKE 'lockout|%'",
                &[],
            )
            .await
            .unwrap();
    }
    assert_eq!(
        login("ada@example.com", "right-password").await.0,
        StatusCode::OK
    );
    // turned off again
    settings(
        &app,
        &token,
        json!({ "security": { "loginLockout": { "attempts": 0 } } }),
    )
    .await;

    // one auth email per account per interval; 60 seconds by default
    let ask = async || {
        request(
            &app,
            "POST",
            "/api/collections/users/request-password-reset",
            &[],
            Some(json!({ "email": "bob@example.com" })),
        )
        .await
    };
    let sent = || {
        mailer
            .sent
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.to == "bob@example.com")
            .count()
    };
    ask().await;
    ask().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        sent(),
        1,
        "the second request within the interval sends nothing"
    );
    // a shorter interval, and the claim is old enough
    settings(
        &app,
        &token,
        json!({ "security": { "authEmailInterval": 5 } }),
    )
    .await;
    {
        let client = tdb.pool().get().await.unwrap();
        client
            .execute(
                "UPDATE _slonbase.mail_sent SET sent_at = now() - interval '6 seconds'",
                &[],
            )
            .await
            .unwrap();
    }
    ask().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(sent(), 2, "an interval of 5 seconds has passed");
    let (status, _) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "security": { "authEmailInterval": 1, "loginLockout": { "attempts": 5, "minutes": 0 } } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    tdb.drop_db().await;
}

#[tokio::test]
async fn rate_limit_tester() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    settings(
        &app,
        &token,
        json!({ "rateLimits": { "enabled": true, "rules": [
            { "label": "*:auth", "audience": "", "duration": 3, "maxRequests": 2 },
            { "label": "POST /api/batch", "audience": "@guest", "duration": 1, "maxRequests": 5 },
            { "label": "/api/", "audience": "@auth", "duration": 10, "maxRequests": 300 },
        ] } }),
    )
    .await;
    let (status, _) = request(
        &app,
        "POST",
        "/api/settings/rate-limits/explain",
        &[],
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let explain = async |method: &str, path: &str, audience: &str| {
        let (status, res) = admin(
            &app,
            &token,
            "POST",
            "/api/settings/rate-limits/explain",
            Some(json!({ "method": method, "path": path, "audience": audience })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{res}");
        res
    };
    let res = explain(
        "POST",
        "/api/collections/_superusers/auth-with-password",
        "@guest",
    )
    .await;
    assert_eq!(res["rule"]["label"], "*:auth");
    assert_eq!(res["index"], 0);
    assert_eq!(res["enabled"], true);
    assert_eq!(
        explain("POST", "/api/batch", "@guest").await["rule"]["maxRequests"],
        5
    );
    // the batch rule is for guests; a signed-in caller falls to the prefix rule
    assert_eq!(
        explain("POST", "/api/batch", "@auth").await["rule"]["maxRequests"],
        300
    );
    assert_eq!(
        explain("GET", "/_/anything", "@guest").await["rule"],
        Value::Null
    );
    // a query string doesn't matter
    assert_eq!(
        explain("POST", "/api/batch?x=1", "@guest").await["rule"]["label"],
        "POST /api/batch"
    );
    let (status, _) = admin(
        &app,
        &token,
        "POST",
        "/api/settings/rate-limits/explain",
        Some(json!({ "method": "GET", "path": "no-slash" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    tdb.drop_db().await;
}
