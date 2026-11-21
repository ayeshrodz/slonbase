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

/// Runs `request` while another instance holds the database-wide owners lock: it must not go
/// on until the lock is released, and its status then is returned.
async fn waits_for_the_owners_lock(
    tdb: &TestDb,
    request: impl std::future::Future<Output = StatusCode> + Send + 'static,
) -> StatusCode {
    let (other_instance, handle) = another_instance_holds_the_owners_lock(tdb).await;
    let task = tokio::spawn(request);
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(!task.is_finished(), "it went on without the lock");
    other_instance.batch_execute("COMMIT").await.unwrap();
    let status = task.await.unwrap();
    handle.abort();
    status
}

/// The owners of a team, by role, as the first owner sees them.
async fn roles_of(app: &Arc<App>, token: &str) -> Vec<String> {
    let (_, team) = as_(app, token, "GET", "/api/team", None).await;
    let mut roles: Vec<String> = team["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap().to_string())
        .collect();
    roles.sort();
    roles
}

/// The check that an owner remains and the delete are one step across instances, too: while
/// another instance changes the owners (it holds the database-wide lock), a role change, a
/// delete and a batch with a delete each wait for it, and then go through.
#[tokio::test]
async fn changes_to_the_owners_wait_for_another_instance() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, first, _dir) = ready_app(&tdb).await;
    let second = member(&app, &first, "second-owner@example.com", "owner").await;
    let (_, other) = as_(&app, &second, "GET", "/api/team/me", None).await;
    let second_id = other["id"].as_str().unwrap().to_string();
    let third = member(&app, &first, "third@example.com", "viewer").await;
    let (_, other) = as_(&app, &third, "GET", "/api/team/me", None).await;
    let third_id = other["id"].as_str().unwrap().to_string();
    let fourth = member(&app, &first, "fourth@example.com", "viewer").await;
    let (_, other) = as_(&app, &fourth, "GET", "/api/team/me", None).await;
    let fourth_id = other["id"].as_str().unwrap().to_string();

    let (status, res) = as_(
        &app,
        &first,
        "PATCH",
        "/api/settings",
        Some(json!({ "batch": { "enabled": true } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(
        roles_of(&app, &first).await,
        ["owner", "owner", "viewer", "viewer"]
    );

    // a role change
    let status = waits_for_the_owners_lock(&tdb, {
        let (app, first, id) = (app.clone(), first.clone(), second_id.clone());
        async move {
            as_(
                &app,
                &first,
                "PATCH",
                &format!("/api/team/members/{id}"),
                Some(json!({ "role": "admin" })),
            )
            .await
            .0
        }
    })
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        roles_of(&app, &first).await,
        ["admin", "owner", "viewer", "viewer"]
    );

    // a delete
    let status = waits_for_the_owners_lock(&tdb, {
        let (app, first, id) = (app.clone(), first.clone(), third_id.clone());
        async move {
            as_(
                &app,
                &first,
                "DELETE",
                &format!("/api/collections/_superusers/records/{id}"),
                None,
            )
            .await
            .0
        }
    })
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(superuser_count(&app, &first).await, 3);

    // a batch with a delete
    let status = waits_for_the_owners_lock(&tdb, {
        let (app, first, id) = (app.clone(), first.clone(), fourth_id.clone());
        async move {
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
        }
    })
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(superuser_count(&app, &first).await, 2);
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
    two_instances_delete_each_other(&tdb, &first_app, &first, &second_app).await;
    tdb.drop_db().await;
}

/// The same with the smallest pools (a connection or two each, which is supported): the lock
/// is on the transaction that deletes, so it needs no connection of its own.
#[tokio::test]
async fn owners_deleting_each_other_through_small_pools_leave_one() {
    for (first_pool, second_pool) in [(1, 2), (2, 1), (1, 1)] {
        let Some(tdb) = TestDb::create().await else {
            return;
        };
        let (first_app, first, _dir) = ready_app_with_file(
            &tdb,
            &format!("[database]\nmax_connections = {first_pool}\n"),
        )
        .await;
        let (_dir2, config2) = temp_config();
        let second_app = new_app_configured(
            &config2,
            Some(tdb.url.clone()),
            slonbase::Extensions::default(),
            |o| o.max_connections = Some(second_pool),
        );
        second_app.bootstrap().await.unwrap();
        two_instances_delete_each_other(&tdb, &first_app, &first, &second_app).await;
        tdb.drop_db().await;
    }
}

/// Two owners, one on each instance, delete each other at the same moment.
async fn two_instances_delete_each_other(
    tdb: &TestDb,
    first_app: &Arc<App>,
    first: &str,
    second_app: &Arc<App>,
) {
    let (first_app, first) = (first_app.clone(), first.to_string());
    let second_app = second_app.clone();
    let second = member(&first_app, &first, "second-owner@example.com", "owner").await;
    // (a viewer, who remains whatever happens: the owners are what must not run out)
    member(&first_app, &first, "viewer@example.com", "viewer").await;
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
    };
    // a delete takes a second: both deletes would have passed the check for another owner
    // before either is done (without the database-wide lock, both would go through)
    admin_exec(
        &tdb.url,
        "CREATE FUNCTION slonbase.slow_delete() RETURNS trigger LANGUAGE plpgsql AS
         $$ BEGIN PERFORM pg_sleep(1); RETURN OLD; END $$;
         CREATE TRIGGER slow_delete BEFORE DELETE ON slonbase._superusers
         FOR EACH ROW EXECUTE FUNCTION slonbase.slow_delete();",
    )
    .await;
    let one = tokio::spawn(delete(first_app.clone(), first.clone(), second_id));
    let other = tokio::spawn(delete(second_app.clone(), second.clone(), first_id));
    let ((a, a_body), (b, b_body)) = (one.await.unwrap(), other.await.unwrap());
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
    let owners = roles_of(&first_app, survivor)
        .await
        .iter()
        .filter(|r| *r == "owner")
        .count();
    assert_eq!(owners, 1, "{a} and {b}");
    let (refused, body) = if a == StatusCode::NO_CONTENT {
        (b, b_body)
    } else {
        (a, a_body)
    };
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("last owner"),
        "the owner check refused it: {refused} {body}"
    );
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

/// A pool with a connection or two (supported) has no connection to spare for the
/// database-wide lock: changes to the owners still go through, under this instance's lock.
#[tokio::test]
async fn owners_change_with_the_smallest_pools() {
    for size in [1, 2] {
        let Some(tdb) = TestDb::create().await else {
            return;
        };
        let (app, first, _dir) =
            ready_app_with_file(&tdb, &format!("[database]\nmax_connections = {size}\n")).await;
        let second = member(&app, &first, &format!("second{size}@example.com"), "owner").await;
        let (_, other) = as_(&app, &second, "GET", "/api/team/me", None).await;
        let second_id = other["id"].as_str().unwrap().to_string();
        let (status, res) = as_(
            &app,
            &first,
            "PATCH",
            &format!("/api/team/members/{second_id}"),
            Some(json!({ "role": "admin" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "pool of {size}: {res}");
        let (status, res) = as_(
            &app,
            &first,
            "DELETE",
            &format!("/api/collections/_superusers/records/{second_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "pool of {size}: {res}");
        // a batch with a delete
        let (status, res) = as_(
            &app,
            &first,
            "PATCH",
            "/api/settings",
            Some(json!({ "batch": { "enabled": true } })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "pool of {size}: {res}");
        let third = member(&app, &first, &format!("third{size}@example.com"), "viewer").await;
        let (_, other) = as_(&app, &third, "GET", "/api/team/me", None).await;
        let third_id = other["id"].as_str().unwrap().to_string();
        let (status, res) = as_(
            &app,
            &first,
            "POST",
            "/api/batch",
            Some(json!({ "requests": [{
                "method": "DELETE",
                "url": format!("/api/collections/_superusers/records/{third_id}"),
            }] })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "pool of {size}: {res}");
        tdb.drop_db().await;
    }
}

/// A viewer sees the settings without the details of the servers and addresses behind them;
/// everyone else sees them as before.
#[tokio::test]
async fn a_viewer_does_not_see_the_infrastructure_in_the_settings() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, owner, _dir) = ready_app(&tdb).await;
    let viewer = member(&app, &owner, "viewer-settings@example.com", "viewer").await;
    let (status, res) = as_(
        &app,
        &owner,
        "PATCH",
        "/api/settings",
        Some(json!({
            "smtp": { "host": "smtp.internal.example", "username": "mailer", "port": 587 },
            "trustedProxy": { "headers": ["X-Forwarded-For"], "peers": ["10.0.0.0/8"] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    let (status, seen) = as_(&app, &owner, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen["smtp"]["host"], "smtp.internal.example");
    assert_eq!(seen["smtp"]["username"], "mailer");
    assert_eq!(seen["trustedProxy"]["peers"], json!(["10.0.0.0/8"]));

    let (status, seen) = as_(&app, &viewer, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(seen["smtp"]["host"], "");
    assert_eq!(seen["smtp"]["username"], "");
    assert_eq!(seen["smtp"]["port"], 587, "what isn't a detail stays");
    assert_eq!(seen["trustedProxy"]["peers"], json!([]));
    assert_eq!(seen["trustedProxy"]["headers"], json!(["X-Forwarded-For"]));
    assert_eq!(seen["smtp"]["password"], "", "secrets are never returned");
    tdb.drop_db().await;
}

/// A batch that deletes a superuser waits for the owners lock without its own timeout running:
/// the wait is the lock's (up to 30 seconds), not the batch's (a second here).
#[tokio::test]
async fn a_batch_waits_for_the_owners_lock_outside_its_own_timeout() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, first, _dir) = ready_app(&tdb).await;
    let viewer = member(&app, &first, "viewer-batch@example.com", "viewer").await;
    let (_, other) = as_(&app, &viewer, "GET", "/api/team/me", None).await;
    let viewer_id = other["id"].as_str().unwrap().to_string();
    let (status, res) = as_(
        &app,
        &first,
        "PATCH",
        "/api/settings",
        Some(json!({ "batch": { "enabled": true, "timeout": 1 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    let (other_instance, handle) = another_instance_holds_the_owners_lock(&tdb).await;
    let task = tokio::spawn({
        let (app, first) = (app.clone(), first.clone());
        async move {
            as_(
                &app,
                &first,
                "POST",
                "/api/batch",
                Some(json!({ "requests": [{
                    "method": "DELETE",
                    "url": format!("/api/collections/_superusers/records/{viewer_id}"),
                }] })),
            )
            .await
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    assert!(!task.is_finished(), "it went on without the lock");
    other_instance.batch_execute("COMMIT").await.unwrap();
    let (status, res) = task.await.unwrap();
    handle.abort();
    assert_eq!(
        status,
        StatusCode::OK,
        "the batch's timeout didn't run: {res}"
    );
    tdb.drop_db().await;
}

/// Role changes made at the same moment all show in the roles this instance goes by.
#[tokio::test]
async fn role_changes_made_at_once_all_take_effect() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, first, _dir) = ready_app(&tdb).await;
    let mut ids = Vec::new();
    for n in 0..3 {
        let token = member(&app, &first, &format!("owner{n}@example.com"), "owner").await;
        let (_, me) = as_(&app, &token, "GET", "/api/team/me", None).await;
        ids.push(me["id"].as_str().unwrap().to_string());
    }
    let demote = |id: String| {
        let (app, first) = (app.clone(), first.clone());
        async move {
            as_(
                &app,
                &first,
                "PATCH",
                &format!("/api/team/members/{id}"),
                Some(json!({ "role": "admin" })),
            )
            .await
            .0
        }
    };
    let statuses = tokio::join!(
        demote(ids[0].clone()),
        demote(ids[1].clone()),
        demote(ids[2].clone())
    );
    assert_eq!(
        [statuses.0, statuses.1, statuses.2],
        [StatusCode::OK; 3],
        "all three changes go through"
    );
    assert_eq!(
        roles_of(&app, &first).await,
        ["admin", "admin", "admin", "owner"]
    );
    tdb.drop_db().await;
}
