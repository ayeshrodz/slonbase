//! `record.set()` in JavaScript hooks takes PocketBase's modifiers (`tags+`, `+tags`,
//! `tags-`, `views+`, `views-`, `code:autogenerate`), and `$security.randomStringByRegex()`.
//! See `setup_flow.rs` for how to point the tests at a database.
#![cfg(feature = "js")]

mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::*;
use slonbase::config::HooksConfig;

const HOOKS: &str = r#"
routerAdd("POST", "/api/js/modifiers", (e) => {
  const people = $app.findCollectionByNameOrId("people");
  const ann = new Record(people);
  ann.set("name", "ann");
  $app.save(ann);
  const bob = new Record(people);
  bob.set("name", "bob");
  $app.save(bob);

  const r = new Record($app.findCollectionByNameOrId("items"));
  const steps = {};
  r.set("tags", ["a"]);
  r.set("tags+", "b");
  r.set("+tags", ["c", "a"]);
  steps.tagsBefore = r.get("tags");
  r.set("tags-", "a");
  steps.tags = r.get("tags");
  r.set("tags+", '["a","a",""]');
  steps.tagsJson = r.get("tags");

  r.set("kind+", "x");
  steps.kindOne = r.get("kind");
  r.set("kind+", ["y", "z"]);
  steps.kindLast = r.get("kind");
  r.set("kind-", "z");
  steps.kindNone = r.get("kind");

  r.set("owners+", ann.id);
  r.set("+owners", [bob.id, ann.id]);
  steps.owners = r.get("owners");

  r.set("views", 5);
  r.set("views+", "2.5");
  r.set("views-", true);
  r.set("views+", " 3");
  r.set("views+", "0x10");
  steps.views = r.get("views");

  r.set("code:autogenerate", "INV-");
  steps.code = r.get("code");
  r.set("note:autogenerate", 42);
  steps.note = r.get("note");

  r.set("nofield+", 1);
  steps.custom = r.get("nofield+");
  r.set("expand", { a: 1 });
  steps.expand = r.expand();
  steps.expandIsData = r.get("expand");

  // (the custom key goes to the save too, which ignores keys that aren't fields)
  $app.save(r);
  const saved = $app.findRecordById("items", r.id);
  e.json(200, {
    steps,
    saved: { tags: saved.get("tags"), kind: saved.get("kind"), owners: saved.get("owners"),
             views: saved.get("views"), code: saved.get("code") },
    regex: $security.randomStringByRegex("(ab|cd)-[0-9]{3}"),
    ids: [ann.id, bob.id],
  });
});

routerAdd("GET", "/api/js/bad-regex", (e) => {
  try {
    $security.randomStringByRegex("(abc");
    e.json(200, { thrown: false });
  } catch (err) {
    e.json(200, { thrown: true });
  }
});
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_set_modifiers() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let hooks_dir = tempfile::tempdir().unwrap();
    std::fs::write(hooks_dir.path().join("main.pb.js"), HOOKS).unwrap();
    let mut ext = slonbase::Extensions::default();
    slonbase::js::load(
        &HooksConfig {
            dir: hooks_dir.path().to_path_buf(),
            pool_size: 1,
            watch: false,
        },
        &mut ext,
    )
    .await
    .unwrap();
    let (app, token, _dir) = ready_app_with(&tdb, ext).await;
    let auth = format!("Bearer {token}");
    let admin = [("authorization", auth.as_str())];

    let (status, people) = request(
        &app,
        "POST",
        "/api/collections",
        &admin,
        Some(json!({ "name": "people", "fields": [{ "name": "name", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{people}");
    let (status, items) = request(
        &app,
        "POST",
        "/api/collections",
        &admin,
        Some(json!({
            "name": "items",
            "fields": [
                { "name": "tags", "type": "select", "values": ["a", "b", "c"], "maxSelect": 3 },
                { "name": "kind", "type": "select", "values": ["x", "y", "z"], "maxSelect": 1 },
                { "name": "owners", "type": "relation", "collectionId": people["id"], "maxSelect": 5 },
                { "name": "views", "type": "number" },
                { "name": "code", "type": "text", "autogeneratePattern": "[0-9]{4}" },
                { "name": "note", "type": "text" },
            ],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{items}");

    let (status, res) = request(&app, "POST", "/api/js/modifiers", &[], None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let steps = &res["steps"];
    let (ann, bob) = (res["ids"][0].clone(), res["ids"][1].clone());
    assert_eq!(steps["tagsBefore"], json!(["c", "a", "b"]), "{res}");
    assert_eq!(steps["tags"], json!(["c", "b"]));
    assert_eq!(steps["tagsJson"], json!(["c", "b", "a"]));
    // a single select keeps the last value
    assert_eq!(steps["kindOne"], "x");
    assert_eq!(steps["kindLast"], "z");
    assert_eq!(steps["kindNone"], "");
    assert_eq!(steps["owners"], json!([bob, ann]));
    // 5 + 2.5 - 1 + 0 (" 3" and "0x10" aren't Go floats)
    assert_eq!(steps["views"], json!(6.5));
    let code = steps["code"].as_str().unwrap();
    assert!(
        code.len() == 8
            && code.starts_with("INV-")
            && code[4..].bytes().all(|b| b.is_ascii_digit()),
        "{code}"
    );
    // no pattern: the value as text
    assert_eq!(steps["note"], "42");
    // keys no field takes are kept as they are; expand is the expand
    assert_eq!(steps["custom"], 1);
    assert_eq!(steps["expand"], json!({ "a": 1 }));
    // (and `get("expand")` reads it, as in PocketBase)
    assert_eq!(steps["expandIsData"], json!({ "a": 1 }));

    // what was saved is what the setters made
    let saved = &res["saved"];
    assert_eq!(saved["tags"], json!(["c", "b", "a"]), "{res}");
    assert_eq!(saved["kind"], "");
    assert_eq!(saved["owners"], json!([bob, ann]));
    assert_eq!(saved["views"], json!(6.5));
    assert_eq!(saved["code"], steps["code"]);

    let regex = res["regex"].as_str().unwrap();
    assert!(
        regex::Regex::new("^(ab|cd)-[0-9]{3}$")
            .unwrap()
            .is_match(regex),
        "{regex}"
    );
    let (status, res) = request(&app, "GET", "/api/js/bad-regex", &[], None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["thrown"], true);
}
