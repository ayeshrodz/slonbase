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

/// Two owners deleting each other at the same moment: one of the deletes is refused, since
/// the other owner is gone by then.
#[tokio::test]
async fn owners_deleting_each_other_leave_one() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, first, _dir) = ready_app(&tdb).await;
    let second = member(&app, &first, "second-owner@example.com", "owner").await;
    let (_, me) = as_(&app, &first, "GET", "/api/team/me", None).await;
    let (_, other) = as_(&app, &second, "GET", "/api/team/me", None).await;
    let (first_id, second_id) = (
        me["id"].as_str().unwrap().to_string(),
        other["id"].as_str().unwrap().to_string(),
    );
    assert_eq!(superuser_count(&app, &first).await, 2);

    let delete = |token: String, id: String| {
        let app = app.clone();
        async move {
            as_(
                &app,
                &token,
                "DELETE",
                &format!("/api/collections/_superusers/records/{id}"),
                None,
            )
            .await
            .0
        }
    };
    let (a, b) = tokio::join!(
        delete(first.clone(), second_id),
        delete(second.clone(), first_id)
    );
    // one delete goes through; the other is refused (as the last owner, or, if the first
    // had finished before its caller was authenticated, as no longer signed in)
    let ok = [a, b]
        .iter()
        .filter(|s| **s == StatusCode::NO_CONTENT)
        .count();
    assert_eq!(ok, 1, "exactly one delete goes through: {a} and {b}");
    let refused = if a == StatusCode::NO_CONTENT { b } else { a };
    assert!(
        matches!(
            refused,
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "the other is refused: {refused}"
    );
    // one owner remains, whichever it is
    let survivor = if a == StatusCode::NO_CONTENT {
        &first
    } else {
        &second
    };
    assert_eq!(superuser_count(&app, survivor).await, 1);
    tdb.drop_db().await;
}

/// A connection of another instance, with its transaction holding the database-wide lock of
/// changes to the owners.
async fn another_instance_holds_the_owners_lock(
    tdb: &TestDb,
) -> (tokio_postgres::Client, tokio::task::JoinHandle<()>) {
    let (client, connection) = tokio_postgres::connect(&tdb.url, tokio_postgres::NoTls)
        .await
        .unwrap();
    let handle = tokio::spawn(async move {
        let _ = connection.await;
    });
    client.batch_execute("BEGIN").await.unwrap();
    client
        .execute(
            "SELECT pg_advisory_xact_lock($1)",
            &[&slonbase::roles::OWNERS_LOCK_KEY],
        )
        .await
        .unwrap();
    (client, handle)
}

/// The check that an owner remains and the delete are one step across instances, too: while
/// another instance changes the owners (it holds the database-wide lock), a delete of an owner
/// waits for it, and a role change and a batch with it.
#[tokio::test]
async fn changes_to_the_owners_wait_for_another_instance() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, first, _dir) = ready_app(&tdb).await;
    let second = member(&app, &first, "second-owner@example.com", "owner").await;
    let (_, other) = as_(&app, &second, "GET", "/api/team/me", None).await;
    let second_id = other["id"].as_str().unwrap().to_string();

    let (status, res) = as_(
        &app,
        &first,
        "PATCH",
        "/api/settings",
        Some(json!({ "batch": { "enabled": true } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    let (other_instance, handle) = another_instance_holds_the_owners_lock(&tdb).await;

    // a delete, a role change and a batch deleting the owner all wait
    let delete = {
        let (app, first, id) = (app.clone(), first.clone(), second_id.clone());
        tokio::spawn(async move {
            as_(
                &app,
                &first,
                "DELETE",
                &format!("/api/collections/_superusers/records/{id}"),
                None,
            )
            .await
            .0
        })
    };
    let demote = {
        let (app, first, id) = (app.clone(), first.clone(), second_id.clone());
        tokio::spawn(async move {
            as_(
                &app,
                &first,
                "PATCH",
                &format!("/api/team/members/{id}"),
                Some(json!({ "role": "viewer" })),
            )
            .await
            .0
        })
    };
    let batch = {
        let (app, first, id) = (app.clone(), first.clone(), second_id.clone());
        tokio::spawn(async move {
            as_(
                &app,
                &first,
                "POST",
                "/api/batch",
                Some(json!({ "requests": [{
                    "method": "DELETE",
                    "url": format!("/api/collections/_superusers/records/{id}"),
                }] })),
            )
            .await
            .0
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(!delete.is_finished(), "the delete went on without the lock");
    assert!(
        !demote.is_finished(),
        "the role change went on without the lock"
    );
    assert!(!batch.is_finished(), "the batch went on without the lock");
    assert_eq!(
        superuser_count(&app, &first).await,
        2,
        "nothing changed while the lock was held"
    );

    // released: they go on, one after the other (the delete and the batch are not both
    // possible, and the role change may come before or after them)
    other_instance.batch_execute("COMMIT").await.unwrap();
    let results = (
        delete.await.unwrap(),
        demote.await.unwrap(),
        batch.await.unwrap(),
    );
    handle.abort();
    let deleted = [results.0, results.2]
        .iter()
        .filter(|s| **s == StatusCode::NO_CONTENT || **s == StatusCode::OK)
        .count();
    assert!(deleted <= 1, "the owner is deleted once: {results:?}");
    assert_eq!(
        superuser_count(&app, &first).await,
        if deleted == 1 { 1 } else { 2 },
        "{results:?}"
    );
    tdb.drop_db().await;
}

/// Two owners deleting each other through two instances on one database: one delete is
/// refused, and an owner remains, every time.
#[tokio::test]
async fn owners_deleting_each_other_through_two_instances_leave_one() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (first_app, first, _dir) = ready_app(&tdb).await;
    // a second instance on the same database
    let (_dir2, config2) = temp_config();
    let second_app = new_app(&config2, Some(tdb.url.clone()));
    second_app.bootstrap().await.unwrap();

    let second = member(&first_app, &first, "second-owner@example.com", "owner").await;
    let (_, me) = as_(&first_app, &first, "GET", "/api/team/me", None).await;
    let (_, other) = as_(&first_app, &second, "GET", "/api/team/me", None).await;
    let (first_id, second_id) = (
        me["id"].as_str().unwrap().to_string(),
        other["id"].as_str().unwrap().to_string(),
    );
    let delete = |app: Arc<App>, token: String, id: String| async move {
        as_(
            &app,
            &token,
            "DELETE",
            &format!("/api/collections/_superusers/records/{id}"),
            None,
        )
        .await
        .0
    };
    let (a, b) = tokio::join!(
        delete(first_app.clone(), first.clone(), second_id),
        delete(second_app.clone(), second.clone(), first_id)
    );
    let ok = [a, b]
        .iter()
        .filter(|s| **s == StatusCode::NO_CONTENT)
        .count();
    assert_eq!(ok, 1, "exactly one delete goes through: {a} and {b}");
    let survivor = if a == StatusCode::NO_CONTENT {
        &first
    } else {
        &second
    };
    assert_eq!(superuser_count(&first_app, survivor).await, 1);
    tdb.drop_db().await;
}

/// A batch deleting every owner is refused like the same deletes one at a time, and so is
/// demoting the owner a delete would leave.
#[tokio::test]
async fn a_batch_cannot_delete_the_last_owners() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, first, _dir) = ready_app(&tdb).await;
    let second = member(&app, &first, "second-owner@example.com", "owner").await;
    let _viewer = member(&app, &first, "viewer-only@example.com", "viewer").await;
    let (status, res) = as_(
        &app,
        &first,
        "PATCH",
        "/api/settings",
        Some(json!({ "batch": { "enabled": true } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (_, me) = as_(&app, &first, "GET", "/api/team/me", None).await;
    let (_, other) = as_(&app, &second, "GET", "/api/team/me", None).await;
    let urls: Vec<String> = [&me, &other]
        .iter()
        .map(|m| {
            format!(
                "/api/collections/_superusers/records/{}",
                m["id"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(superuser_count(&app, &first).await, 3);

    let requests: Vec<Value> = urls
        .iter()
        .map(|url| json!({ "method": "DELETE", "url": url }))
        .collect();
    let (status, res) = as_(
        &app,
        &first,
        "POST",
        "/api/batch",
        Some(json!({ "requests": requests })),
    )
    .await;
    assert!(
        !status.is_success(),
        "both owners in one batch: {status} {res}"
    );
    assert_eq!(
        superuser_count(&app, &first).await,
        3,
        "nothing was deleted"
    );

    // one of them is fine, as it is directly
    let (status, res) = as_(
        &app,
        &first,
        "POST",
        "/api/batch",
        Some(json!({ "requests": [{ "method": "DELETE", "url": urls[1] }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(superuser_count(&app, &first).await, 2);
    tdb.drop_db().await;
}
