//! Dashboard roles and API keys (2.0): what owners, admins, developers and viewers may do,
//! that there's always an owner, and keys that act for a superuser with a role of their own.
//! See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

async fn as_(
    app: &Arc<App>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    request(app, method, uri, &[("authorization", auth.as_str())], body).await
}

/// A superuser with a role, signed in.
async fn member(app: &Arc<App>, owner: &str, email: &str, role: &str) -> (String, String) {
    let (status, res) = as_(
        app,
        owner,
        "POST",
        "/api/collections/_superusers/records",
        Some(json!({ "email": email, "password": "member-password-1", "passwordConfirm": "member-password-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let id = res["id"].as_str().unwrap().to_string();
    let (status, res) = as_(
        app,
        owner,
        "PATCH",
        &format!("/api/team/members/{id}"),
        Some(json!({ "role": role })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, login) = request(
        app,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        &[],
        Some(json!({ "identity": email, "password": "member-password-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    (id, login["token"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn roles() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, owner, _dir) = ready_app(&tdb).await;
    let (status, posts) = as_(
        &app,
        &owner,
        "POST",
        "/api/collections",
        Some(json!({ "name": "posts", "fields": [{ "name": "title", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{posts}");
    as_(
        &app,
        &owner,
        "POST",
        "/api/backups",
        Some(json!({ "name": "b.zip" })),
    )
    .await;

    let (dev_id, mut dev) = member(&app, &owner, "dev@example.com", "developer").await;
    let (_, viewer) = member(&app, &owner, "viewer@example.com", "viewer").await;
    let (_, admin) = member(&app, &owner, "lead@example.com", "admin").await;

    let (_, me) = as_(&app, &dev, "GET", "/api/team/me", None).await;
    assert_eq!(me["role"], "developer");
    let (_, me) = as_(&app, &owner, "GET", "/api/team/me", None).await;
    assert_eq!(me["role"], "owner");

    // everyone reads
    for token in [&dev, &viewer, &admin] {
        let (status, _) = as_(&app, token, "GET", "/api/collections", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = as_(&app, token, "GET", "/api/settings", None).await;
        assert_eq!(status, StatusCode::OK);
    }
    // viewers change nothing, nor download the installation
    let (status, res) = as_(
        &app,
        &viewer,
        "POST",
        "/api/collections/posts/records",
        Some(json!({ "title": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");
    assert!(res["message"].as_str().unwrap().contains("viewer"));
    let file_token = async |token: &str| -> String {
        let (_, res) = as_(&app, token, "POST", "/api/files/token", None).await;
        res["token"].as_str().unwrap().to_string()
    };
    let t = file_token(&viewer).await;
    let (status, _, _) = raw_request(
        &app,
        "GET",
        &format!("/api/backups/b.zip?token={t}"),
        &[],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // but refresh their session
    let (status, _) = as_(
        &app,
        &viewer,
        "POST",
        "/api/collections/_superusers/auth-refresh",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // developers work on the data, not on the settings, backups or superusers
    let (status, _) = as_(
        &app,
        &dev,
        "POST",
        "/api/collections/posts/records",
        Some(json!({ "title": "by dev" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = as_(
        &app,
        &dev,
        "PATCH",
        "/api/collections/posts",
        Some(json!({ "listRule": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for (method, uri, body) in [
        (
            "PATCH",
            "/api/settings",
            Some(json!({ "meta": { "appName": "x" } })),
        ),
        ("POST", "/api/backups", Some(json!({}))),
        (
            "POST",
            "/api/collections/_superusers/records",
            Some(json!({ "email": "x@example.com" })),
        ),
        (
            "PATCH",
            "/api/team/members/x",
            Some(json!({ "role": "owner" })),
        ),
    ] {
        let (status, _) = as_(&app, &dev, method, uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
    // their own password, though
    let (status, res) = as_(
        &app,
        &dev,
        "PATCH",
        &format!("/api/collections/_superusers/records/{dev_id}"),
        Some(json!({ "password": "member-password-2", "passwordConfirm": "member-password-2", "oldPassword": "member-password-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    // (which ended their sessions)
    let (_, login) = request(
        &app,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        &[],
        Some(json!({ "identity": "dev@example.com", "password": "member-password-2" })),
    )
    .await;
    dev = login["token"].as_str().unwrap().to_string();

    // admins: the settings and backups, not roles, superusers or restores
    let (status, _) = as_(
        &app,
        &admin,
        "PATCH",
        "/api/settings",
        Some(json!({ "meta": { "appName": "Acme" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let t = file_token(&admin).await;
    let (status, _, _) = raw_request(
        &app,
        "GET",
        &format!("/api/backups/b.zip?token={t}"),
        &[],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = as_(&app, &admin, "POST", "/api/backups/b.zip/restore", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = as_(
        &app,
        &admin,
        "PATCH",
        &format!("/api/team/members/{dev_id}"),
        Some(json!({ "role": "owner" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "no promoting");
    let (status, _) = as_(
        &app,
        &admin,
        "DELETE",
        &format!("/api/collections/_superusers/records/{dev_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // the roles aren't settings: not sent, not changed through them
    let (_, s) = as_(&app, &owner, "GET", "/api/settings", None).await;
    assert!(
        s.get("superuserRoles").is_none() && s.get("apiKeys").is_none(),
        "{s}"
    );
    let (status, _) = as_(
        &app,
        &admin,
        "PATCH",
        "/api/settings",
        Some(json!({ "superuserRoles": {}, "apiKeys": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, me) = as_(&app, &dev, "GET", "/api/team/me", None).await;
    assert_eq!(me["role"], "developer", "unchanged");

    // there's always an owner
    let (_, team) = as_(&app, &owner, "GET", "/api/team", None).await;
    let owner_id = team["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "owner")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, res) = as_(
        &app,
        &owner,
        "PATCH",
        &format!("/api/team/members/{owner_id}"),
        Some(json!({ "role": "admin" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    let (status, res) = as_(
        &app,
        &owner,
        "DELETE",
        &format!("/api/collections/_superusers/records/{owner_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    // with another owner, it can go
    let (status, _) = as_(
        &app,
        &owner,
        "PATCH",
        &format!("/api/team/members/{dev_id}"),
        Some(json!({ "role": "owner" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, me) = as_(&app, &dev, "GET", "/api/team/me", None).await;
    assert_eq!(me["role"], "owner");
    let (status, _) = as_(
        &app,
        &owner,
        "PATCH",
        &format!("/api/team/members/{owner_id}"),
        Some(json!({ "role": "viewer" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    tdb.drop_db().await;
}

#[tokio::test]
async fn api_keys() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, owner, _dir) = ready_app(&tdb).await;
    as_(
        &app,
        &owner,
        "POST",
        "/api/collections",
        Some(json!({ "name": "posts", "fields": [{ "name": "title", "type": "text" }] })),
    )
    .await;

    // a key, shown once
    let (status, key) = as_(
        &app,
        &owner,
        "POST",
        "/api/team/keys",
        Some(json!({ "name": "CI reader", "role": "viewer" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{key}");
    let secret = key["key"].as_str().unwrap().to_string();
    assert!(secret.starts_with("sb_") && secret.len() > 40);
    assert!(secret.starts_with(key["prefix"].as_str().unwrap()));
    let (_, team) = as_(&app, &owner, "GET", "/api/team", None).await;
    let listed = team["apiKeys"].to_string();
    assert!(!listed.contains(&secret), "never shown again");
    assert!(!listed.contains("hash"), "{listed}");

    // it acts for the superuser, with its role
    let (status, _) = as_(&app, &secret, "GET", "/api/collections/posts/records", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = as_(
        &app,
        &secret,
        "POST",
        "/api/collections/posts/records",
        Some(json!({ "title": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, me) = as_(&app, &secret, "GET", "/api/team/me", None).await;
    assert_eq!(me["role"], "viewer");
    // keys make no file tokens (those would act with the superuser's role)
    let (status, _) = as_(&app, &secret, "POST", "/api/files/token", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // a wrong key is no one
    let (status, _) = as_(&app, &format!("{secret}x"), "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // keys can be held to IPs (tests come from no IP: refused)
    let (_, limited) = as_(
        &app,
        &owner,
        "POST",
        "/api/team/keys",
        Some(json!({ "name": "office", "role": "admin", "allowedIPs": ["10.0.0.0/8"] })),
    )
    .await;
    let (status, _) = as_(
        &app,
        limited["key"].as_str().unwrap(),
        "GET",
        "/api/settings",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // no key above its maker's role
    let (status, writer) = as_(
        &app,
        &owner,
        "POST",
        "/api/team/keys",
        Some(json!({ "name": "deploy", "role": "admin", "expiresDays": 30 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!writer["expires"].as_str().unwrap().is_empty());
    let writer_key = writer["key"].as_str().unwrap().to_string();
    let (status, res) = as_(
        &app,
        &writer_key,
        "POST",
        "/api/team/keys",
        Some(json!({ "name": "escalate", "role": "owner" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    let (status, _) = as_(
        &app,
        &writer_key,
        "POST",
        "/api/collections/posts/records",
        Some(json!({ "title": "via key" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // revoked
    let (status, _) = as_(
        &app,
        &owner,
        "DELETE",
        &format!("/api/team/keys/{}", key["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = as_(&app, &secret, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    tdb.drop_db().await;
}
