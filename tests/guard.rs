//! Rate limits and the superuser IP allowlist. Client IPs come from a trusted
//! `X-Forwarded-For` header (tests don't go through a socket). See `setup_flow.rs` for how
//! to point the tests at a database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

async fn call(
    app: &Arc<App>,
    ip: &str,
    token: Option<&str>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = token.map(|t| format!("Bearer {t}"));
    let mut headers = vec![("x-forwarded-for", ip)];
    if let Some(auth) = &auth {
        headers.push(("authorization", auth.as_str()));
    }
    request(app, method, uri, &headers, body).await
}

#[tokio::test]
async fn rate_limits() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let admin = |method: &'static str, uri: &'static str, body: Option<Value>| {
        let (app, token) = (app.clone(), token.clone());
        async move { call(&app, "10.0.0.1", Some(&token), method, uri, body).await }
    };
    let (status, res) = admin(
        "POST",
        "/api/collections",
        Some(json!({ "name": "users", "type": "auth", "createRule": "", "listRule": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, res) = admin(
        "PATCH",
        "/api/settings",
        Some(json!({
            "trustedProxy": { "headers": ["X-Forwarded-For"] },
            "rateLimits": {
                "enabled": true,
                "excludedIPs": ["192.168.0.0/16"],
                "rules": [
                    { "label": "*:auth", "duration": 60, "maxRequests": 2 },
                    { "label": "users:list", "audience": "@guest", "duration": 60, "maxRequests": 1 },
                    { "label": "/api/", "duration": 60, "maxRequests": 100 },
                ],
            },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    let login = json!({ "identity": "nobody@example.com", "password": "wrong-password" });
    let sign_in = |ip: &'static str| {
        let (app, login) = (app.clone(), login.clone());
        async move {
            call(
                &app,
                ip,
                None,
                "POST",
                "/api/collections/users/auth-with-password",
                Some(login),
            )
            .await
            .0
        }
    };
    assert_eq!(sign_in("203.0.113.1").await, StatusCode::BAD_REQUEST);
    assert_eq!(sign_in("203.0.113.1").await, StatusCode::BAD_REQUEST);
    assert_eq!(sign_in("203.0.113.1").await, StatusCode::TOO_MANY_REQUESTS);
    let (_, body) = call(
        &app,
        "203.0.113.1",
        None,
        "POST",
        "/api/collections/users/auth-with-password",
        Some(login.clone()),
    )
    .await;
    assert_eq!(body["message"], "Too Many Requests.");
    assert_eq!(body["status"], 429);
    // per client, and excluded IPs aren't limited
    assert_eq!(sign_in("203.0.113.2").await, StatusCode::BAD_REQUEST);
    for _ in 0..4 {
        assert_eq!(sign_in("192.168.1.10").await, StatusCode::BAD_REQUEST);
    }
    // superusers aren't limited either
    for _ in 0..4 {
        let (status, _) = call(
            &app,
            "203.0.113.1",
            Some(&token),
            "POST",
            "/api/collections/users/auth-with-password",
            Some(login.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // audiences: the guest rule doesn't apply to signed-in records
    call(&app, "10.0.0.2", None, "POST", "/api/collections/users/records", Some(json!({
        "email": "member@example.com", "password": "member-password", "passwordConfirm": "member-password"
    }))).await;
    let (_, member) = call(
        &app,
        "10.0.0.2",
        None,
        "POST",
        "/api/collections/users/auth-with-password",
        Some(json!({
            "identity": "member@example.com", "password": "member-password"
        })),
    )
    .await;
    let member_token = member["token"].as_str().unwrap().to_string();
    let list = |ip: &'static str, token: Option<String>| {
        let app = app.clone();
        async move {
            call(
                &app,
                ip,
                token.as_deref(),
                "GET",
                "/api/collections/users/records",
                None,
            )
            .await
            .0
        }
    };
    assert_eq!(list("198.51.100.1", None).await, StatusCode::OK);
    assert_eq!(
        list("198.51.100.1", None).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        list("198.51.100.1", Some(member_token.clone())).await,
        StatusCode::OK
    );
    assert_eq!(
        list("198.51.100.1", Some(member_token)).await,
        StatusCode::OK
    );

    // changing the rules resets the counters; disabling turns limits off
    let (status, _) = admin(
        "PATCH",
        "/api/settings",
        Some(json!({ "rateLimits": { "rules": [
        { "label": "*:auth", "duration": 60, "maxRequests": 3 },
    ] } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sign_in("203.0.113.1").await, StatusCode::BAD_REQUEST);
    admin(
        "PATCH",
        "/api/settings",
        Some(json!({ "rateLimits": { "enabled": false } })),
    )
    .await;
    for _ in 0..5 {
        assert_eq!(sign_in("203.0.113.9").await, StatusCode::BAD_REQUEST);
    }

    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn superuser_ips() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, _) = call(
        &app,
        "10.0.0.1",
        Some(&token),
        "PATCH",
        "/api/settings",
        Some(json!({ "trustedProxy": { "headers": ["X-Forwarded-For"] }, "superuserIPs": ["10.0.0.0/8"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // superuser requests and superuser sign-in only from the allowed IPs
    let (status, _) = call(
        &app,
        "10.9.9.9",
        Some(&token),
        "GET",
        "/api/collections",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "8.8.8.8",
        Some(&token),
        "GET",
        "/api/collections",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let login = json!({ "identity": ADMIN_EMAIL, "password": ADMIN_PASSWORD });
    let (status, _) = call(
        &app,
        "8.8.8.8",
        None,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        Some(login.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &app,
        "10.1.2.3",
        None,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        Some(login),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // everyone else is unaffected
    let (status, _) = call(&app, "8.8.8.8", None, "GET", "/api/health", None).await;
    assert_eq!(status, StatusCode::OK);

    // a superuser's file token from outside counts as a guest's
    let (status, _) = call(
        &app,
        "10.0.0.1",
        Some(&token),
        "POST",
        "/api/collections",
        Some(json!({
            "name": "docs", "fields": [{ "name": "file", "type": "file", "protected": true }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (content_type, body) = multipart(&[], &[("file", "a.txt", b"secret")]);
    let auth = format!("Bearer {token}");
    let (status, _, created) = raw_request(
        &app,
        "POST",
        "/api/collections/docs/records",
        &[
            ("content-type", content_type.as_str()),
            ("authorization", auth.as_str()),
            ("x-forwarded-for", "10.0.0.1"),
        ],
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let record: Value = serde_json::from_slice(&created).unwrap();
    let (_, file_token) = call(
        &app,
        "10.0.0.1",
        Some(&token),
        "POST",
        "/api/files/token",
        None,
    )
    .await;
    let uri = format!(
        "/api/files/docs/{}/{}?token={}",
        record["id"].as_str().unwrap(),
        record["file"].as_str().unwrap(),
        file_token["token"].as_str().unwrap()
    );
    let (status, _, _) = raw_request(
        &app,
        "GET",
        &uri,
        &[("x-forwarded-for", "10.0.0.1")],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) =
        raw_request(&app, "GET", &uri, &[("x-forwarded-for", "8.8.8.8")], vec![]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    drop(app);
    tdb.drop_db().await;
}
