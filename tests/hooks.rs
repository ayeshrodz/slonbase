//! Rust extensions: hooks, routes, cron jobs and the record service. See `setup_flow.rs`
//! for how to point the tests at a database.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use serde_json::{Map, Value, json};

use common::*;
use slonbase::app::App;
use slonbase::error::ApiError;

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

async fn wait_for(log: &Mutex<Vec<String>>, entry: &str) {
    for _ in 0..100 {
        if log.lock().unwrap().iter().any(|e| e == entry) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{entry:?} not in {:?}", log.lock().unwrap());
}

fn extensions(log: Arc<Mutex<Vec<String>>>) -> slonbase::Extensions {
    let mut ext = slonbase::Extensions::default();
    let hooks = &mut ext.hooks;

    hooks
        .on_record_create_request
        .add(&["posts"], |mut e, next| async move {
            if e.record.get("title") == Some(&json!("forbidden")) {
                return Err(ApiError::bad_request("Not this title."));
            }
            e.record.insert("status".into(), json!("draft"));
            let mut e = next.run(e).await?;
            e.record.insert("hooked".into(), json!(true));
            Ok(e)
        });
    // limited to another collection
    hooks
        .on_record_create_request
        .add(&["others"], |_, _| async {
            Err(ApiError::bad_request("wrong collection"))
        });
    hooks
        .on_record_update_request
        .add(&["posts"], |mut e, next| async move {
            // the current record with the submitted data applied
            let title = e.record["title"]
                .as_str()
                .unwrap_or_default()
                .to_uppercase();
            e.record.insert("title".into(), json!(title));
            next.run(e).await
        });
    hooks
        .on_record_delete_request
        .add(&["posts"], |mut e, next| async move {
            if e.record.get("status") == Some(&json!("locked")) {
                e.response = Some(
                    (StatusCode::CONFLICT, Json(json!({ "message": "locked" }))).into_response(),
                );
                return Ok(e);
            }
            next.run(e).await
        });
    hooks
        .on_record_view_request
        .add(&["posts"], |mut e, next| async move {
            e.record.insert("viewed".into(), json!(true));
            next.run(e).await
        });
    hooks
        .on_records_list_request
        .add(&["posts"], |mut e, next| async move {
            e.records.retain(|r| r["status"] != "secret");
            next.run(e).await
        });
    for (hook, action) in [
        (&mut hooks.on_record_after_create_success, "create"),
        (&mut hooks.on_record_after_update_success, "update"),
        (&mut hooks.on_record_after_delete_success, "delete"),
    ] {
        let log = log.clone();
        hook.add(&["posts"], move |e, next| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(format!(
                    "{action} {}",
                    e.record["title"].as_str().unwrap_or_default()
                ));
                next.run(e).await
            }
        });
    }
    {
        let log = log.clone();
        hooks
            .cron("cleanup", "0 3 * * *", move |core| {
                let log = log.clone();
                async move {
                    let n = core.count_records("posts", "", &Map::new()).await?;
                    log.lock().unwrap().push(format!("cron {n}"));
                    Ok(())
                }
            })
            .unwrap();
    }
    assert!(
        hooks
            .cron("bad", "not a cron", |_| async { Ok(()) })
            .is_err()
    );
    {
        let log = log.clone();
        hooks.on_ready(move |_| {
            let log = log.clone();
            async move { log.lock().unwrap().push("ready".into()) }
        });
    }

    ext.route(
        "/api/hello/{status}",
        get(
            |State(app): State<Arc<App>>, Path(status): Path<String>| async move {
                let core = app.core()?;
                let mut params = Map::new();
                params.insert("status".into(), json!(status));
                let posts = core
                    .find_records_by_filter("posts", "status = {:status}", "title", 0, 0, &params)
                    .await?;
                let titles: Vec<Value> = posts.iter().map(|p| p["title"].clone()).collect();
                Ok::<_, ApiError>(Json(json!({ "titles": titles })))
            },
        ),
    );
    ext
}

#[tokio::test]
async fn rust_extensions() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let log = Arc::new(Mutex::new(Vec::new()));
    let (app, token, _dir) = ready_app_with(&tdb, extensions(log.clone())).await;
    let admin = Some(token.as_str());
    wait_for(&log, "ready").await;
    let (status, res) = call(&app, admin, "POST", "/api/collections", Some(json!({
        "name": "posts", "listRule": "", "viewRule": "", "createRule": "", "updateRule": "", "deleteRule": "",
        "fields": [
            { "name": "title", "type": "text" },
            { "name": "status", "type": "text" },
        ],
    }))).await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // create: the hook changes the data, and the response after `next`
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
    let id = post["id"].as_str().unwrap().to_string();
    wait_for(&log, "create hello").await;
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

    // update: hooks see the current record merged with the request
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
    assert_eq!(post["status"], "draft", "untouched fields stay");
    wait_for(&log, "update RENAMED").await;

    // view and list
    let (_, post) = call(
        &app,
        None,
        "GET",
        &format!("/api/collections/posts/records/{id}"),
        None,
    )
    .await;
    assert_eq!(post["viewed"], true);

    // the record service (no rules, after-success hooks fire)
    let core = app.core().unwrap();
    let mut data = Map::new();
    data.insert("title".into(), json!("from rust"));
    data.insert("status".into(), json!("secret"));
    let saved = core.save_record("posts", data).await.unwrap();
    assert_eq!(saved["collectionName"], "posts");
    wait_for(&log, "create from rust").await;
    let mut change = Map::new();
    change.insert("id".into(), saved["id"].clone());
    change.insert("title".into(), json!("from rust 2"));
    let updated = core.save_record("posts", change).await.unwrap();
    assert_eq!(updated["status"], "secret", "only the given fields change");
    assert_eq!(
        core.find_record_by_id("posts", saved["id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap()["title"],
        "from rust 2"
    );
    let mut params = Map::new();
    params.insert("s".into(), json!("secret"));
    assert_eq!(
        core.count_records("posts", "status = {:s}", &params)
            .await
            .unwrap(),
        1
    );
    assert!(
        core.find_record_by_id("posts", "missing")
            .await
            .unwrap()
            .is_none()
    );

    let (_, list) = call(&app, None, "GET", "/api/collections/posts/records", None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");

    // custom route
    let (status, res) = call(&app, None, "GET", "/api/hello/secret", None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res, json!({ "titles": ["from rust 2"] }));

    // cron jobs are listed and can be run from the dashboard
    let (_, crons) = call(&app, admin, "GET", "/api/crons", None).await;
    assert_eq!(
        crons,
        json!([{ "id": "cleanup", "expression": "0 3 * * *" }])
    );
    let (status, _) = call(&app, admin, "POST", "/api/crons/cleanup", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    wait_for(&log, "cron 2").await;

    // delete: a hook can answer instead
    core.save_record(
        "posts",
        [
            ("id".to_string(), json!(id)),
            ("status".to_string(), json!("locked")),
        ]
        .into_iter()
        .collect(),
    )
    .await
    .unwrap();
    let (status, res) = call(
        &app,
        None,
        "DELETE",
        &format!("/api/collections/posts/records/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{res}");
    core.delete_record("posts", &id).await.unwrap();
    wait_for(&log, "delete RENAMED").await;
    let (status, _) = call(
        &app,
        None,
        "DELETE",
        &format!("/api/collections/posts/records/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // batch requests run the request hooks, with their writes in the batch's transaction
    let (status, res) = call(
        &app,
        admin,
        "PATCH",
        "/api/settings",
        Some(json!({ "batch": { "enabled": true, "maxRequests": 10 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, results) = call(&app, None, "POST", "/api/batch", Some(json!({ "requests": [
        { "method": "POST", "url": "/api/collections/posts/records", "body": { "id": "batchpost000001", "title": "in batch", "status": "published" } },
        // sees the record created earlier in the same batch
        { "method": "PATCH", "url": "/api/collections/posts/records/batchpost000001", "body": { "title": "patched" } },
        { "method": "PUT", "url": "/api/collections/posts/records", "body": { "id": "batchpost000001", "status": "locked" } },
        { "method": "DELETE", "url": "/api/collections/posts/records/batchpost000001" },
    ] }))).await;
    assert_eq!(status, StatusCode::OK, "{results}");
    assert_eq!(results[0]["body"]["status"], "draft", "{results}");
    assert_eq!(results[0]["body"]["hooked"], true);
    assert_eq!(results[1]["body"]["title"], "PATCHED");
    assert_eq!(results[1]["body"]["status"], "draft");
    // the hook answered instead of deleting
    assert_eq!(
        results[3],
        json!({ "status": 409, "body": { "message": "locked" } })
    );
    wait_for(&log, "update PATCHED").await;
    assert!(
        core.find_record_by_id("posts", "batchpost000001")
            .await
            .unwrap()
            .is_some()
    );

    // a hook's error fails the whole batch, writes made before it included
    let (status, res) = call(&app, None, "POST", "/api/batch", Some(json!({ "requests": [
        { "method": "POST", "url": "/api/collections/posts/records", "body": { "id": "batchpost000003", "title": "kept?" } },
        { "method": "POST", "url": "/api/collections/posts/records", "body": { "title": "forbidden" } },
    ] }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(
        res["data"]["requests"]["1"]["response"]["message"], "Not this title.",
        "{res}"
    );
    assert!(
        core.find_record_by_id("posts", "batchpost000003")
            .await
            .unwrap()
            .is_none()
    );

    drop(core);
    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn rust_data_hooks() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let mut ext = slonbase::Extensions::default();
    ext.hooks
        .on_collection_create_request
        .add(&[], |mut e, next| async move {
            if let Some(c) = e.data.get_mut("collection").and_then(Value::as_object_mut) {
                c.insert("listRule".into(), json!(""));
            }
            next.run(e).await
        });
    ext.hooks.on_batch_request.add(&[], |e, next| async move {
        let refused = e.data["batch"]
            .as_array()
            .is_some_and(|b| b.iter().any(|r| r["body"]["title"] == "refuse"));
        if refused {
            return Err(ApiError::bad_request("Not this batch."));
        }
        next.run(e).await
    });
    let (app, token, _dir) = ready_app_with(&tdb, ext).await;
    let admin = Some(token.as_str());

    let (status, res) = call(
        &app,
        admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "notes", "createRule": "",
            "fields": [{ "name": "title", "type": "text" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["listRule"], "", "the hook set the rule");

    let (status, res) = call(
        &app,
        admin,
        "PATCH",
        "/api/settings",
        Some(json!({
            "batch": { "enabled": true, "maxRequests": 10, "timeout": 3 },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let batch = |title: &str| {
        json!({ "requests": [{
            "method": "POST", "url": "/api/collections/notes/records", "body": { "title": title },
        }] })
    };
    let (status, res) = call(&app, None, "POST", "/api/batch", Some(batch("fine"))).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, res) = call(&app, None, "POST", "/api/batch", Some(batch("refuse"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["message"], "Not this batch.");
}

#[tokio::test]
async fn rust_execute_and_error_hooks() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut ext = slonbase::Extensions::default();
    ext.hooks
        .on_record_create_execute
        .add(&["items"], |e, next| async move {
            let title = e.record["title"].as_str().unwrap_or_default().to_string();
            if title == "skip" {
                return Ok(e);
            }
            let e = next.run(e).await?;
            if title == "rollback" {
                return Err(ApiError::bad_request("After the write."));
            }
            Ok(e)
        });
    {
        let log = log.clone();
        ext.hooks
            .on_record_create_execute
            .add(&["items"], move |e, next| {
                let log = log.clone();
                async move {
                    let e = next.run(e).await?;
                    // after the write: the record as written
                    let id = e.record["id"].as_str().unwrap_or_default();
                    log.lock().unwrap().push(format!("written {}", id.len()));
                    Ok(e)
                }
            });
    }
    {
        let log = log.clone();
        ext.hooks
            .on_record_after_create_error
            .add(&["items"], move |e, next| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(format!(
                        "create-error {} {}",
                        e.record["title"].as_str().unwrap_or_default(),
                        e.error.status.as_u16()
                    ));
                    next.run(e).await
                }
            });
    }
    let (app, token, _dir) = ready_app_with(&tdb, ext).await;
    let admin = Some(token.as_str());
    let (status, res) = call(
        &app,
        admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "items", "listRule": "", "createRule": "",
            "fields": [{ "name": "title", "type": "text", "max": 10 }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let create = |title: &str| {
        let body = json!({ "title": title });
        let app = app.clone();
        async move {
            call(
                &app,
                None,
                "POST",
                "/api/collections/items/records",
                Some(body),
            )
            .await
        }
    };

    let (status, _) = create("fine").await;
    assert_eq!(status, StatusCode::OK);
    wait_for(&log, "written 15").await;

    // an error after the write rolls it back
    let (status, res) = create("rollback").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["message"], "After the write.");
    wait_for(&log, "create-error rollback 400").await;

    // a handler that doesn't call next cancels the write
    let (status, res) = create("skip").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["message"], "The write was cancelled by a hook.");

    let (_, list) = call(&app, None, "GET", "/api/collections/items/records", None).await;
    let titles: Vec<&Value> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["title"])
        .collect();
    assert_eq!(titles, [&json!("fine")]);

    // a batch that fails: its earlier create is undone and reported too
    let (status, res) = call(
        &app,
        admin,
        "PATCH",
        "/api/settings",
        Some(json!({
            "batch": { "enabled": true, "maxRequests": 10, "timeout": 3 },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let requests = json!({ "requests": [
        { "method": "POST", "url": "/api/collections/items/records", "body": { "title": "first" } },
        { "method": "POST", "url": "/api/collections/items/records", "body": { "title": "far too long" } },
    ] });
    let (status, res) = call(&app, None, "POST", "/api/batch", Some(requests)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    wait_for(&log, "create-error far too long 400").await;
    wait_for(&log, "create-error first 400").await;
}
