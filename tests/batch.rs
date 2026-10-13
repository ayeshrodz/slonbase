//! Batch requests. See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

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

fn stored_files(dir: &std::path::Path) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.join("storage")];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                stack.push(entry.path());
            } else {
                count += 1;
            }
        }
    }
    count
}

#[tokio::test]
async fn batch_requests() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, dir) = ready_app(&tdb).await;
    let admin = Some(token.as_str());
    call(&app, admin, "POST", "/api/collections", Some(json!({
        "id": "pbc_authors00001", "name": "authors", "listRule": "", "viewRule": "", "createRule": "",
        "fields": [{ "name": "name", "type": "text", "required": true }],
    }))).await;
    let (status, res) = call(&app, admin, "POST", "/api/collections", Some(json!({
        "name": "books", "listRule": "", "viewRule": "", "createRule": null,
        "fields": [
            { "name": "title", "type": "text", "required": true },
            { "name": "author", "type": "relation", "collectionId": "pbc_authors00001", "maxSelect": 1 },
            { "name": "meta", "type": "json" },
            { "name": "cover", "type": "file" },
        ],
    }))).await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // off by default
    let one = json!({ "requests": [{ "method": "POST", "url": "/api/collections/authors/records", "body": { "name": "x" } }] });
    let (status, res) = call(&app, admin, "POST", "/api/batch", Some(one.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");
    call(
        &app,
        admin,
        "PATCH",
        "/api/settings",
        Some(json!({ "batch": { "enabled": true, "maxRequests": 6 } })),
    )
    .await;

    let (_, existing) = call(
        &app,
        admin,
        "POST",
        "/api/collections/authors/records",
        Some(json!({ "name": "Old" })),
    )
    .await;
    let existing_id = existing["id"].as_str().unwrap();
    let (status, results) = call(&app, admin, "POST", "/api/batch", Some(json!({ "requests": [
        { "method": "POST", "url": "/api/collections/authors/records", "body": { "id": "author000000001", "name": "Ann" } },
        { "method": "POST", "url": "/api/collections/books/records?expand=author", "body": { "title": "First", "author": "author000000001" } },
        { "method": "PATCH", "url": format!("/api/collections/authors/records/{existing_id}"), "body": { "name": "Renamed" } },
        { "method": "PUT", "url": "/api/collections/authors/records", "body": { "id": "author000000002", "name": "Upserted" } },
        { "method": "PUT", "url": "/api/collections/authors/records", "body": { "id": "author000000001", "name": "Ann B." } },
        { "method": "DELETE", "url": format!("/api/collections/authors/records/{existing_id}") },
    ] }))).await;
    assert_eq!(status, StatusCode::OK, "{results}");
    let results = results.as_array().unwrap();
    assert_eq!(results.len(), 6);
    assert_eq!(results[0]["status"], 200);
    assert_eq!(results[0]["body"]["name"], "Ann");
    // expanded after the batch commits, so relations show its final state
    assert_eq!(results[1]["body"]["expand"]["author"]["name"], "Ann B.");
    assert_eq!(results[2]["body"]["name"], "Renamed");
    assert_eq!(results[3]["body"]["id"], "author000000002");
    assert_eq!(
        results[4]["body"]["name"], "Ann B.",
        "an upsert of an existing record updates it"
    );
    assert_eq!(results[5], json!({ "status": 204, "body": null }));
    let (_, authors) = call(
        &app,
        admin,
        "GET",
        "/api/collections/authors/records?sort=name",
        None,
    )
    .await;
    let names: Vec<&str> = authors["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Ann B.", "Upserted"]);

    // all or nothing
    let (status, res) = call(&app, admin, "POST", "/api/batch", Some(json!({ "requests": [
        { "method": "POST", "url": "/api/collections/authors/records", "body": { "name": "Not kept" } },
        { "method": "POST", "url": "/api/collections/books/records", "body": { "author": "author000000001" } },
    ] }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["message"], "Batch transaction failed.");
    let failure = &res["data"]["requests"]["1"];
    assert_eq!(failure["code"], "batch_request_failed");
    assert_eq!(failure["response"]["status"], 400);
    assert!(failure["response"]["data"]["title"].is_object(), "{res}");
    let (_, authors) = call(&app, admin, "GET", "/api/collections/authors/records", None).await;
    assert_eq!(
        authors["totalItems"], 2,
        "the first request was rolled back"
    );

    // rules apply to each request, with the batch's auth
    let (status, res) = call(&app, None, "POST", "/api/batch", Some(json!({ "requests": [
        { "method": "POST", "url": "/api/collections/authors/records", "body": { "name": "Guest" } },
        { "method": "POST", "url": "/api/collections/books/records", "headers": { "Authorization": token }, "body": { "title": "x" } },
    ] }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["data"]["requests"]["1"]["response"]["status"], 403);

    // multipart: files per request, and they're discarded when the batch fails
    let payload = json!({ "requests": [
        { "method": "POST", "url": "/api/collections/books/records", "body": { "title": "With cover", "meta": "{\"a\":1}" } },
    ] })
    .to_string();
    let (content_type, body) = multipart(
        &[("@jsonPayload", &payload)],
        &[("requests.0.cover", "cover.txt", b"cover")],
    );
    let auth = format!("Bearer {token}");
    let (status, _, bytes) = raw_request(
        &app,
        "POST",
        "/api/batch",
        &[
            ("content-type", content_type.as_str()),
            ("authorization", auth.as_str()),
        ],
        body,
    )
    .await;
    let results: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, StatusCode::OK, "{results}");
    assert!(
        results[0]["body"]["cover"]
            .as_str()
            .unwrap()
            .ends_with(".txt")
    );
    assert_eq!(results[0]["body"]["meta"], json!({ "a": 1 }));
    let files_before = stored_files(dir.path());
    let payload = json!({ "requests": [
        { "method": "POST", "url": "/api/collections/books/records", "body": { "title": "Gone" } },
        { "method": "POST", "url": "/api/collections/books/records", "body": {} },
    ] })
    .to_string();
    let (content_type, body) = multipart(
        &[("@jsonPayload", &payload)],
        &[("requests.0.cover", "cover.txt", b"cover")],
    );
    let (status, _, _) = raw_request(
        &app,
        "POST",
        "/api/batch",
        &[
            ("content-type", content_type.as_str()),
            ("authorization", auth.as_str()),
        ],
        body,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        stored_files(dir.path()),
        files_before,
        "no files left behind"
    );

    // limits and unknown actions
    let seven: Vec<Value> = (0..7).map(|_| json!({ "method": "POST", "url": "/api/collections/authors/records", "body": { "name": "x" } })).collect();
    let (status, _) = call(
        &app,
        admin,
        "POST",
        "/api/batch",
        Some(json!({ "requests": seven })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, res) = call(
        &app,
        admin,
        "POST",
        "/api/batch",
        Some(json!({ "requests": [
        { "method": "GET", "url": "/api/collections/authors/records" },
    ] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(
        res["data"]["requests"]["0"]["response"]["message"],
        "Unknown batch request action."
    );
    let (status, _) = call(
        &app,
        admin,
        "POST",
        "/api/batch",
        Some(json!({ "requests": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    drop(app);
    tdb.drop_db().await;
}
