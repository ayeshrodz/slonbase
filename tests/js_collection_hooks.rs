//! Collection hooks that change other collections: the change they make is nested in the one
//! that runs them, instead of waiting for it. See `setup_flow.rs` for how to point the tests at
//! a database.
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
// each "orders..." collection gets a log collection of its own
onCollectionCreate((e) => {
  if (e.collection.name.startsWith("orders") && !e.collection.name.endsWith("_log")) {
    $app.save(new Collection({
      name: e.collection.name + "_log",
      fields: [{ name: "message", type: "text" }],
    }));
  }
  e.next();
});

// a change to "orders" is noted on the "audit" collection's schema
onCollectionUpdate((e) => {
  if (e.collection.name === "orders") {
    const audit = $app.findCollectionByNameOrId("audit");
    audit.fields.push({ name: "seen_" + e.collection.fields.length, type: "text" });
    $app.save(audit);
  }
  // a hook that changes the collection being saved, under it
  if (e.collection.name === "self_edit" && !e.collection.fields.some((f) => f.name === "from_hook")) {
    const stored = $app.findCollectionByNameOrId("self_edit");
    stored.fields.push({ name: "from_hook", type: "text" });
    $app.save(stored);
  }
  e.next();
});

// the log goes with its collection
onCollectionDelete((e) => {
  if (e.collection.name.startsWith("orders") && !e.collection.name.endsWith("_log")) {
    $app.delete($app.findCollectionByNameOrId(e.collection.name + "_log"));
  }
  e.next();
});

// after the change is saved, in its execute hook
onCollectionCreateExecute((e) => {
  e.next();
  if (e.collection.name === "after_next") {
    $app.save(new Collection({ name: "after_next_companion" }));
  }
});

// before it is saved: a collection of its own is fine, one with the same name isn't
onCollectionCreateExecute((e) => {
  if (e.collection.name === "before_next") {
    $app.save(new Collection({ name: "before_next_companion" }));
  }
  if (e.collection.name === "clash" && e.collection.fields.some((f) => f.name === "outer")) {
    $app.save(new Collection({ name: "clash" }));
  }
  e.next();
});

// a nested change that fails and is caught changes nothing
onCollectionCreate((e) => {
  if (e.collection.name === "caught") {
    try {
      $app.save(new Collection({ name: "" }));
    } catch (err) {}
  }
  e.next();
});

// a collection the new one relates to is deleted just before it is saved
onCollectionCreateExecute((e) => {
  if (e.collection.name === "related") $app.delete($app.findCollectionByNameOrId("target"));
  e.next();
});

// the collection an import deletes is changed by its own delete hook
onCollectionDelete((e) => {
  if (e.collection.name === "doomed" && !e.collection.fields.some((f) => f.name === "flag")) {
    const doomed = $app.findCollectionByNameOrId("doomed");
    doomed.fields.push({ name: "flag", type: "bool" });
    $app.save(doomed);
  }
  e.next();
});

// a field a view reads is hidden while the view is being created
onCollectionValidate((e) => {
  if (e.collection.name === "src_view") {
    const src = $app.findCollectionByNameOrId("src");
    const note = src.fields.find((f) => f.name === "note");
    if (!note.hidden) {
      note.hidden = true;
      $app.save(src);
    }
  }
  e.next();
});

// a restore can't run inside a collection change
onCollectionCreate((e) => {
  if (e.collection.name === "restores") $app.restoreBackup("before_hooks.zip");
  e.next();
});
"#;

/// A request that must not hang.
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

async fn names(app: &Arc<App>, token: &str) -> Vec<String> {
    let (_, list) = call(app, token, "GET", "/api/collections?perPage=500", None).await;
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_hooks_can_change_other_collections() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
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
    // (views need the SQL console)
    let (app, token, _dir) = ready_app_configured(&tdb, ext, |o| o.sql_console = Some(true)).await;
    // a real backup, so a restore from a hook gets as far as the lock
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/backups",
        Some(json!({ "name": "before_hooks.zip" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");

    // an import whose delete hook changes the collection it deletes fails
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "doomed" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, res) = call(
        &app,
        &token,
        "PUT",
        "/api/collections/import",
        Some(json!({ "collections": [{ "name": "survivor", "type": "base", "fields": [] }], "deleteMissing": true })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(
        res["message"].as_str().unwrap().contains("A hook changed"),
        "{res}"
    );
    let all = names(&app, &token).await;
    assert!(
        all.contains(&"doomed".into()) && !all.contains(&"survivor".into()),
        "{all:?}"
    );

    // create: the hook creates the log collection inside the change
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "audit", "fields": [{ "name": "note", "type": "text" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "orders", "fields": [{ "name": "total", "type": "number" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let all = names(&app, &token).await;
    assert!(
        all.contains(&"orders".into()) && all.contains(&"orders_log".into()),
        "{all:?}"
    );

    // update: the hook changes another collection
    let (_, orders) = call(&app, &token, "GET", "/api/collections/orders", None).await;
    let mut fields = orders["fields"].as_array().unwrap().clone();
    fields.push(json!({ "name": "paid", "type": "bool" }));
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/collections/orders",
        Some(json!({ "fields": fields })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (_, audit) = call(&app, &token, "GET", "/api/collections/audit", None).await;
    assert!(
        audit["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"].as_str().unwrap().starts_with("seen_")),
        "{audit}"
    );
    let (_, orders) = call(&app, &token, "GET", "/api/collections/orders", None).await;
    assert!(
        orders["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "paid"),
        "{orders}"
    );

    // import: the create hook runs for the imported collection, and its own change nests too
    let (status, res) = call(
        &app,
        &token,
        "PUT",
        "/api/collections/import",
        Some(json!({
            "collections": [{ "name": "orders_eu", "type": "base", "fields": [] }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let all = names(&app, &token).await;
    assert!(
        all.contains(&"orders_eu".into()) && all.contains(&"orders_eu_log".into()),
        "{all:?}"
    );

    // delete: the hook deletes the log collection
    let (status, res) = call(&app, &token, "DELETE", "/api/collections/orders_eu", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let all = names(&app, &token).await;
    assert!(
        !all.contains(&"orders_eu".into()) && !all.contains(&"orders_eu_log".into()),
        "{all:?}"
    );

    // a change made after the collection is saved is kept with it, in memory too
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "after_next" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, res) = call(
        &app,
        &token,
        "GET",
        "/api/collections/after_next_companion",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // a hook that changes the collection being saved, or collections just before it is saved,
    // makes the change fail instead of saving over what the hook did
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "self_edit" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/collections/self_edit",
        Some(json!({ "listRule": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(
        res["message"].as_str().unwrap().contains("A hook changed"),
        "{res}"
    );
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "before_next" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let all = names(&app, &token).await;
    assert!(
        all.contains(&"before_next".into()) && all.contains(&"before_next_companion".into()),
        "{all:?}"
    );
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "clash", "fields": [{ "name": "outer", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(
        res["message"].as_str().unwrap().contains("A hook changed"),
        "{res}"
    );
    let (_, clash) = call(&app, &token, "GET", "/api/collections/clash", None).await;
    assert!(
        !clash["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "outer"),
        "the hook's collection stays: {clash}"
    );
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "caught" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // a restore from a collection hook is refused, not left waiting
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "restores" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(
        res["message"]
            .as_str()
            .unwrap()
            .contains("can't be restored from a collection hook"),
        "{res}"
    );
    assert!(!names(&app, &token).await.contains(&"restores".into()));

    // a relation to a collection a hook deleted just before the save is refused
    let (status, target) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "target" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{target}");
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "related", "fields": [
            { "name": "to", "type": "relation", "collectionId": target["id"], "maxSelect": 1 },
        ] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(
        res["message"].as_str().unwrap().contains("A hook changed"),
        "{res}"
    );
    assert!(!names(&app, &token).await.contains(&"related".into()));

    // a view takes the visibility its source fields have when it is saved, not before a hook
    // changed them
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "src", "fields": [{ "name": "note", "type": "text" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, view) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "src_view", "type": "view", "viewQuery": "SELECT id, note FROM src",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let note = view["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "note")
        .cloned();
    assert_eq!(
        note.as_ref().map(|f| f["hidden"].clone()),
        Some(json!(true)),
        "{view}"
    );

    // and other changes still take turns with them
    let mut tasks = Vec::new();
    for n in 0..4 {
        let (app, token) = (app.clone(), token.clone());
        tasks.push(tokio::spawn(async move {
            call(
                &app,
                &token,
                "POST",
                "/api/collections",
                Some(json!({ "name": format!("orders_{n}") })),
            )
            .await
        }));
    }
    for task in tasks {
        let (status, res) = task.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    let all = names(&app, &token).await;
    for n in 0..4 {
        assert!(all.contains(&format!("orders_{n}_log")), "{all:?}");
    }
}
