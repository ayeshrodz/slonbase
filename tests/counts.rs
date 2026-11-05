//! List totals served from the count cache stay exact: after writes through the API, writes by
//! other programs (the realtime triggers' notifications), truncates through the SQL console,
//! and for rules that depend on the caller or on the time. Needs PostgreSQL.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

async fn call(
    app: &Arc<App>,
    token: Option<&str>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = token.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.iter().map(|a| ("authorization", a.as_str())).collect();
    request(app, method, uri, &headers, body).await
}

async fn total(app: &Arc<App>, token: Option<&str>, query: &str) -> i64 {
    let (status, res) = call(
        app,
        token,
        "GET",
        &format!("/api/collections/notes/records?perPage=1{query}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    res["totalItems"].as_i64().unwrap()
}

/// Polls until the total is `want` (a write by another program arrives by notification).
async fn eventually(app: &Arc<App>, token: Option<&str>, query: &str, want: i64) {
    for _ in 0..100 {
        if total(app, token, query).await == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(total(app, token, query).await, want);
}

#[tokio::test]
async fn cached_totals_stay_exact() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app_configured(&tdb, Default::default(), |o| {
        o.sql_console = Some(true);
        o.sql_console_writes = Some(true);
    })
    .await;
    let admin = Some(admin.as_str());

    let (_, users_collection) = call(
        &app,
        admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "users", "type": "auth", "createRule": "", "listRule": "", "viewRule": "",
        })),
    )
    .await;
    let (status, res) = call(
        &app,
        admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "notes",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "owner", "type": "relation", "collectionId": users_collection["id"], "maxSelect": 1 },
                { "name": "due", "type": "date" }
            ],
            "listRule": "owner = '' || owner = @request.auth.id",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let mut users = Vec::new();
    for i in 0..2 {
        let email = format!("u{i}@example.com");
        let (status, user) = call(
            &app,
            None,
            "POST",
            "/api/collections/users/records",
            Some(json!({
                "email": email, "password": "password123", "passwordConfirm": "password123",
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{user}");
        let (_, login) = call(
            &app,
            None,
            "POST",
            "/api/collections/users/auth-with-password",
            Some(json!({
                "identity": email, "password": "password123",
            })),
        )
        .await;
        users.push((
            user["id"].as_str().unwrap().to_string(),
            login["token"].as_str().unwrap().to_string(),
        ));
    }
    let (u0, t0) = (&users[0].0, Some(users[0].1.as_str()));
    let t1 = Some(users[1].1.as_str());

    // through the API: exact at once, again and again
    assert_eq!(total(&app, None, "").await, 0);
    for i in 1..=3 {
        call(
            &app,
            admin,
            "POST",
            "/api/collections/notes/records",
            Some(json!({ "title": format!("n{i}") })),
        )
        .await;
        assert_eq!(total(&app, None, "").await, i);
        assert_eq!(total(&app, None, "").await, i, "served from the cache");
    }

    // per caller: the rule's parameters are part of the key
    call(
        &app,
        admin,
        "POST",
        "/api/collections/notes/records",
        Some(json!({ "title": "mine", "owner": u0 })),
    )
    .await;
    assert_eq!(total(&app, None, "").await, 3);
    assert_eq!(total(&app, t0, "").await, 4);
    assert_eq!(total(&app, t1, "").await, 3);
    assert_eq!(total(&app, t0, "").await, 4);

    // written by another program: the trigger's notification makes it stale
    let client = tdb.pool().get().await.unwrap();
    client
        .execute(
            "INSERT INTO slonbase.notes (id, title) VALUES ('external0000001', 'x')",
            &[],
        )
        .await
        .unwrap();
    eventually(&app, None, "", 4).await;
    client
        .execute(
            "DELETE FROM slonbase.notes WHERE id = 'external0000001'",
            &[],
        )
        .await
        .unwrap();
    eventually(&app, None, "", 3).await;

    // a TRUNCATE through the SQL console, which no trigger sees
    let (status, res) = call(
        &app,
        admin,
        "POST",
        "/api/sql",
        Some(json!({ "query": "TRUNCATE slonbase.notes" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(total(&app, None, "").await, 0);
    assert_eq!(total(&app, t0, "").await, 0);

    // time-dependent filters are never cached (they change without writes)
    let soon = (chrono::Utc::now() + chrono::Duration::seconds(2))
        .format("%Y-%m-%d %H:%M:%S%.3fZ")
        .to_string();
    call(
        &app,
        admin,
        "POST",
        "/api/collections/notes/records",
        Some(json!({ "title": "due", "due": soon })),
    )
    .await;
    let filter = "&filter=due%20%3E%20%40now";
    assert_eq!(total(&app, None, filter).await, 1);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        total(&app, None, filter).await,
        0,
        "no write, but the time moved on"
    );

    drop(client);
    drop(app);
    tdb.drop_db().await;
}
