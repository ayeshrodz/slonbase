//! Whole-path tests for collection changes made while hooks run: a deleted collection's files go
//! with it even when a hook creates a collection with the same id afterwards, and a request that
//! is cancelled while a hook is inside a nested change doesn't leave the schema locked. See
//! `setup_flow.rs` for how to point the tests at a database.
#![cfg(feature = "js")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;
use slonbase::config::HooksConfig;

const HOOKS: &str = r#"
// the collection "docs" is created again, with the same id, right after it is deleted
onCollectionDeleteExecute((e) => {
  e.next();
  if (e.collection.name === "docs") {
    $app.save(new Collection({
      id: e.collection.id, name: "docs",
      listRule: "", viewRule: "", createRule: "",
      fields: [{ name: "attachment", type: "file", maxSelect: 1 }],
    }));
  }
});

// a change to "slow" takes a while and changes "other" in the middle
onCollectionUpdate((e) => {
  if (e.collection.name === "slow") {
    sleep(800);
    const other = $app.findCollectionByNameOrId("other");
    other.fields.push({ name: "added_by_hook", type: "text" });
    $app.save(other);
  }
  e.next();
});
"#;

async fn app(tdb: &TestDb) -> (Arc<App>, String, tempfile::TempDir, tempfile::TempDir) {
    let hooks_dir = tempfile::tempdir().unwrap();
    std::fs::write(hooks_dir.path().join("main.pb.js"), HOOKS).unwrap();
    let mut ext = slonbase::Extensions::default();
    slonbase::js::load(
        &HooksConfig {
            dir: hooks_dir.path().to_path_buf(),
            pool_size: 2,
            watch: false,
            ..Default::default()
        },
        &mut ext,
    )
    .await
    .unwrap();
    let (app, token, dir) = ready_app_with(tdb, ext).await;
    (app, token, dir, hooks_dir)
}

async fn call(
    app: &Arc<App>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let bearer = format!("Bearer {token}");
    tokio::time::timeout(
        Duration::from_secs(30),
        request(
            app,
            method,
            uri,
            &[("authorization", bearer.as_str())],
            body,
        ),
    )
    .await
    .unwrap_or_else(|_| panic!("{method} {uri} didn't finish: a deadlock"))
}

/// A record of `docs` with a file; its id and the file's name.
async fn upload(app: &Arc<App>, token: &str) -> (String, String) {
    let (content_type, body) = multipart(&[], &[("attachment", "note.txt", b"hello")]);
    let bearer = format!("Bearer {token}");
    let (status, _, bytes) = raw_request(
        app,
        "POST",
        "/api/collections/docs/records",
        &[
            ("content-type", content_type.as_str()),
            ("authorization", bearer.as_str()),
        ],
        body,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let record: Value = serde_json::from_slice(&bytes).unwrap();
    (
        record["id"].as_str().unwrap().to_string(),
        record["attachment"].as_str().unwrap().to_string(),
    )
}

async fn file_status(app: &Arc<App>, id: &str, name: &str) -> StatusCode {
    let (status, _, _) = raw_request(
        app,
        "GET",
        &format!("/api/files/docs/{id}/{name}"),
        &[],
        Vec::new(),
    )
    .await;
    status
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deleted_collections_files_go_and_the_new_one_with_its_id_keeps_its_own() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir, _hooks) = app(&tdb).await;
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "id": "pbc_docsfiles", "name": "docs",
            "listRule": "", "viewRule": "", "createRule": "",
            "fields": [{ "name": "attachment", "type": "file", "maxSelect": 1 }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (old_id, old_file) = upload(&app, &token).await;
    assert_eq!(file_status(&app, &old_id, &old_file).await, StatusCode::OK);

    // deleted; the hook creates "docs" again with the same id once the delete is saved
    let (status, res) = call(&app, &token, "DELETE", "/api/collections/docs", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (status, res) = call(&app, &token, "GET", "/api/collections/docs", None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["id"], "pbc_docsfiles");
    assert_eq!(
        file_status(&app, &old_id, &old_file).await,
        StatusCode::NOT_FOUND,
        "the files of the deleted collection went with it"
    );

    // a file kept in the new collection is still there after the deleted one's clean-up is over
    let (new_id, new_file) = upload(&app, &token).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(file_status(&app, &new_id, &new_file).await, StatusCode::OK);
    tdb.drop_db().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_collection_change_cancelled_inside_a_hook_doesnt_leave_the_schema_locked() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir, _hooks) = app(&tdb).await;
    for name in ["slow", "other", "third"] {
        let (status, res) = call(
            &app,
            &token,
            "POST",
            "/api/collections",
            Some(json!({ "name": name, "fields": [{ "name": "a", "type": "text" }] })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    // the request is dropped while the hook is inside its nested change
    let slow = call(
        &app,
        &token,
        "PATCH",
        "/api/collections/slow",
        Some(json!({ "listRule": "" })),
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), slow)
            .await
            .is_err(),
        "the change was expected to still be running"
    );

    // another change goes through, after the one that was cancelled is done with the lock
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/collections/third",
        Some(json!({ "listRule": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (_, other) = call(&app, &token, "GET", "/api/collections/other", None).await;
    let added = other["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["name"] == "added_by_hook")
        .count();
    assert!(
        added <= 1,
        "the nested change is applied once or not at all: {other}"
    );
    tdb.drop_db().await;
}
