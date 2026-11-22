//! A collection's files and a collection change that is cancelled while a hook still runs.
//! See `setup_flow.rs` for how to point the tests at a database.
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
// a collection created after `e.next()` with the id of the one just deleted is given a file
onCollectionDeleteExecute((e) => {
  e.next();
  if (e.collection.name === "reborn_src") {
    const reborn = new Collection({
      id: e.collection.id,
      name: "reborn",
      fields: [{ name: "doc", type: "file" }],
    });
    $app.save(reborn);
    const record = new Record(reborn);
    record.set("doc", $filesystem.fileFromBytes("kept after the delete", "kept.txt"));
    $app.save(record);
  }
});

// a change whose own hook creates a nested change; the nested change's hook is slow
onCollectionCreate((e) => {
  if (e.collection.name === "outer") {
    $app.save(new Collection({ name: "inner" }));
  }
  if (e.collection.name === "inner") {
    // says that the slow hook has started (a collection the test looks for)
    $app.save(new Collection({ name: "inner_started" }));
    const until = Date.now() + 6000;
    while (Date.now() < until) {}
  }
  e.next();
});
"#;

fn stored_files(dir: &tempfile::TempDir) -> Vec<String> {
    let root = dir.path().join("storage");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                // (with `/` on every platform)
                out.push(
                    path.strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    out.sort();
    out
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

async fn app_with_hooks(tdb: &TestDb) -> (Arc<App>, String, tempfile::TempDir) {
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
    ready_app_with(tdb, ext).await
}

/// The files of a deleted collection are removed as soon as the delete commits, inside
/// `e.next()`: a collection a handler creates afterwards with the same id keeps the files it is
/// given.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_collection_created_with_the_deleted_ones_id_keeps_its_files() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, dir) = app_with_hooks(&tdb).await;

    // a collection with a record that has a file
    let (status, created) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "reborn_src",
            "fields": [{ "name": "doc", "type": "file" }],
            "createRule": "",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let (content_type, body) = multipart(&[], &[("doc", "old.txt", b"removed with the delete")]);
    let bearer = format!("Bearer {token}");
    let (status, _, bytes) = raw_request(
        &app,
        "POST",
        "/api/collections/reborn_src/records",
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
    let before = stored_files(&dir);
    assert_eq!(before.len(), 1, "{before:?}");
    assert!(before[0].starts_with(&format!("{id}/")), "{before:?}");

    // the delete hook creates `reborn` with the same id and a file after `e.next()`
    let (status, res) = call(&app, &token, "DELETE", "/api/collections/reborn_src", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (status, reborn) = call(&app, &token, "GET", "/api/collections/reborn", None).await;
    assert_eq!(status, StatusCode::OK, "{reborn}");
    assert_eq!(reborn["id"], id.as_str());

    let after = stored_files(&dir);
    assert_eq!(after.len(), 1, "only the new file is left: {after:?}");
    assert!(after[0].starts_with(&format!("{id}/")), "{after:?}");
    assert!(
        after[0].ends_with("kept.txt") || after[0].contains("kept"),
        "{after:?}"
    );
    assert_ne!(after, before, "the old file went, the new one stayed");
    tdb.drop_db().await;
}

/// A request is cancelled while a hook is inside a change nested in it: the change still running
/// keeps the schema lock, so another collection change waits for it instead of running beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_collection_change_keeps_the_lock_while_its_nested_hook_runs() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = app_with_hooks(&tdb).await;
    let bearer = format!("Bearer {token}");

    // the request runs in a task that is aborted (dropped) once the nested change's slow hook
    // has started, which it says by saving a collection
    let request_app = app.clone();
    let request_bearer = bearer.clone();
    let request_task = tokio::spawn(async move {
        request(
            &request_app,
            "POST",
            "/api/collections",
            &[("authorization", request_bearer.as_str())],
            Some(json!({ "name": "outer" })),
        )
        .await
    });
    let mut started = false;
    for _ in 0..400 {
        let (_, list) = call(&app, &token, "GET", "/api/collections?perPage=500", None).await;
        if list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "inner_started")
        {
            started = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(started, "the nested change's hook should have started");
    request_task.abort();
    assert!(request_task.await.unwrap_err().is_cancelled());
    // another change has to wait for the one still running: it is started now and, a while
    // later (far less than the hook's six seconds, so a slow test thread cannot reach the end
    // of the hook first), it has still not finished
    let other_app = app.clone();
    let other_bearer = bearer.clone();
    let other = tokio::spawn(async move {
        request(
            &other_app,
            "POST",
            "/api/collections",
            &[("authorization", other_bearer.as_str())],
            Some(json!({ "name": "other" })),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(
        !other.is_finished(),
        "the second change ran beside the first instead of waiting for it"
    );
    let (status, res) = tokio::time::timeout(Duration::from_secs(30), other)
        .await
        .expect("the second change finishes once the hook has")
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{res}");

    // the nested change ended and was saved, whole; nothing is half done
    let (_, list) = call(&app, &token, "GET", "/api/collections?perPage=500", None).await;
    let names: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"inner") && names.contains(&"other"),
        "{names:?}"
    );

    // and later changes are not held up
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "after" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    tdb.drop_db().await;
}
