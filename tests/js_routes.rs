//! JavaScript middleware (`routerUse`), file uploads in JavaScript routes, `$filesystem`,
//! model hooks and watching the hook files. See `setup_flow.rs` for how to point the tests
//! at a database.
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

// files from the server's disk or from a URL
routerAdd("POST", "/api/js/fetch/{id}", (e) => {
  const body = e.requestInfo().body;
  const record = $app.findRecordById("docs", e.request.pathValue("id"));
  const file = body.path ? $filesystem.fileFromPath(body.path) : $filesystem.fileFromURL(body.url);
  record.set("files", [file]);
  $app.save(record);
  e.json(200, { files: record.get("files"), name: file.originalName, size: file.size });
});

// model hooks: every write to notes, whatever its source
onRecordCreate((e) => {
  if (e.record.getString("title") === "nope") throw new BadRequestError("Not this title.");
  e.record.set("slug", e.record.getString("title").toLowerCase().replace(/ /g, "-"));
  e.next();
}, "notes");

onRecordUpdate((e) => {
  e.record.set("slug", e.record.getString("title").toLowerCase().replace(/ /g, "-"));
  e.next();
}, "notes");

onRecordDelete((e) => {
  if (e.record.getBool("locked")) throw new BadRequestError("The note is locked.");
  e.next();
}, "notes");

routerAdd("POST", "/api/js/note", (e) => {
  const note = new Record($app.findCollectionByNameOrId("notes"));
  note.set("title", e.requestInfo().body.title);
  $app.save(note);
  e.json(200, { slug: note.get("slug") });
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
            watch: false,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_hooks_and_file_sources() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir, _hooks) = app_with_hooks(&tdb).await;
    let bearer = format!("Bearer {token}");
    let admin = [("authorization", bearer.as_str())];
    for body in [
        json!({
            "name": "docs", "listRule": "", "viewRule": "", "createRule": "", "updateRule": "", "deleteRule": "",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "files", "type": "file", "maxSelect": 5, "maxSize": 1048576 },
            ],
        }),
        json!({
            "name": "notes", "listRule": "", "viewRule": "", "createRule": "", "updateRule": "", "deleteRule": "",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "slug", "type": "text" },
                { "name": "locked", "type": "bool" },
            ],
        }),
    ] {
        let (status, res) = request(&app, "POST", "/api/collections", &admin, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }

    // model hooks run for API writes...
    let (status, note) = request(
        &app,
        "POST",
        "/api/collections/notes/records",
        &[],
        Some(json!({ "title": "Hello World" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{note}");
    assert_eq!(note["slug"], "hello-world");
    let id = note["id"].as_str().unwrap().to_string();
    let (status, res) = request(
        &app,
        "POST",
        "/api/collections/notes/records",
        &[],
        Some(json!({ "title": "nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["message"], "Not this title.");
    let (status, note) = request(
        &app,
        "PATCH",
        &format!("/api/collections/notes/records/{id}"),
        &[],
        Some(json!({ "title": "Second Title", "locked": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{note}");
    assert_eq!(note["slug"], "second-title");
    let (status, res) = request(
        &app,
        "DELETE",
        &format!("/api/collections/notes/records/{id}"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["message"], "The note is locked.");

    // ...for $app.save in JavaScript, the Rust record service and batches
    let (status, res) = request(
        &app,
        "POST",
        "/api/js/note",
        &[],
        Some(json!({ "title": "From JS" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["slug"], "from-js");
    let core = app.core().unwrap();
    let saved = core
        .save_record(
            "notes",
            [("title".to_string(), json!("From Rust"))]
                .into_iter()
                .collect(),
        )
        .await
        .unwrap();
    assert_eq!(saved["slug"], "from-rust");
    let (status, res) = request(
        &app,
        "PATCH",
        "/api/settings",
        &admin,
        Some(json!({ "batch": { "enabled": true } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, res) = request(&app, "POST", "/api/batch", &[], Some(json!({ "requests": [
        { "method": "POST", "url": "/api/collections/notes/records", "body": { "title": "In Batch" } },
    ] }))).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res[0]["body"]["slug"], "in-batch");

    // $filesystem.fileFromPath and fileFromURL
    let (status, doc) = request(
        &app,
        "POST",
        "/api/collections/docs/records",
        &[],
        Some(json!({ "title": "doc" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    let doc_id = doc["id"].as_str().unwrap().to_string();
    let source = tempfile::tempdir().unwrap();
    let path = source.path().join("report.txt");
    std::fs::write(&path, b"from disk").unwrap();
    let (status, res) = request(
        &app,
        "POST",
        &format!("/api/js/fetch/{doc_id}"),
        &[],
        Some(json!({ "path": path.to_string_lossy() })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["name"], "report.txt");
    assert_eq!(res["size"], 9);
    let stored = res["files"][0].as_str().unwrap().to_string();
    let (_, _, content) = raw_request(
        &app,
        "GET",
        &format!("/api/files/docs/{doc_id}/{stored}"),
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(content, b"from disk");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let files = axum::Router::new().route(
            "/assets/logo.svg",
            axum::routing::get(|| async { "<svg></svg>" }),
        );
        axum::serve(listener, files).await.unwrap();
    });
    let (status, res) = request(
        &app,
        "POST",
        &format!("/api/js/fetch/{doc_id}"),
        &[],
        Some(json!({ "url": format!("http://{addr}/assets/logo.svg") })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["name"], "logo.svg");
    assert_eq!(res["size"], 11);
    let (status, res) = request(
        &app,
        "POST",
        &format!("/api/js/fetch/{doc_id}"),
        &[],
        Some(json!({ "url": format!("http://{addr}/missing.txt") })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["message"], "fileFromURL: the server answered 404.");

    drop(core);
    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn watching_the_hook_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.pb.js"), "// v1").unwrap();
    let watch = tokio::spawn(slonbase::js::changed(dir.path().to_path_buf()));
    // nothing changes: it keeps waiting
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert!(!watch.is_finished());
    std::fs::create_dir(dir.path().join("lib")).unwrap();
    std::fs::write(
        dir.path().join("lib").join("util.js"),
        "module.exports = {};",
    )
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), watch)
        .await
        .expect("a change is noticed")
        .unwrap();
}
