//! Importing collections that change an existing schema: columns change type or disappear, a
//! referenced collection is dropped, an index or a rule is invalid. Each import is applied whole
//! or not at all, and records stay readable. See `setup_flow.rs` for how to point the tests at
//! a database.

mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;

#[tokio::test]
async fn imports_change_schemas_whole_or_not_at_all() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = format!("Bearer {token}");
    let go = |method: &'static str, uri: &'static str, body: Option<Value>| {
        let (app, auth) = (app.clone(), auth.clone());
        async move { request(&app, method, uri, &[("authorization", auth.as_str())], body).await }
    };

    let (status, authors) = go(
        "POST",
        "/api/collections",
        Some(json!({ "name": "authors", "fields": [{ "name": "name", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{authors}");
    let (status, books) = go(
        "POST",
        "/api/collections",
        Some(json!({
            "name": "books",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "pages", "type": "text" },
                { "name": "author", "type": "relation", "collectionId": authors["id"] }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{books}");
    let (_, author) = go(
        "POST",
        "/api/collections/authors/records",
        Some(json!({ "name": "Ada" })),
    )
    .await;
    let (status, _) = go(
        "POST",
        "/api/collections/books/records",
        Some(json!({ "title": "Notes", "pages": "42", "author": author["id"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let list_ok = || {
        let go = &go;
        async move {
            let (status, res) = go("GET", "/api/collections/books/records", None).await;
            assert_eq!(status, StatusCode::OK, "books unreadable: {res}");
            res
        }
    };

    // 1. a text column becomes a number: existing text values convert or the import is refused,
    //    but never a 500, and the records stay readable
    let mut fields = books["fields"].clone();
    for f in fields.as_array_mut().unwrap() {
        if f["name"] == "pages" {
            f["type"] = json!("number");
        }
    }
    let (status, res) = go(
        "PUT",
        "/api/collections/import",
        Some(json!({ "collections": [{ "id": books["id"], "name": "books", "type": "base", "fields": fields }], "deleteMissing": false })),
    )
    .await;
    assert!(status.as_u16() < 500, "type change: {status} {res}");
    list_ok().await;

    // 2. a column disappears: its values go, the rest stays
    let kept: Vec<Value> = books["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["name"] != "pages")
        .cloned()
        .collect();
    let (status, res) = go(
        "PUT",
        "/api/collections/import",
        Some(json!({ "collections": [{ "id": books["id"], "name": "books", "type": "base", "fields": kept }], "deleteMissing": false })),
    )
    .await;
    assert!(status.as_u16() < 500, "dropping a column: {status} {res}");
    let after = list_ok().await;
    if status.is_success() {
        assert_eq!(after["items"][0]["title"], "Notes");
        assert!(after["items"][0].get("pages").is_none());
    }

    // 3. a good collection beside a bad one: nothing of the import is applied
    let (status, res) = go(
        "PUT",
        "/api/collections/import",
        Some(json!({ "collections": [
            { "name": "fresh", "type": "base", "fields": [{ "name": "x", "type": "text" }] },
            { "name": "bad rule", "type": "base", "listRule": "((" }
        ], "deleteMissing": false })),
    )
    .await;
    assert!(status.is_client_error(), "{status} {res}");
    let (status, _) = go("GET", "/api/collections/fresh", None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a failed import left a collection"
    );

    // 4. dropping a referenced collection with deleteMissing: refused, or the relation goes with
    //    it; either way no 500 and the other collection stays readable
    let (status, res) = go(
        "PUT",
        "/api/collections/import",
        Some(json!({ "collections": [{ "id": books["id"], "name": "books", "type": "base", "fields": [{ "name": "title", "type": "text" }] }], "deleteMissing": true })),
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "dropping a referenced collection: {status} {res}"
    );
    list_ok().await;

    tdb.drop_db().await;
}

/// `database.statement_timeout` ends a query that runs too long, such as a view that is slow to
/// read, instead of letting it hold a pooled connection.
#[tokio::test]
async fn a_slow_view_is_cut_off_by_the_statement_timeout() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (_first, token, _dir) = ready_app(&tdb).await;
    let (_dir2, config) = temp_config();
    std::fs::write(
        &config,
        "[server]\nsql_console = true\n[database]\nstatement_timeout = 1\n",
    )
    .unwrap();
    let app = new_app(&config, Some(tdb.url.clone()));
    app.bootstrap().await.unwrap();
    let auth = format!("Bearer {token}");
    let (status, view) = request(
        &app,
        "POST",
        "/api/collections",
        &[("authorization", auth.as_str())],
        Some(json!({
            "name": "slow", "type": "view",
            "viewQuery": "SELECT 'a' AS id, pg_sleep(20)::text AS c",
            "listRule": ""
        })),
    )
    .await;
    // creating a view only needs it to compile; reading it runs it
    assert!(status.is_success(), "{status} {view}");
    let started = std::time::Instant::now();
    let (status, res) = request(&app, "GET", "/api/collections/slow/records", &[], None).await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "the query ran for {:?} ({status}): {res}",
        started.elapsed()
    );
    assert!(!status.is_success(), "{res}");
    tdb.drop_db().await;
}
