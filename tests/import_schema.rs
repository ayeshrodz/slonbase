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

/// The fields of `posts` as they are now.
async fn res_fields<F, Fut>(go: &F) -> Value
where
    F: Fn(&'static str, &'static str, Option<Value>) -> Fut,
    Fut: std::future::Future<Output = (StatusCode, Value)>,
{
    let (_, posts) = go("GET", "/api/collections/posts", None).await;
    posts["fields"].clone()
}

/// A column that a view reads is renamed, and the view's query is updated to the new name in
/// the same import: the view isn't rebuilt from the query it had.
#[tokio::test]
async fn an_import_renames_a_column_and_updates_the_view_that_reads_it() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let auth = format!("Bearer {token}");
    let go = |method: &'static str, uri: &'static str, body: Option<Value>| {
        let (app, auth) = (app.clone(), auth.clone());
        async move { request(&app, method, uri, &[("authorization", auth.as_str())], body).await }
    };
    let (status, posts) = go(
        "POST",
        "/api/collections",
        Some(json!({ "name": "posts", "fields": [{ "name": "legacy_title", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{posts}");
    let (status, titles) = go(
        "POST",
        "/api/collections",
        Some(json!({ "name": "post_titles", "type": "view",
                     "viewQuery": "SELECT id, legacy_title FROM posts" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{titles}");
    // a second view of the import that doesn't change: it is made again all the same
    let (status, ids) = go(
        "POST",
        "/api/collections",
        Some(json!({ "name": "post_ids", "type": "view", "viewQuery": "SELECT id FROM posts" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ids}");
    // and one that is not in the import: rebuilt from its own query, as before
    let (status, other) = go(
        "POST",
        "/api/collections",
        Some(json!({ "name": "post_other", "type": "view", "viewQuery": "SELECT id FROM posts" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{other}");
    let (status, _) = go(
        "POST",
        "/api/collections/posts/records",
        Some(json!({ "legacy_title": "First" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mut fields = posts["fields"].clone();
    for f in fields.as_array_mut().unwrap() {
        if f["name"] == "legacy_title" {
            f["name"] = json!("title");
        }
    }
    let (status, res) = go(
        "PUT",
        "/api/collections/import",
        Some(json!({ "collections": [
            { "id": posts["id"], "name": "posts", "type": "base", "fields": fields },
            { "id": titles["id"], "name": "post_titles", "type": "view",
              "viewQuery": "SELECT id, title FROM posts" },
            { "id": ids["id"], "name": "post_ids", "type": "view",
              "viewQuery": "SELECT id FROM posts" },
        ], "deleteMissing": false })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (status, list) = go("GET", "/api/collections/post_titles/records", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"][0]["title"], "First", "{list}");
    // a view added by an import that keeps another of its views unchanged and reads it, while a
    // table changes
    let mut fields = res_fields(&go).await;
    fields
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "note", "type": "text" }));
    let (status, res) = go(
        "PUT",
        "/api/collections/import",
        Some(json!({ "collections": [
            { "id": posts["id"], "name": "posts", "type": "base", "fields": fields },
            { "id": ids["id"], "name": "post_ids", "type": "view",
              "viewQuery": "SELECT id FROM posts" },
            { "name": "copied_ids", "type": "view", "viewQuery": "SELECT id FROM post_ids" },
        ], "deleteMissing": false })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (status, list) = go("GET", "/api/collections/copied_ids/records", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["totalItems"], 1, "{list}");
    // a column is renamed again, the view that reads it is updated, and a new view reads that
    // view: the updated one is made first
    let mut fields = res_fields(&go).await;
    for f in fields.as_array_mut().unwrap() {
        if f["name"] == "title" {
            f["name"] = json!("headline");
        }
    }
    let (status, res) = go(
        "PUT",
        "/api/collections/import",
        Some(json!({ "collections": [
            { "id": posts["id"], "name": "posts", "type": "base", "fields": fields },
            { "id": titles["id"], "name": "post_titles", "type": "view",
              "viewQuery": "SELECT id, headline FROM posts" },
            { "name": "copied_titles", "type": "view",
              "viewQuery": "SELECT id, headline FROM post_titles" },
        ], "deleteMissing": false })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (status, list) = go("GET", "/api/collections/copied_titles/records", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"][0]["headline"], "First", "{list}");
    // and a view whose query is wrong is still refused, with its own error
    let (status, res) = go(
        "PUT",
        "/api/collections/import",
        Some(json!({ "collections": [
            { "name": "broken", "type": "view", "viewQuery": "SELECT nothing FROM nowhere" },
        ], "deleteMissing": false })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"].to_string().contains("viewQuery"), "{res}");
    for (name, uri) in [
        ("post_ids", "/api/collections/post_ids/records"),
        ("post_other", "/api/collections/post_other/records"),
    ] {
        let (status, list) = go("GET", uri, None).await;
        assert_eq!(status, StatusCode::OK, "{name}: {list}");
        assert_eq!(list["totalItems"], 1, "{name}: {list}");
    }
    tdb.drop_db().await;
}

/// An index's condition names columns of the collection, and the flag of a system field isn't
/// the client's to clear.
#[tokio::test]
async fn indexes_name_known_columns_and_system_fields_stay_system() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = format!("Bearer {token}");
    let go = |method: &'static str, uri: &'static str, body: Option<Value>| {
        let (app, auth) = (app.clone(), auth.clone());
        async move { request(&app, method, uri, &[("authorization", auth.as_str())], body).await }
    };
    let make = |index: &str| {
        json!({ "name": "posts", "fields": [{ "name": "title", "type": "text" }],
                "indexes": [index] })
    };
    // an unknown column in the condition is the client's mistake, not the server's
    let (status, res) = go(
        "POST",
        "/api/collections",
        Some(make(
            "CREATE INDEX idx_posts_title ON posts (title) WHERE missing_col = ''",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"]["indexes"].is_object(), "{res}");
    // a known one works (and a name in other letters is the field's name)
    let (status, res) = go(
        "POST",
        "/api/collections",
        Some(make(
            "CREATE INDEX idx_posts_title ON posts (title) WHERE title != ''",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // a JSON column can be tested in the condition (it just can't be indexed)
    let (status, res) = go(
        "POST",
        "/api/collections",
        Some(json!({ "name": "notes",
                     "fields": [{ "name": "title", "type": "text" }, { "name": "meta", "type": "json" }],
                     "indexes": ["CREATE INDEX idx_notes_title ON notes (title) WHERE meta IS NOT NULL"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // a system field stays one when an update says it isn't
    let (status, system) = go("GET", "/api/collections/_externalAuths", None).await;
    assert_eq!(status, StatusCode::OK, "{system}");
    let mut fields = system["fields"].clone();
    let flagged: Vec<String> = fields
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["system"] == true)
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect();
    assert!(!flagged.is_empty(), "{system}");
    for f in fields.as_array_mut().unwrap() {
        f["system"] = json!(false);
    }
    let (status, res) = go(
        "PATCH",
        "/api/collections/_externalAuths",
        Some(json!({ "fields": fields })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (_, after) = go("GET", "/api/collections/_externalAuths", None).await;
    for name in &flagged {
        let field = after["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name.as_str())
            .unwrap_or_else(|| panic!("{name} is gone: {res} {after}"));
        assert_eq!(field["system"], true, "{name}: {res} {after}");
    }
    tdb.drop_db().await;
}
