//! JavaScript migrations (`pb_migrations`). See `setup_flow.rs` for how to point the tests
//! at a database.
#![cfg(feature = "js")]

mod common;

use std::path::Path;
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;
use slonbase::config::HooksConfig;
use slonbase::js::migrate;

// as PocketBase's automigrate writes them
const CREATED: &str = r#"/// <reference path="../pb_data/types.d.ts" />
migrate((app) => {
  const collection = new Collection({
    "createRule": null,
    "deleteRule": null,
    "fields": [
      { "autogeneratePattern": "[a-z0-9]{15}", "hidden": false, "id": "text3208210256", "max": 15, "min": 15,
        "name": "id", "pattern": "^[a-z0-9]+$", "presentable": false, "primaryKey": true, "required": true,
        "system": true, "type": "text" },
      { "autogeneratePattern": "", "hidden": false, "id": "text724990059", "max": 0, "min": 0, "name": "title",
        "pattern": "", "presentable": false, "primaryKey": false, "required": true, "system": false, "type": "text" },
      { "hidden": false, "id": "number1", "max": null, "min": null, "name": "views", "onlyInt": false,
        "presentable": false, "required": false, "system": false, "type": "number" },
      { "hidden": false, "id": "bool1", "name": "published", "presentable": false, "required": false,
        "system": false, "type": "bool" }
    ],
    "id": "pbc_1125843985",
    "indexes": [],
    "listRule": "",
    "name": "posts",
    "system": false,
    "type": "base",
    "updateRule": null,
    "viewRule": null
  });

  return app.save(collection);
}, (app) => {
  const collection = app.findCollectionByNameOrId("pbc_1125843985");

  return app.delete(collection);
})
"#;

const UPDATED: &str = r#"migrate((app) => {
  const collection = app.findCollectionByNameOrId("pbc_1125843985")

  // update collection data
  unmarshal({ "viewRule": "" }, collection)

  // add field
  collection.fields.addAt(2, new Field({
    "autogeneratePattern": "", "hidden": false, "id": "text_summary", "max": 200, "min": 0, "name": "summary",
    "pattern": "", "presentable": false, "primaryKey": false, "required": false, "system": false, "type": "text"
  }))

  // remove field
  collection.fields.removeById("number1")

  return app.save(collection)
}, (app) => {
  const collection = app.findCollectionByNameOrId("pbc_1125843985")
  unmarshal({ "viewRule": null }, collection)
  collection.fields.removeById("text_summary")
  collection.fields.add(new NumberField({ "id": "number1", "name": "views" }))
  return app.save(collection)
})
"#;

const SEED: &str = r#"migrate((app) => {
  const superuser = new Record(app.findCollectionByNameOrId("_superusers"));
  superuser.set("email", "migrated@example.com");
  superuser.set("password", "migrated-pass-1");
  app.save(superuser);

  const posts = app.findCollectionByNameOrId("posts");
  for (const title of ["one", "two"]) {
    const r = new Record(posts);
    r.set("title", title);
    r.set("published", true);
    app.save(r);
  }
  // a failed call that's caught leaves the transaction usable
  try {
    app.db().newQuery("SELECT * FROM no_such_table").execute();
  } catch (_) {}

  // reads see the migration's own writes
  const n = app.countRecords("posts", "published = true");
  if (n !== 2) throw new Error("expected 2 posts, got " + n);
  app.db().newQuery("UPDATE slonbase.posts SET summary = 'from sql' WHERE title = {:t}").bind({ t: "two" }).execute();
  if (app.findFirstRecordByData("posts", "title", "two").get("summary") !== "from sql") throw new Error("no summary");
  // the query builder, with unqualified names, in the migration's transaction
  const found = app.recordQuery("posts").where($dbx.hashExp({ summary: "from sql" })).all();
  if (found.length !== 1 || found[0].get("title") !== "two") throw new Error("recordQuery: " + found.length);
  app.db().update("posts", { summary: "" }, $dbx.hashExp({ title: "one" })).execute();

  const settings = app.settings();
  settings.meta.appName = "Migrated";
  app.save(settings);
}, (app) => {
  for (const r of app.findRecordsByFilter("posts", "", "", 0, 0)) app.delete(r);
  app.delete(app.findAuthRecordByEmail("_superusers", "migrated@example.com"));
})
"#;

const VIEW: &str = r#"migrate((app) => {
  app.save(new Collection({
    name: "published_posts",
    type: "view",
    listRule: "",
    viewQuery: "SELECT id, title FROM posts WHERE published = true",
  }));
}, (app) => {
  app.delete(app.findCollectionByNameOrId("published_posts"));
})
"#;

fn write(dir: &Path, name: &str, source: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(name), source).unwrap();
}

async fn history(app: &Arc<App>) -> Vec<String> {
    migrate::applied(&app.core().unwrap()).await.unwrap()
}

fn field_names(collection: &Value) -> Vec<String> {
    collection["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect()
}

async fn sign_in(app: &Arc<App>, email: &str, password: &str) -> StatusCode {
    request(
        app,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        &[],
        Some(json!({ "identity": email, "password": password })),
    )
    .await
    .0
}

#[tokio::test]
async fn applies_pocketbase_migrations() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let migrations = tempfile::tempdir().unwrap();
    let dir = migrations.path().to_path_buf();
    write(&dir, "1700000001_created_posts.js", CREATED);
    write(&dir, "1700000002_updated_posts.js", UPDATED);
    write(&dir, "1700000003_seed.js", SEED);
    write(&dir, "1700000004_view.js", VIEW);
    write(&dir, "types.d.ts", "// not a migration");

    let configured = dir.clone();
    let (app, token, _config) = ready_app_configured(&tdb, Default::default(), move |o| {
        o.migrations_dir = Some(configured);
        o.sql_console = Some(false);
    })
    .await;
    let admin = [("authorization", token.as_str())];

    assert_eq!(
        history(&app).await,
        [
            "1700000001_created_posts.js",
            "1700000002_updated_posts.js",
            "1700000003_seed.js",
            "1700000004_view.js"
        ]
    );
    let (status, posts) = request(&app, "GET", "/api/collections/posts", &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{posts}");
    assert_eq!(posts["id"], "pbc_1125843985");
    assert_eq!(posts["viewRule"], "");
    let names = field_names(&posts);
    assert_eq!(&names[..3], ["id", "title", "summary"], "{names:?}");
    assert!(!names.contains(&"views".to_string()), "{names:?}");

    let (_, list) = request(
        &app,
        "GET",
        "/api/collections/posts/records?sort=title",
        &[],
        None,
    )
    .await;
    assert_eq!(list["totalItems"], 2, "{list}");
    assert_eq!(list["items"][1]["summary"], "from sql");
    // view collections are created without the SQL console: migrations are trusted code
    let (_, view) = request(
        &app,
        "GET",
        "/api/collections/published_posts/records",
        &[],
        None,
    )
    .await;
    assert_eq!(view["totalItems"], 2, "{view}");
    assert_eq!(
        sign_in(&app, "migrated@example.com", "migrated-pass-1").await,
        StatusCode::OK
    );
    let (_, settings) = request(&app, "GET", "/api/settings", &admin, None).await;
    assert_eq!(settings["meta"]["appName"], "Migrated");

    // another server on the database applies nothing again
    let (_config2, path) = temp_config();
    let configured = dir.clone();
    let second = new_app_configured(&path, Some(tdb.url.clone()), Default::default(), move |o| {
        o.migrations_dir = Some(configured)
    });
    second.bootstrap().await.unwrap();
    let (_, list) = request(&second, "GET", "/api/collections/posts/records", &[], None).await;
    assert_eq!(list["totalItems"], 2, "{list}");

    // down, newest first
    let core = app.core().unwrap();
    let hooks = tempfile::tempdir().unwrap();
    let reverted = migrate::down(&core, &dir, hooks.path(), 2).await.unwrap();
    assert_eq!(reverted, ["1700000004_view.js", "1700000003_seed.js"]);
    assert!(core.registry().get("published_posts").is_none());
    let (_, list) = request(&app, "GET", "/api/collections/posts/records", &[], None).await;
    assert_eq!(list["totalItems"], 0, "{list}");
    assert_eq!(
        sign_in(&app, "migrated@example.com", "migrated-pass-1").await,
        StatusCode::BAD_REQUEST
    );
    migrate::down(&core, &dir, hooks.path(), 1).await.unwrap();
    let names = field_names(
        &request(&app, "GET", "/api/collections/posts", &admin, None)
            .await
            .1,
    );
    assert!(names.contains(&"views".to_string()) && !names.contains(&"summary".to_string()));
    assert_eq!(history(&app).await, ["1700000001_created_posts.js"]);

    // up again applies the reverted ones
    let applied = migrate::up(&core, &dir, hooks.path()).await.unwrap();
    assert_eq!(applied.len(), 3, "{applied:?}");
    tdb.drop_db().await;
}

#[tokio::test]
async fn a_failed_migration_changes_nothing() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, _token, _config) = ready_app(&tdb).await;
    let core = app.core().unwrap();
    let migrations = tempfile::tempdir().unwrap();
    let dir = migrations.path();
    let hooks = tempfile::tempdir().unwrap();
    write(
        dir,
        "1_first.js",
        "migrate((app) => { app.save(new Collection({ name: \"first\" })); })",
    );
    write(
        dir,
        "2_broken.js",
        r#"migrate((app) => {
  app.save(new Collection({ name: "notes", fields: [{ name: "text", type: "text" }] }));
  const r = new Record(app.findCollectionByNameOrId("notes"));
  r.set("text", "hello");
  app.save(r);
  app.db().newQuery("CREATE TABLE public.migration_leftover (id int)").execute();
  throw new Error("something went wrong");
})"#,
    );
    write(
        dir,
        "3_later.js",
        "migrate((app) => { app.save(new Collection({ name: \"later\" })); })",
    );

    let err = migrate::up(&core, dir, hooks.path()).await.unwrap_err();
    let message = format!("{err:#}");
    assert!(
        message.contains("2_broken.js") && message.contains("something went wrong"),
        "{message}"
    );
    // the first one stays applied, the broken one left nothing behind, the later one waits
    assert_eq!(migrate::applied(&core).await.unwrap(), ["1_first.js"]);
    assert!(core.registry().get("first").is_some());
    assert!(core.registry().get("notes").is_none());
    let client = tdb.pool().get().await.unwrap();
    let leftovers: bool = client
        .query_one(
            "SELECT to_regclass('public.migration_leftover') IS NOT NULL OR to_regclass('slonbase.notes') IS NOT NULL",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!leftovers);

    // validation errors name the field
    write(
        dir,
        "2_broken.js",
        "migrate((app) => { app.save(new Collection({ name: \"bad name!\" })); })",
    );
    let message = format!(
        "{:#}",
        migrate::up(&core, dir, hooks.path()).await.unwrap_err()
    );
    assert!(message.contains("name"), "{message}");

    // fixed, the rest applies
    write(
        dir,
        "2_broken.js",
        "migrate((app) => { app.save(new Collection({ name: \"notes\" })); })",
    );
    let applied = migrate::up(&core, dir, hooks.path()).await.unwrap();
    assert_eq!(applied, ["2_broken.js", "3_later.js"]);

    // history-sync forgets deleted files; down refuses files it can't find
    std::fs::remove_file(dir.join("1_first.js")).unwrap();
    assert_eq!(
        migrate::sync_history(&core, dir).await.unwrap(),
        ["1_first.js"]
    );
    tdb.drop_db().await;
}

#[tokio::test]
async fn automigrate_writes_replayable_files() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let migrations = tempfile::tempdir().unwrap();
    let dir = migrations.path().to_path_buf();
    let configured = dir.clone();
    let (app, token, _config) = ready_app_configured(&tdb, Default::default(), move |o| {
        o.migrations_dir = Some(configured);
        o.automigrate = Some(true);
    })
    .await;
    let admin = [("authorization", token.as_str())];

    let (status, created) = request(
        &app,
        "POST",
        "/api/collections",
        &admin,
        Some(json!({ "name": "tasks", "listRule": "", "fields": [{ "name": "title", "type": "text", "required": true }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let mut fields = created["fields"].as_array().unwrap().clone();
    fields.push(json!({ "name": "done", "type": "bool" }));
    let (status, body) = request(
        &app,
        "PATCH",
        "/api/collections/tasks",
        &admin,
        Some(json!({ "fields": fields, "viewRule": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = request(
        &app,
        "POST",
        "/api/collections",
        &admin,
        Some(json!({ "name": "scratch" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(&app, "DELETE", "/api/collections/scratch", &admin, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let files: Vec<String> = migrate::files(&dir)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let kinds: Vec<&str> = files
        .iter()
        .map(|f| f.split_once('_').unwrap().1.trim_end_matches(".js"))
        .collect();
    assert_eq!(
        kinds,
        [
            "created_tasks",
            "updated_tasks",
            "created_scratch",
            "deleted_scratch"
        ],
        "{files:?}"
    );
    // they describe changes that already happened
    assert_eq!(history(&app).await, files);

    // replayed on another database, they give the same collection
    let Some(other) = TestDb::create().await else {
        return;
    };
    let (replica, replica_token, _config2) = ready_app(&other).await;
    let core = replica.core().unwrap();
    let hooks = tempfile::tempdir().unwrap();
    let applied = migrate::up(&core, &dir, hooks.path()).await.unwrap();
    assert_eq!(applied.len(), 4);
    let (_, original) = request(&app, "GET", "/api/collections/tasks", &admin, None).await;
    let (_, copy) = request(
        &replica,
        "GET",
        "/api/collections/tasks",
        &[("authorization", replica_token.as_str())],
        None,
    )
    .await;
    for key in ["id", "fields", "listRule", "viewRule", "indexes"] {
        assert_eq!(original[key], copy[key], "{key}");
    }
    assert!(core.registry().get("scratch").is_none());

    // and reverted, the update first
    migrate::down(&core, &dir, hooks.path(), 3).await.unwrap();
    let fields = field_names(&core_collection(&replica, "tasks"));
    assert!(!fields.contains(&"done".to_string()), "{fields:?}");
    tdb.drop_db().await;
    other.drop_db().await;
}

fn core_collection(app: &Arc<App>, name: &str) -> Value {
    slonbase::collections::api::to_json(&app.core().unwrap().collection(name).unwrap())
}

#[tokio::test]
async fn hooks_save_collections_and_settings() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let hooks = tempfile::tempdir().unwrap();
    std::fs::write(
        hooks.path().join("main.pb.js"),
        r#"
routerAdd("POST", "/api/js/collections", (e) => {
  const c = new Collection({ name: "made_by_hook", fields: [new TextField({ name: "label" })] });
  $app.save(c);
  c.fields.add(new BoolField({ name: "flag" }));
  c.listRule = "";
  $app.save(c);
  const settings = $app.settings();
  settings.meta.appName = "Hooked";
  $app.save(settings);
  const all = $app.findAllCollections("base").map((x) => x.name);
  e.json(200, { id: c.id, fields: c.fields.fieldNames(), isBase: c.isBase(), all });
});
routerAdd("DELETE", "/api/js/collections", (e) => {
  $app.delete($app.findCollectionByNameOrId("made_by_hook"));
  e.noContent(204);
});
"#,
    )
    .unwrap();
    let mut ext = slonbase::Extensions::default();
    slonbase::js::load(
        &HooksConfig {
            dir: hooks.path().to_path_buf(),
            pool_size: 1,
            watch: false,
        },
        &mut ext,
    )
    .await
    .unwrap();
    let (app, token, _config) = ready_app_with(&tdb, ext).await;
    let admin = [("authorization", token.as_str())];

    let (status, body) = request(&app, "POST", "/api/js/collections", &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["id"].as_str().unwrap().starts_with("pbc_"), "{body}");
    assert_eq!(body["fields"], json!(["id", "label", "flag"]));
    assert_eq!(body["isBase"], true);
    assert!(
        body["all"]
            .as_array()
            .unwrap()
            .contains(&json!("made_by_hook"))
    );
    let (_, c) = request(&app, "GET", "/api/collections/made_by_hook", &admin, None).await;
    assert_eq!(c["listRule"], "");
    let (_, settings) = request(&app, "GET", "/api/settings", &admin, None).await;
    assert_eq!(settings["meta"]["appName"], "Hooked");

    let (status, _) = request(&app, "DELETE", "/api/js/collections", &admin, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(&app, "GET", "/api/collections/made_by_hook", &admin, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    tdb.drop_db().await;
}

// as in PocketBase's snapshots (`pocketbase migrate collections`) and exports
const SNAPSHOT: &str = r#"migrate((app) => {
  const snapshot = [
    { "id": "pbc_2279338944", "name": "_mfas", "system": true, "type": "base",
      "fields": [{ "id": "text455797646", "name": "collectionRef", "type": "text", "system": true }] },
    { "id": "pbc_3142635823", "name": "_superusers", "system": true, "type": "auth",
      "fields": [{ "id": "email3885137012", "name": "email", "type": "email", "system": true }],
      "authToken": { "duration": 86400 } },
    { "id": "pbc_items0000001", "name": "items", "system": false, "type": "base", "listRule": "",
      "fields": [{ "id": "number1", "name": "n", "type": "number" }] },
    { "id": "pbc_itemsview001", "name": "items_view", "system": false, "type": "view", "listRule": "",
      "viewQuery": "SELECT id, n FROM items WHERE n > 1" }
  ];

  return app.importCollections(snapshot, false);
}, (app) => {
  return null;
})
"#;

#[tokio::test]
async fn imports_pocketbase_snapshots() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _config) = ready_app(&tdb).await;
    let admin = [("authorization", token.as_str())];
    let core = app.core().unwrap();
    let superusers_before = field_names(&core_collection(&app, "_superusers"));
    let migrations = tempfile::tempdir().unwrap();
    let hooks = tempfile::tempdir().unwrap();
    write(migrations.path(), "1_collections_snapshot.js", SNAPSHOT);
    migrate::up(&core, migrations.path(), hooks.path())
        .await
        .unwrap();

    // the view reads the table created by the same import
    let view = core_collection(&app, "items_view");
    assert_eq!(field_names(&view), ["id", "n"]);
    // system collections keep their fields and take the settings
    let superusers = core_collection(&app, "_superusers");
    assert_eq!(field_names(&superusers), superusers_before);
    assert_eq!(superusers["authToken"]["duration"], 86400);
    assert!(core.registry().get("_mfas").is_none());

    // the API's import takes them too
    let (status, body) = request(
        &app,
        "PUT",
        "/api/collections/import",
        &admin,
        Some(json!({ "collections": [
            { "name": "_mfas", "system": true, "fields": [] },
            { "name": "_superusers", "system": true, "type": "auth", "fields": [], "authToken": { "duration": 3600 } },
            { "name": "more", "fields": [{ "name": "x", "type": "text" }] }
        ] })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        core_collection(&app, "_superusers")["authToken"]["duration"],
        3600
    );
    assert!(core.registry().get("more").is_some());
    tdb.drop_db().await;
}
