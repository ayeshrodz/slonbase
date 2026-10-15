//! JavaScript validation, collection and mail hooks. See `setup_flow.rs` for how to point
//! the tests at a database.
#![cfg(feature = "js")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use common::*;
use slonbase::config::HooksConfig;
use slonbase::mail::MemoryMailer;

const HOOKS: &str = r#"
onRecordValidate((e) => {
  if (e.record.getInt("qty") < 0) {
    throw new BadRequestError("Invalid quantity.", { qty: { code: "negative", message: "Must not be negative." } });
  }
  e.next();
}, "items");

onCollectionCreate((e) => {
  if (e.collection.name.startsWith("tmp_")) throw new BadRequestError("No temporary collections.");
  // public by default
  if (e.collection.listRule === null) e.collection.listRule = "";
  e.next();
});

onCollectionUpdate((e) => {
  if (e.collection.name === "renamed_by_hook") throw new BadRequestError("Reserved name.");
  e.next();
});

onCollectionDelete((e) => {
  if (e.collection.name === "keep") throw new BadRequestError("This one stays.");
  e.next();
});

onCollectionAfterCreateSuccess((e) => {
  if (e.collection.name === "events") return e.next();
  const event = new Record($app.findCollectionByNameOrId("events"));
  event.set("message", "created " + e.collection.name);
  $app.save(event);
  e.next();
});

onRecordDelete((e) => {
  if (e.record.getBool("pinned")) throw new BadRequestError("A pinned child can't go.");
  e.next();
}, "children");

onMailerSend((e) => {
  e.message.subject = "[app] " + e.message.subject;
  e.next();
});

onMailerRecordPasswordResetSend((e) => {
  // no reset emails for this domain
  if (e.record.email().endsWith("@blocked.test")) return;
  e.message.html += "<p>kind: " + e.kind + "</p>";
  e.next();
});
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validate_collection_and_mail_hooks() {
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
        },
        &mut ext,
    )
    .await
    .unwrap();
    let (app, token, _dir) = ready_app_with(&tdb, ext).await;
    let mailer = Arc::new(MemoryMailer::default());
    app.core().unwrap().set_mailer(mailer.clone());
    let bearer = format!("Bearer {token}");
    let admin = [("authorization", bearer.as_str())];

    // collection hooks: the change a handler makes is applied
    let (status, events) = request(
        &app,
        "POST",
        "/api/collections",
        &admin,
        Some(json!({
            "name": "events", "listRule": null, "viewRule": "",
            "fields": [{ "name": "message", "type": "text" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events}");
    assert_eq!(events["listRule"], "", "{events}");
    let (status, res) = request(
        &app,
        "POST",
        "/api/collections",
        &admin,
        Some(json!({
            "name": "tmp_x", "fields": [],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["message"], "No temporary collections.");
    let (status, items) = request(
        &app,
        "POST",
        "/api/collections",
        &admin,
        Some(json!({
            "name": "items", "listRule": "", "createRule": "", "updateRule": "",
            "fields": [{ "name": "qty", "type": "number" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{items}");
    let (status, res) = request(
        &app,
        "PATCH",
        "/api/collections/items",
        &admin,
        Some(json!({
            "name": "renamed_by_hook",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["message"], "Reserved name.");
    let (status, _) = request(
        &app,
        "POST",
        "/api/collections",
        &admin,
        Some(json!({
            "name": "keep", "fields": [],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, res) = request(&app, "DELETE", "/api/collections/keep", &admin, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["message"], "This one stays.");
    let (status, _) = request(&app, "GET", "/api/collections/keep", &admin, None).await;
    assert_eq!(status, StatusCode::OK, "still there");
    // after-success hooks run in the background
    let mut messages = Vec::new();
    for _ in 0..100 {
        let (_, list) = request(
            &app,
            "GET",
            "/api/collections/events/records?sort=message",
            &[],
            None,
        )
        .await;
        messages = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["message"].as_str().unwrap().to_string())
            .collect();
        if messages.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(messages, ["created items", "created keep"]);

    // validation hooks run after the built-in validation, for every write
    let (status, res) = request(
        &app,
        "POST",
        "/api/collections/items/records",
        &[],
        Some(json!({ "qty": -1 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["data"]["qty"]["code"], "negative", "{res}");
    let (status, item) = request(
        &app,
        "POST",
        "/api/collections/items/records",
        &[],
        Some(json!({ "qty": 2 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{item}");
    let id = item["id"].as_str().unwrap();
    let (status, _) = request(
        &app,
        "PATCH",
        &format!("/api/collections/items/records/{id}"),
        &[],
        Some(json!({ "qty": -5 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let core = app.core().unwrap();
    let saved = core
        .save_record(
            "items",
            [("qty".to_string(), json!(-3))].into_iter().collect(),
        )
        .await;
    assert!(saved.is_err(), "the record service validates too");

    // mail hooks: every email, and per kind with the record
    let (status, res) = request(
        &app,
        "POST",
        "/api/collections",
        &admin,
        Some(json!({
            "name": "users", "type": "auth", "createRule": "",
            "fields": [],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    for email in ["ann@example.com", "bob@blocked.test"] {
        let (status, res) = request(
            &app,
            "POST",
            "/api/collections/users/records",
            &[],
            Some(json!({
                "email": email, "password": "12345678", "passwordConfirm": "12345678",
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{res}");
        let (status, _) = request(
            &app,
            "POST",
            "/api/collections/users/request-password-reset",
            &[],
            Some(json!({ "email": email })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let mut sent = Vec::new();
    for _ in 0..100 {
        sent = mailer.sent.lock().unwrap().clone();
        if !sent.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    // give a skipped email time to show up if it were sent
    tokio::time::sleep(Duration::from_millis(300)).await;
    let sent = {
        let all = mailer.sent.lock().unwrap().clone();
        assert!(all.len() >= sent.len());
        all
    };
    assert_eq!(
        sent.len(),
        1,
        "{:?}",
        sent.iter().map(|m| &m.to).collect::<Vec<_>>()
    );
    assert_eq!(sent[0].to, "ann@example.com");
    assert!(sent[0].subject.starts_with("[app] "), "{}", sent[0].subject);
    assert!(
        sent[0].html.contains("kind: passwordReset"),
        "{}",
        sent[0].html
    );

    // collection imports run the collection hooks too
    let (status, res) = request(
        &app,
        "PUT",
        "/api/collections/import",
        &admin,
        Some(json!({
            "collections": [{ "name": "tmp_imported", "type": "base", "fields": [] }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["message"], "No temporary collections.");
    let (status, res) = request(
        &app,
        "PUT",
        "/api/collections/import",
        &admin,
        Some(json!({
            "collections": [{ "name": "imported", "type": "base", "listRule": null, "fields": [] }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (_, imported) = request(&app, "GET", "/api/collections/imported", &admin, None).await;
    assert_eq!(
        imported["listRule"], "",
        "the create hook changed the imported collection"
    );
    let mut seen = false;
    for _ in 0..100 {
        let (_, list) = request(
            &app,
            "GET",
            "/api/collections/events/records?filter=message%3D%22created%20imported%22",
            &[],
            None,
        )
        .await;
        if list["totalItems"] == 1 {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(seen, "the after-create hook ran for the import");

    // cascaded deletes run the delete hook of the cascaded records
    let (status, parents) = request(
        &app,
        "POST",
        "/api/collections",
        &admin,
        Some(json!({
            "name": "parents", "listRule": "", "viewRule": "", "createRule": "", "deleteRule": "",
            "fields": [{ "name": "name", "type": "text" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{parents}");
    let (status, res) = request(&app, "POST", "/api/collections", &admin, Some(json!({
        "name": "children", "listRule": "", "viewRule": "", "createRule": "", "deleteRule": "",
        "fields": [
            { "name": "pinned", "type": "bool" },
            { "name": "parent", "type": "relation", "collectionId": parents["id"], "maxSelect": 1, "cascadeDelete": true },
        ],
    }))).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let create = |collection: &'static str, body: serde_json::Value| {
        let app = app.clone();
        async move {
            let (status, res) = request(
                &app,
                "POST",
                &format!("/api/collections/{collection}/records"),
                &[],
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{res}");
            res["id"].as_str().unwrap().to_string()
        }
    };
    let keep = create("parents", json!({ "name": "keep" })).await;
    create("children", json!({ "parent": keep, "pinned": true })).await;
    let (status, res) = request(
        &app,
        "DELETE",
        &format!("/api/collections/parents/records/{keep}"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["message"], "A pinned child can't go.");
    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/collections/parents/records/{keep}"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the parent stays");
    let gone = create("parents", json!({ "name": "gone" })).await;
    let child = create("children", json!({ "parent": gone, "pinned": false })).await;
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/collections/parents/records/{gone}"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(
        &app,
        "GET",
        &format!("/api/collections/children/records/{child}"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the child went with it");

    drop(core);
    drop(app);
    tdb.drop_db().await;
}
