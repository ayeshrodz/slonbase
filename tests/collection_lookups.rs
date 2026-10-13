//! `@collection` lookups in filters and rules. See `setup_flow.rs` for how to point the
//! tests at a database.

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

fn q(filter: &str) -> String {
    form_urlencoded::byte_serialize(filter.as_bytes()).collect()
}

async fn titles(app: &Arc<App>, token: Option<&str>, query: &str) -> (StatusCode, Vec<String>) {
    let (status, list) = call(
        app,
        token,
        "GET",
        &format!("/api/collections/posts/records?sort=title&{query}"),
        None,
    )
    .await;
    let items = list["items"].as_array().cloned().unwrap_or_default();
    (
        status,
        items
            .iter()
            .map(|r| r["title"].as_str().unwrap_or_default().to_string())
            .collect(),
    )
}

#[tokio::test]
async fn collection_lookups() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let admin = Some(token.as_str());
    for body in [
        json!({ "id": "pbc_posts000001", "name": "posts", "listRule": null, "viewRule": "", "createRule": "", "fields": [{ "name": "title", "type": "text" }] }),
        json!({ "name": "members", "createRule": "", "fields": [
            { "name": "post", "type": "relation", "collectionId": "pbc_posts000001", "maxSelect": 1 },
            { "name": "user", "type": "text" },
        ] }),
        json!({ "name": "empty", "fields": [{ "name": "name", "type": "text" }] }),
    ] {
        let (status, res) = call(&app, admin, "POST", "/api/collections", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    let mut ids = Vec::new();
    for title in ["p1", "p2", "p3"] {
        let (_, post) = call(
            &app,
            None,
            "POST",
            "/api/collections/posts/records",
            Some(json!({ "title": title })),
        )
        .await;
        ids.push(post["id"].as_str().unwrap().to_string());
    }
    for (post, user) in [(&ids[0], "ann"), (&ids[1], "bob")] {
        let (status, res) = call(
            &app,
            None,
            "POST",
            "/api/collections/members/records",
            Some(json!({ "post": post, "user": user })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }

    // references to the same collection share a row
    let (status, t) = titles(
        &app,
        admin,
        &format!(
            "filter={}",
            q("@collection.members.post = id && @collection.members.user = 'ann'")
        ),
    )
    .await;
    assert_eq!((status, t), (StatusCode::OK, vec!["p1".to_string()]));
    // aliases are independent rows
    let (_, t) = titles(
        &app,
        admin,
        &format!(
            "filter={}",
            q("@collection.members:a.post = id && @collection.members:b.user = 'bob'")
        ),
    )
    .await;
    assert_eq!(t, vec!["p1", "p2"]);
    // relation paths from a lookup
    let (_, t) = titles(
        &app,
        admin,
        &format!(
            "filter={}",
            q("@collection.members.post.title = title && @collection.members.user = 'bob'")
        ),
    )
    .await;
    assert_eq!(t, vec!["p2"]);
    // other conditions still work when nothing matches, or the lookup collection is empty
    let (_, t) = titles(
        &app,
        admin,
        &format!(
            "filter={}",
            q("title = 'p3' || @collection.members.user = 'nobody'")
        ),
    )
    .await;
    assert_eq!(t, vec!["p3"]);
    let (_, t) = titles(
        &app,
        admin,
        &format!(
            "filter={}",
            q("title = 'p3' || @collection.empty.name = 'x'")
        ),
    )
    .await;
    assert_eq!(t, vec!["p3"]);

    // in API rules
    let (status, res) = call(&app, admin, "PATCH", "/api/collections/posts", Some(json!({
        "listRule": "@collection.members.post = id && @collection.members.user = @request.query.u",
    }))).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (_, t) = titles(&app, None, "u=bob").await;
    assert_eq!(t, vec!["p2"]);
    let (_, t) = titles(&app, None, "u=zed").await;
    assert!(t.is_empty());

    // unknown collections and fields
    let (status, _) = titles(
        &app,
        admin,
        &format!("filter={}", q("@collection.nope.x = 1")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, res) = call(
        &app,
        admin,
        "PATCH",
        "/api/collections/posts",
        Some(json!({ "listRule": "@collection.members.nope = 1" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");

    drop(app);
    tdb.drop_db().await;
}
