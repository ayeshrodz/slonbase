//! `POST /api/collections/meta/dry-run-view`: a view query's fields and a sample of its rows,
//! with nothing saved. Needs PostgreSQL.

mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;

#[tokio::test]
async fn dry_run_view() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let auth = format!("Bearer {token}");
    let call = |method: &'static str, uri: &'static str, body: Option<Value>| {
        let app = app.clone();
        let auth = auth.clone();
        async move { request(&app, method, uri, &[("authorization", auth.as_str())], body).await }
    };
    let (status, _) = call(
        "POST",
        "/api/collections",
        Some(json!({ "name": "posts", "fields": [{ "name": "title", "type": "text" }, { "name": "views", "type": "number" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for (title, views) in [("a", 1), ("b", 2), ("c", 3)] {
        call(
            "POST",
            "/api/collections/posts/records",
            Some(json!({ "title": title, "views": views })),
        )
        .await;
    }

    let (status, res) = call(
        "POST",
        "/api/collections/meta/dry-run-view",
        Some(
            json!({ "query": "SELECT id, title, views * 2 AS doubled FROM posts ORDER BY views" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let fields: Vec<(&str, &str)> = res["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["name"].as_str().unwrap(), f["type"].as_str().unwrap()))
        .collect();
    assert_eq!(
        fields,
        [("id", "text"), ("title", "text"), ("doubled", "number")]
    );
    let sample = res["sample"].as_array().unwrap();
    assert_eq!(sample.len(), 3);
    assert_eq!(sample[2]["title"], "c");
    assert_eq!(sample[2]["doubled"], 6.0);
    assert!(
        sample[0]["collectionName"]
            .as_str()
            .unwrap()
            .starts_with("temp_view_")
    );

    // at most 10 rows, and nothing saved
    let (_, res) = call(
        "POST",
        "/api/collections/meta/dry-run-view",
        Some(json!({ "query": "SELECT g::text AS id FROM generate_series(1, 50) g" })),
    )
    .await;
    assert_eq!(res["sample"].as_array().unwrap().len(), 10);
    let (_, list) = call("GET", "/api/collections?perPage=200", None).await;
    assert!(
        !list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"].as_str().unwrap().starts_with("temp_view"))
    );

    // refused: no id, duplicate ids, writes, other statements; and only superusers
    for (query, expect) in [
        ("SELECT title FROM posts", "id column"),
        ("SELECT 'same' AS id FROM posts", "non-unique ids"),
        ("SELECT '' AS id", "empty or invalid ids"),
        ("DELETE FROM posts", "SELECT"),
        ("SELECT id FROM posts; DROP TABLE posts", "single statement"),
    ] {
        let (status, res) = call(
            "POST",
            "/api/collections/meta/dry-run-view",
            Some(json!({ "query": query })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {res}");
        assert!(res.to_string().contains(expect), "{query}: {res}");
    }
    // read-only, even through a function that writes
    let (status, res) = call(
        "POST",
        "/api/collections/meta/dry-run-view",
        Some(json!({ "query": "SELECT id FROM posts WHERE pg_catalog.nextval('pg_catalog.pg_class_oid_seq') > 0" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    let (_, posts) = call("GET", "/api/collections/posts/records", None).await;
    assert_eq!(posts["totalItems"], 3);
    let (status, _) = request(
        &app,
        "POST",
        "/api/collections/meta/dry-run-view",
        &[],
        Some(json!({ "query": "SELECT id FROM posts" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dry_run_view_needs_the_sql_console() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = format!("Bearer {token}");
    let (status, res) = request(
        &app,
        "POST",
        "/api/collections/meta/dry-run-view",
        &[("authorization", auth.as_str())],
        Some(json!({ "query": "SELECT 'x' AS id" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");
}

/// Whatever a previewed query leaves on its connection (an advisory lock here, which a
/// rollback doesn't release) goes away with it: the connection isn't reused.
#[tokio::test]
async fn dry_run_view_leaves_no_session_state() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let auth = format!("Bearer {token}");
    for query in [
        "SELECT 'x' AS id, pg_advisory_lock(4242) AS locked",
        // and when the query fails after taking it
        "SELECT 'x' AS id, pg_advisory_lock(4243)::text::int AS locked",
    ] {
        request(
            &app,
            "POST",
            "/api/collections/meta/dry-run-view",
            &[("authorization", auth.as_str())],
            Some(json!({ "query": query })),
        )
        .await;
    }
    let client = tdb.pool().get().await.unwrap();
    let mut held = -1;
    for _ in 0..50 {
        held = client
            .query_one(
                "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND objid IN (4242, 4243) \
                 AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
                &[],
            )
            .await
            .unwrap()
            .get::<_, i64>(0);
        if held == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(held, 0, "advisory locks outlived the preview");
    drop(client);
    tdb.drop_db().await;
}
