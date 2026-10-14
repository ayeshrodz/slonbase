//! JavaScript hooks (`*.pb.js`). See `setup_flow.rs` for how to point the tests at a
//! database.
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
onRecordCreateRequest((e) => {
  if (e.record.get("title") === "forbidden") {
    throw new BadRequestError("Not this title.", { title: { code: "forbidden", message: "No." } });
  }
  e.record.set("status", "draft");
  e.next();
  e.record.set("hooked", true);
}, "posts");

onRecordUpdateRequest((e) => {
  // the current record with the submitted data applied
  e.record.set("title", e.record.getString("title").toUpperCase());
  e.next();
}, "posts");

onRecordDeleteRequest((e) => {
  if (e.record.get("status") === "locked") {
    e.json(409, { message: "locked" });
    return;
  }
  e.next();
}, "posts");

onRecordViewRequest((e) => {
  e.record.set("viewer", e.auth ? e.auth.collection().name : "guest");
  e.next();
}, "posts");

onRecordsListRequest((e) => {
  e.records = e.records.filter((r) => r.get("status") !== "secret");
  e.result.totalItems = e.records.length;
  e.next();
}, "posts");

onRecordAfterCreateSuccess((e) => {
  const log = new Record($app.findCollectionByNameOrId("logs"));
  log.set("message", "created " + e.record.get("title"));
  $app.save(log);
  e.next();
}, "posts");

routerAdd("GET", "/api/js/hello/{name}", (e) => {
  const posts = $app.findRecordsByFilter("posts", "status = {:s}", "title", 10, 0, { s: "draft" });
  e.json(200, {
    hello: e.request.pathValue("name"),
    q: e.request.url.query().get("q"),
    titles: posts.map((p) => p.get("title")),
    auth: e.auth ? e.auth.email() : null,
  });
});

routerAdd("POST", "/api/js/echo", (e) => e.json(201, e.requestInfo().body), $apis.requireSuperuserAuth());
routerAdd("GET", "/api/js/text", (e) => e.string(200, "plain"));
routerAdd("GET", "/api/js/error", () => {
  throw new Error("boom");
});

routerAdd("GET", "/api/js/misc", (e) => {
  const rows = $app.db().newQuery("SELECT {:a}::int + 1 AS n, {:t} AS t").bind({ a: 41, t: "it's" }).all();
  const util = require(`${__hooks}/util.js`);
  const r = new Record($app.findCollectionByNameOrId("posts"), { title: "via js" });
  $app.save(r);
  r.set("status", "js");
  $app.save(r);
  const again = $app.findRecordById("posts", r.id);
  let missing = "";
  try {
    $app.findRecordById("posts", "nope");
  } catch (err) {
    missing = err.status;
  }
  e.json(200, {
    rows,
    util: util.double(21),
    sha: $security.sha256("abc"),
    random: $security.randomString(12).length,
    saved: again.get("status"),
    isNew: again.isNew(),
    missing,
  });
});

routerAdd("GET", "/api/js/http", (e) => {
  const res = $http.send({ url: e.request.url.query().get("url"), headers: { "x-test": "1" } });
  e.json(200, { status: res.statusCode, json: res.json });
});

cronAdd("jsjob", "*/5 * * * *", () => {
  const log = new Record("logs");
  log.set("message", "cron " + $app.countRecords("posts", "status = 'js'"));
  $app.save(log);
});

onBootstrap((e) => {
  e.next();
  console.log("bootstrapped");
});
"#;

async fn call(
    app: &Arc<App>,
    token: Option<&str>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = token.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.iter().map(|a| ("authorization", a.as_str())).collect();
    request(app, method, uri, &headers, body).await
}

async fn log_messages(app: &Arc<App>, token: &str) -> Vec<String> {
    let (_, logs) = call(
        app,
        Some(token),
        "GET",
        "/api/collections/logs/records?sort=created",
        None,
    )
    .await;
    logs["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["message"].as_str().unwrap().to_string())
        .collect()
}

async fn wait_for_log(app: &Arc<App>, token: &str, message: &str) {
    for _ in 0..100 {
        if log_messages(app, token).await.iter().any(|m| m == message) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("{message:?} not in {:?}", log_messages(app, token).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn js_hooks() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let hooks_dir = tempfile::tempdir().unwrap();
    std::fs::write(hooks_dir.path().join("main.pb.js"), HOOKS).unwrap();
    std::fs::write(
        hooks_dir.path().join("util.js"),
        "module.exports = { double: (n) => n * 2 };",
    )
    .unwrap();
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
    let admin = Some(token.as_str());
    for (name, fields) in [
        (
            "posts",
            json!([{ "name": "title", "type": "text" }, { "name": "status", "type": "text" }]),
        ),
        (
            "logs",
            json!([{ "name": "message", "type": "text" }, { "name": "created", "type": "autodate", "onCreate": true }]),
        ),
    ] {
        let (status, res) = call(&app, admin, "POST", "/api/collections", Some(json!({
            "name": name, "listRule": "", "viewRule": "", "createRule": "", "updateRule": "", "deleteRule": "",
            "fields": fields,
        }))).await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }

    // record request hooks
    let (status, post) = call(
        &app,
        None,
        "POST",
        "/api/collections/posts/records",
        Some(json!({ "title": "hello", "status": "published" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{post}");
    assert_eq!(post["status"], "draft");
    assert_eq!(post["hooked"], true);
    assert_eq!(post["collectionName"], "posts", "{post}");
    let id = post["id"].as_str().unwrap().to_string();
    wait_for_log(&app, &token, "created hello").await;

    let (status, res) = call(
        &app,
        None,
        "POST",
        "/api/collections/posts/records",
        Some(json!({ "title": "forbidden" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["message"], "Not this title.");
    assert_eq!(res["data"]["title"]["code"], "forbidden");

    let (status, post) = call(
        &app,
        None,
        "PATCH",
        &format!("/api/collections/posts/records/{id}"),
        Some(json!({ "title": "renamed" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{post}");
    assert_eq!(post["title"], "RENAMED");
    assert_eq!(post["status"], "draft");

    let (_, post) = call(
        &app,
        admin,
        "GET",
        &format!("/api/collections/posts/records/{id}"),
        None,
    )
    .await;
    assert_eq!(post["viewer"], "_superusers", "{post}");

    // routes
    let (status, res) = call(&app, admin, "GET", "/api/js/hello/ann?q=1", None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(
        res,
        json!({ "hello": "ann", "q": "1", "titles": ["RENAMED"], "auth": ADMIN_EMAIL })
    );
    let (status, _) = call(&app, None, "POST", "/api/js/echo", Some(json!({ "a": 1 }))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, res) = call(&app, admin, "POST", "/api/js/echo", Some(json!({ "a": 1 }))).await;
    assert_eq!((status, res), (StatusCode::CREATED, json!({ "a": 1 })));
    let (status, _, body) = raw_request(&app, "GET", "/api/js/text", &[], Vec::new()).await;
    assert_eq!(
        (status, body.as_slice()),
        (StatusCode::OK, b"plain".as_slice())
    );
    let (status, res) = call(&app, None, "GET", "/api/js/error", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        res["message"],
        "Something went wrong while processing your request."
    );

    let (status, res) = call(&app, None, "GET", "/api/js/misc", None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["rows"], json!([{ "n": 42, "t": "it's" }]));
    assert_eq!(res["util"], 42);
    assert_eq!(
        res["sha"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(res["random"], 12);
    assert_eq!(res["saved"], "js");
    assert_eq!(res["isNew"], false);
    assert_eq!(res["missing"], 404);
    // $app.save() runs the after-success hooks too
    wait_for_log(&app, &token, "created via js").await;

    // list hook
    call(
        &app,
        admin,
        "PATCH",
        &format!("/api/collections/posts/records/{id}"),
        Some(json!({ "status": "secret" })),
    )
    .await;
    let (_, list) = call(&app, None, "GET", "/api/collections/posts/records", None).await;
    assert_eq!(list["totalItems"], 1, "{list}");
    assert_eq!(list["items"][0]["title"], "via js");

    // $http.send against a local server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let router = axum::Router::new().route(
            "/data",
            axum::routing::get(|headers: axum::http::HeaderMap| async move {
                axum::Json(json!({ "x": headers.get("x-test").and_then(|v| v.to_str().ok()) }))
            }),
        );
        axum::serve(listener, router).await.unwrap();
    });
    let (status, res) = call(
        &app,
        None,
        "GET",
        &format!("/api/js/http?url=http://{address}/data"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res, json!({ "status": 200, "json": { "x": "1" } }));

    // cron jobs
    let (_, crons) = call(&app, admin, "GET", "/api/crons", None).await;
    assert_eq!(
        crons,
        json!([{ "id": "jsjob", "expression": "*/5 * * * *" }])
    );
    call(&app, admin, "POST", "/api/crons/jsjob", None).await;
    wait_for_log(&app, &token, "cron 1").await;

    // delete hook
    let (_, via_js) = call(
        &app,
        None,
        "GET",
        "/api/collections/posts/records?filter=title%3D'via%20js'",
        None,
    )
    .await;
    let js_id = via_js["items"][0]["id"].as_str().unwrap().to_string();
    call(
        &app,
        admin,
        "PATCH",
        &format!("/api/collections/posts/records/{js_id}"),
        Some(json!({ "status": "locked" })),
    )
    .await;
    let (status, res) = call(
        &app,
        None,
        "DELETE",
        &format!("/api/collections/posts/records/{js_id}"),
        None,
    )
    .await;
    assert_eq!(
        (status, res),
        (StatusCode::CONFLICT, json!({ "message": "locked" }))
    );
    let (status, _) = call(
        &app,
        None,
        "DELETE",
        &format!("/api/collections/posts/records/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn load_errors() {
    let dir = tempfile::tempdir().unwrap();
    let config = HooksConfig {
        dir: dir.path().to_path_buf(),
        pool_size: 1,
        watch: false,
    };
    // no files: nothing to do
    slonbase::js::load(&config, &mut slonbase::Extensions::default())
        .await
        .unwrap();
    std::fs::write(
        dir.path().join("bad.pb.js"),
        "routerAdd('GET', '/x', (e) => {\n  oops(\n",
    )
    .unwrap();
    let err = slonbase::js::load(&config, &mut slonbase::Extensions::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("bad.pb.js"), "{err}");
    std::fs::write(
        dir.path().join("bad.pb.js"),
        "cronAdd('x', 'not a cron', () => {})",
    )
    .unwrap();
    let err = slonbase::js::load(&config, &mut slonbase::Extensions::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("cron"), "{err}");
}
