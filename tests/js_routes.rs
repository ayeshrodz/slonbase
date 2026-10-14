//! JavaScript middleware (`routerUse`) and file uploads in JavaScript routes. See
//! `setup_flow.rs` for how to point the tests at a database.
#![cfg(feature = "js")]

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;
use slonbase::config::HooksConfig;

const HOOKS: &str = r#"
// runs for every request, the built-in API included
routerUse((e) => {
  if (e.request.header.get("x-block") === "yes") {
    e.json(403, { message: "blocked by middleware" });
    return;
  }
  if (e.request.url.path === "/api/js/peek") {
    // the body stays readable for the route after the middleware read it
    e.setHeader("X-Peeked", String(e.requestInfo().body.n));
  }
  e.setHeader("X-Middleware", e.auth ? "auth" : "guest");
  e.next();
});

routerAdd("POST", "/api/js/peek", (e) => e.json(200, { n: e.requestInfo().body.n }));

// stores the uploaded files in a new record
routerAdd("POST", "/api/js/upload", (e) => {
  const files = e.findUploadedFiles("document");
  const record = new Record($app.findCollectionByNameOrId("docs"));
  record.set("title", e.requestInfo().body.title);
  record.set("files", files);
  $app.save(record);
  e.json(200, {
    id: record.id,
    names: files.map((f) => f.originalName),
    sizes: files.map((f) => f.size),
    files: record.get("files"),
  });
});

// appends a generated file to an existing record, keeping the stored ones
routerAdd("POST", "/api/js/append/{id}", (e) => {
  const record = $app.findRecordById("docs", e.request.pathValue("id"));
  const note = $filesystem.fileFromBytes("hello from js", "note.txt");
  record.set("files", [...record.get("files"), note]);
  $app.save(record);
  e.json(200, { files: record.get("files") });
});
"#;

async fn app_with_hooks(tdb: &TestDb) -> (Arc<App>, String, tempfile::TempDir, tempfile::TempDir) {
    let hooks_dir = tempfile::tempdir().unwrap();
    std::fs::write(hooks_dir.path().join("main.pb.js"), HOOKS).unwrap();
    let mut ext = slonbase::Extensions::default();
    slonbase::js::load(
        &HooksConfig {
            dir: hooks_dir.path().to_path_buf(),
            pool_size: 2,
        },
        &mut ext,
    )
    .await
    .unwrap();
    let (app, token, dir) = ready_app_with(tdb, ext).await;
    (app, token, dir, hooks_dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn middleware_and_uploads() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir, _hooks) = app_with_hooks(&tdb).await;
    let bearer = format!("Bearer {token}");
    let admin = [("authorization", bearer.as_str())];

    let (status, res) = request(&app, "POST", "/api/collections", &admin, Some(json!({
        "name": "docs", "listRule": "", "viewRule": "", "createRule": "", "updateRule": "", "deleteRule": "",
        "fields": [
            { "name": "title", "type": "text" },
            { "name": "files", "type": "file", "maxSelect": 5, "maxSize": 1048576 },
        ],
    }))).await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // the middleware wraps the built-in API: it adds headers and can answer instead
    let (status, headers, _) = raw_request(&app, "GET", "/api/health", &[], Vec::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-middleware"], "guest");
    let (status, headers, _) = raw_request(
        &app,
        "GET",
        "/api/collections/docs/records",
        &admin,
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-middleware"], "auth");
    let (status, res) = request(
        &app,
        "GET",
        "/api/collections/docs/records",
        &[("x-block", "yes")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(res["message"], "blocked by middleware");

    // a JSON body read by the middleware still reaches the route
    let (status, headers, body) = raw_request(
        &app,
        "POST",
        "/api/js/peek",
        &[("content-type", "application/json")],
        br#"{"n": 7}"#.to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-peeked"], "7");
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({ "n": 7 })
    );

    // multipart uploads in a route, saved to a file field
    let (content_type, body) = multipart(
        &[("title", "report")],
        &[
            ("document", "a.txt", b"first file"),
            ("document", "b.txt", b"second"),
        ],
    );
    let (status, _, res) = raw_request(
        &app,
        "POST",
        "/api/js/upload",
        &[("content-type", content_type.as_str())],
        body,
    )
    .await;
    let res: Value = serde_json::from_slice(&res).unwrap();
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["names"], json!(["a.txt", "b.txt"]));
    assert_eq!(res["sizes"], json!([10, 6]));
    let stored: Vec<String> = res["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_string())
        .collect();
    assert_eq!(stored.len(), 2, "{res}");
    assert!(
        stored[0].starts_with('a') && stored[0].ends_with(".txt"),
        "{stored:?}"
    );
    let id = res["id"].as_str().unwrap().to_string();
    let (status, _, content) = raw_request(
        &app,
        "GET",
        &format!("/api/files/docs/{id}/{}", stored[1]),
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content, b"second");

    // a file made in JavaScript is appended after the kept ones
    let (status, res) = request(&app, "POST", &format!("/api/js/append/{id}"), &[], None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let files = res["files"].as_array().unwrap();
    assert_eq!(files.len(), 3, "{res}");
    assert_eq!(files[0], stored[0].as_str());
    let note = files[2].as_str().unwrap();
    assert!(note.starts_with("note_"), "{note}");
    let (_, _, content) = raw_request(
        &app,
        "GET",
        &format!("/api/files/docs/{id}/{note}"),
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(content, b"hello from js");

    drop(app);
    tdb.drop_db().await;
}
