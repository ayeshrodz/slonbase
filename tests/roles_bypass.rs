//! A restricted dashboard role (developer, admin, viewer) must not reach what its role forbids by
//! another route to the same thing: the collection's id instead of its name, a batch request,
//! an import, a project's collections. See `setup_flow.rs` for how to point the tests at a
//! database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

const SUPERUSERS_ID: &str = "pbc_3142635823";

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

async fn member(app: &Arc<App>, owner: &str, email: &str, role: &str) -> String {
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
    login["token"].as_str().unwrap().to_string()
}

async fn superuser_count(app: &Arc<App>, owner: &str) -> u64 {
    let (_, list) = as_(
        app,
        owner,
        "GET",
        "/api/collections/_superusers/records",
        None,
    )
    .await;
    list["totalItems"].as_u64().unwrap()
}

#[tokio::test]
async fn restricted_roles_cannot_reach_superusers_another_way() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, owner, _dir) = ready_app(&tdb).await;
    let (_, posts) = as_(
        &app,
        &owner,
        "POST",
        "/api/collections",
        Some(json!({ "name": "posts", "fields": [{ "name": "title", "type": "text" }] })),
    )
    .await;
    let posts_id = posts["id"].as_str().unwrap().to_string();
    let developer = member(&app, &owner, "dev@example.com", "developer").await;
    let admin = member(&app, &owner, "lead@example.com", "admin").await;
    let viewer = member(&app, &owner, "viewer@example.com", "viewer").await;
    // batch requests are off by default
    let (status, res) = as_(
        &app,
        &owner,
        "PATCH",
        "/api/settings",
        Some(json!({ "batch": { "enabled": true } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let before = superuser_count(&app, &owner).await;

    let new_account = |n: u32| {
        json!({
            "email": format!("planted{n}@example.com"),
            "password": "planted-password-1",
            "passwordConfirm": "planted-password-1"
        })
    };

    // the same operation by name and by id: refused for every restricted role
    let mut n = 0;
    for (who, token) in [
        ("developer", &developer),
        ("admin", &admin),
        ("viewer", &viewer),
    ] {
        for target in ["_superusers", SUPERUSERS_ID] {
            n += 1;
            let (status, res) = as_(
                &app,
                token,
                "POST",
                &format!("/api/collections/{target}/records"),
                Some(new_account(n)),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{who}: create via {target}: {res}"
            );

            let (status, res) = as_(
                &app,
                token,
                "PATCH",
                &format!("/api/collections/{target}"),
                Some(json!({ "listRule": "" })),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{who}: change {target}: {res}"
            );

            let (status, res) = as_(
                &app,
                token,
                "DELETE",
                &format!("/api/collections/{target}/records/{}", "x".repeat(15)),
                None,
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{who}: delete in {target}: {res}"
            );

            let (status, res) = as_(
                &app,
                token,
                "DELETE",
                &format!("/api/collections/{target}/truncate"),
                None,
            )
            .await;
            assert!(
                status.is_client_error(),
                "{who}: truncate {target}: {status} {res}"
            );
        }
    }

    // a batch can't do what a direct request can't
    for (who, token) in [
        ("developer", &developer),
        ("admin", &admin),
        ("viewer", &viewer),
    ] {
        for target in ["_superusers", SUPERUSERS_ID] {
            n += 1;
            let (status, res) = as_(
                &app,
                token,
                "POST",
                "/api/batch",
                Some(json!({ "requests": [
                    { "method": "POST", "url": format!("/api/collections/{target}/records"), "body": new_account(n) }
                ]})),
            )
            .await;
            assert!(
                !status.is_success(),
                "{who}: batch create via {target}: {status} {res}"
            );
        }
    }
    // an ordinary batch still works for a developer, and a mixed one commits nothing
    let (status, res) = as_(
        &app,
        &developer,
        "POST",
        "/api/batch",
        Some(json!({ "requests": [
            { "method": "POST", "url": "/api/collections/posts/records", "body": { "title": "ok" } }
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    n += 1;
    let (status, res) = as_(
        &app,
        &developer,
        "POST",
        "/api/batch",
        Some(json!({ "requests": [
            { "method": "POST", "url": "/api/collections/posts/records", "body": { "title": "must not stay" } },
            { "method": "POST", "url": format!("/api/collections/{SUPERUSERS_ID}/records"), "body": new_account(n) }
        ]})),
    )
    .await;
    assert!(!status.is_success(), "mixed batch: {status} {res}");
    let (_, list) = as_(
        &app,
        &owner,
        "GET",
        "/api/collections/posts/records?filter=title%3D%27must%20not%20stay%27",
        None,
    )
    .await;
    assert_eq!(
        list["totalItems"], 0,
        "the mixed batch left a record: {list}"
    );

    // an import can't replace the superusers collection either (by name or id)
    for who_token in [&developer, &admin] {
        let (status, res) = as_(
            &app,
            who_token,
            "PUT",
            "/api/collections/import",
            Some(json!({ "collections": [
                { "id": SUPERUSERS_ID, "name": "_superusers", "type": "auth", "listRule": "", "viewRule": "" }
            ], "deleteMissing": false })),
        )
        .await;
        assert!(!status.is_success(), "import: {status} {res}");
    }

    // ordinary collections, by name or id, still work for a developer
    for target in ["posts", posts_id.as_str()] {
        let (status, res) = as_(
            &app,
            &developer,
            "POST",
            &format!("/api/collections/{target}/records"),
            Some(json!({ "title": "fine" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "developer: {target}: {res}");
    }
    // and nothing was planted or changed
    assert_eq!(superuser_count(&app, &owner).await, before);
    let (_, su) = as_(
        &app,
        &owner,
        "GET",
        &format!("/api/collections/{SUPERUSERS_ID}"),
        None,
    )
    .await;
    assert_ne!(su["listRule"], "", "{su}");

    tdb.drop_db().await;
}

/// An API key acts for its superuser, with at most its own role: it must not be able to change
/// the credentials of the owner it belongs to, or end its sessions.
#[tokio::test]
async fn api_keys_cannot_edit_their_owner() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, owner, _dir) = ready_app(&tdb).await;
    let (_, me) = as_(&app, &owner, "GET", "/api/team/me", None).await;
    let owner_id = me["id"].as_str().unwrap().to_string();
    for role in ["viewer", "developer", "admin"] {
        let (status, key) = as_(
            &app,
            &owner,
            "POST",
            "/api/team/keys",
            Some(json!({ "name": format!("{role} key"), "role": role })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{key}");
        let key = key["key"].as_str().unwrap().to_string();
        for target in ["_superusers", SUPERUSERS_ID] {
            let uri = format!("/api/collections/{target}/records/{owner_id}");
            let (status, res) = as_(
                &app,
                &key,
                "PATCH",
                &uri,
                Some(json!({ "password": "hijacked-password-1", "passwordConfirm": "hijacked-password-1" })),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{role} key: change owner via {target}: {res}"
            );
            let (status, res) =
                as_(&app, &key, "POST", &format!("{uri}/revoke-tokens"), None).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{role} key: revoke owner's tokens via {target}: {res}"
            );
        }
    }
    // the owner's password is unchanged, and the owner's own session can still change it
    let (status, _) = request(
        &app,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        &[],
        Some(json!({ "identity": ADMIN_EMAIL, "password": ADMIN_PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, res) = as_(
        &app,
        &owner,
        "PATCH",
        &format!("/api/collections/_superusers/records/{owner_id}"),
        Some(json!({ "password": "new-owner-password-1", "passwordConfirm": "new-owner-password-1", "oldPassword": ADMIN_PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    tdb.drop_db().await;
}

/// The last owner can't be deleted, by name or by id, while other superusers remain.
#[tokio::test]
async fn the_last_owner_stays() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, owner, _dir) = ready_app(&tdb).await;
    let (_, me) = as_(&app, &owner, "GET", "/api/team/me", None).await;
    let owner_id = me["id"].as_str().unwrap().to_string();
    member(&app, &owner, "viewer@example.com", "viewer").await;
    for target in ["_superusers", SUPERUSERS_ID] {
        let (status, res) = as_(
            &app,
            &owner,
            "DELETE",
            &format!("/api/collections/{target}/records/{owner_id}"),
            None,
        )
        .await;
        assert!(
            status.is_client_error(),
            "delete last owner via {target}: {status} {res}"
        );
    }
    let (status, _) = as_(&app, &owner, "GET", "/api/team/me", None).await;
    assert_eq!(status, StatusCode::OK, "the owner is still there");

    tdb.drop_db().await;
}

/// The SQL console reads every table (password hashes, token keys, the signing secret): only
/// owners may use it, whatever the console's own settings.
#[tokio::test]
async fn only_owners_use_the_sql_console() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, owner, _dir) = ready_app(&tdb).await;
    let developer = member(&app, &owner, "dev@example.com", "developer").await;
    let admin = member(&app, &owner, "lead@example.com", "admin").await;
    let viewer = member(&app, &owner, "viewer@example.com", "viewer").await;
    // the console is off by default (which refuses everyone, so it proves nothing): a second
    // instance on the same database has it on
    let (_dir2, config) = temp_config();
    std::fs::write(&config, "[server]\nsql_console = true\n").unwrap();
    let app = new_app(&config, Some(tdb.url.clone()));
    app.bootstrap().await.unwrap();
    let query = json!({ "query": "SELECT 1 AS one" });
    let (status, res) = as_(&app, &owner, "POST", "/api/sql", Some(query.clone())).await;
    assert_eq!(status, StatusCode::OK, "the owner can use it: {res}");
    for (who, token) in [
        ("developer", &developer),
        ("admin", &admin),
        ("viewer", &viewer),
    ] {
        let (status, res) = as_(&app, token, "POST", "/api/sql", Some(query.clone())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}: {res}");
        assert!(
            !res.to_string().contains("disabled"),
            "{who} was refused by the role, not by the console being off: {res}"
        );
    }
    tdb.drop_db().await;
}
