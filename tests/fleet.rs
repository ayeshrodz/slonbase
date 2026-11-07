//! Several instances on one database: tokens (stateless, and revoked everywhere at once), the
//! self-test of the notification path and what Health says about it, and `listen_url`. See
//! `scripts/fleet-test/` for the same questions asked of real processes behind a balancer.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::realtime::{SyncState, check_sync};

async fn as_admin(
    app: &std::sync::Arc<slonbase::app::App>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    request(app, method, uri, &[("authorization", auth.as_str())], body).await
}

/// A second instance on the same database, bootstrapped and aware of the first.
async fn second_instance(tdb: &TestDb) -> (std::sync::Arc<slonbase::app::App>, tempfile::TempDir) {
    let (dir, config) = temp_config();
    let app = new_app(&config, Some(tdb.url.clone()));
    app.bootstrap().await.unwrap();
    (app, dir)
}

async fn wait_for(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

/// Tokens carry no session: one instance's token works on the others, and a new token key
/// (a password change, or `revoke-tokens`) ends every token of the record on all of them.
#[tokio::test]
async fn tokens_work_on_every_instance_and_are_revoked_on_all_of_them() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (first, admin, _dir1) = ready_app(&tdb).await;
    let (second, _dir2) = second_instance(&tdb).await;

    // a superuser token from the first instance works on the second
    let (status, body) = as_admin(&second, &admin, "GET", "/api/collections", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = as_admin(
        &first,
        &admin,
        "POST",
        "/api/collections",
        Some(json!({ "name": "members", "type": "auth", "listRule": "", "viewRule": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        wait_for(|| second.core().unwrap().registry().get("members").is_some()).await,
        "the second instance has the collection"
    );
    let mut ids = Vec::new();
    for name in ["ann", "bob"] {
        let (status, body) = as_admin(
            &first,
            &admin,
            "POST",
            "/api/collections/members/records",
            Some(json!({
                "email": format!("{name}@example.com"),
                "password": "a-long-password-1",
                "passwordConfirm": "a-long-password-1",
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        ids.push(body["id"].as_str().unwrap().to_string());
    }
    let sign_in = |email: &'static str| {
        let (first, second) = (first.clone(), second.clone());
        async move {
            let (status, body) = request(
                &first,
                "POST",
                "/api/collections/members/auth-with-password",
                &[],
                Some(json!({ "identity": email, "password": "a-long-password-1" })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let token = body["token"].as_str().unwrap().to_string();
            // signed in on the first instance, refreshed on the second
            let (status, _) = request(
                &second,
                "POST",
                "/api/collections/members/auth-refresh",
                &[("authorization", token.as_str())],
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            token
        }
    };
    let ann = sign_in("ann@example.com").await;
    let bob = sign_in("bob@example.com").await;
    let revoke = |app: &std::sync::Arc<slonbase::app::App>, id: &str, token: &str| {
        let (app, uri, token) = (
            app.clone(),
            format!("/api/collections/members/records/{id}/revoke-tokens"),
            token.to_string(),
        );
        async move {
            request(
                &app,
                "POST",
                &uri,
                &[("authorization", token.as_str())],
                None,
            )
            .await
        }
    };

    // somebody else can't revoke ann's tokens; ann can, through the second instance
    let (status, _) = revoke(&second, &ids[0], &bob).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = request(
        &second,
        "POST",
        &format!("/api/collections/members/records/{}/revoke-tokens", ids[0]),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = revoke(&second, &ids[0], &ann).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    // her old token is refused by both instances, bob's still works on both
    for app in [&first, &second] {
        let (status, _) = request(
            app,
            "POST",
            "/api/collections/members/auth-refresh",
            &[("authorization", ann.as_str())],
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "ann's token after revoking"
        );
        let (status, _) = request(
            app,
            "POST",
            "/api/collections/members/auth-refresh",
            &[("authorization", bob.as_str())],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "bob's token is untouched");
    }
    // a superuser can revoke bob's, and a record that doesn't exist is a 404
    let (status, body) = as_admin(
        &first,
        &admin,
        "POST",
        &format!("/api/collections/members/records/{}/revoke-tokens", ids[1]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, _) = request(
        &second,
        "POST",
        "/api/collections/members/auth-refresh",
        &[("authorization", bob.as_str())],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = as_admin(
        &second,
        &admin,
        "POST",
        "/api/collections/members/records/doesnotexist001/revoke-tokens",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // a password change does the same, on every instance
    let carol = sign_in("ann@example.com").await;
    let (status, body) = as_admin(
        &second,
        &admin,
        "PATCH",
        &format!("/api/collections/members/records/{}", ids[0]),
        Some(json!({ "password": "another-password-2", "passwordConfirm": "another-password-2" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = request(
        &first,
        "POST",
        "/api/collections/members/auth-refresh",
        &[("authorization", carol.as_str())],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    drop((first, second));
    tdb.drop_db().await;
}

/// Instances send themselves a notification through the database and expect it back; Health
/// says so when it doesn't arrive, which is what a pooler in transaction mode does.
#[tokio::test]
async fn health_says_whether_notifications_arrive() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app(&tdb).await;
    let core = app.core().unwrap();
    check_sync(&core).await;
    assert_eq!(core.realtime.sync_state(), SyncState::Working);
    let (_, body) = as_admin(&app, &admin, "GET", "/api/system/status", None).await;
    let ids: Vec<&str> = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["status"] != "ok")
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&"sync"),
        "no complaint while it works: {ids:?}"
    );

    // a listener that listens somewhere else never hears the pings: the same as a pooler
    // dropping LISTEN. (Another database on the server: notifications don't cross databases.)
    let other = format!("slonbase_test_{}", slonbase::ids::random_id(10));
    admin_exec(&tdb.admin_url, &format!("CREATE DATABASE {other}")).await;
    let other_url = with_database(&tdb.admin_url, &other);
    let (dir2, config2) = temp_config();
    let deaf = new_app_configured(
        &config2,
        Some(tdb.url.clone()),
        slonbase::Extensions::default(),
        |o| o.database_listen_url = Some(other_url.clone()),
    );
    deaf.bootstrap().await.unwrap();
    let deaf_core = deaf.core().unwrap();
    check_sync(&deaf_core).await;
    assert_eq!(deaf_core.realtime.sync_state(), SyncState::Broken);
    let (status, body) = as_admin(&deaf, &admin, "GET", "/api/system/status", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sync = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "sync")
        .expect("a sync check");
    assert_eq!(sync["status"], "error");
    let detail = sync["detail"].as_str().unwrap();
    assert!(
        detail.contains("listen_url") && detail.contains("transaction mode"),
        "the check names the cause and the fix: {detail}"
    );

    // a listen_url that points at the same database works, and is what the docs recommend
    let (dir3, config3) = temp_config();
    let direct = new_app_configured(
        &config3,
        Some(tdb.url.clone()),
        slonbase::Extensions::default(),
        |o| o.database_listen_url = Some(tdb.url.clone()),
    );
    direct.bootstrap().await.unwrap();
    let direct_core = direct.core().unwrap();
    check_sync(&direct_core).await;
    assert_eq!(direct_core.realtime.sync_state(), SyncState::Working);

    drop((app, deaf, direct, core, deaf_core, direct_core, dir2, dir3));
    admin_exec(
        &tdb.admin_url,
        &format!("DROP DATABASE IF EXISTS {other} WITH (FORCE)"),
    )
    .await;
    tdb.drop_db().await;
}

/// Health counts the server's connections against its limit, whoever holds them.
#[tokio::test]
async fn health_reports_the_connection_budget_only_when_it_is_nearly_spent() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app(&tdb).await;
    let (status, body) = as_admin(&app, &admin, "GET", "/api/system/status", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "connections"),
        "a mostly idle server isn't flagged"
    );
    drop(app);
    tdb.drop_db().await;
}
