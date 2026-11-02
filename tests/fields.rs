//! The `fields` query parameter, as in PocketBase: nested paths, `*`, the `:excerpt` modifier,
//! on every JSON answer (records, auth, settings, batch). Needs PostgreSQL.

mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;

#[tokio::test]
async fn fields_everywhere() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = format!("Bearer {token}");
    let call = |method: &'static str, uri: String, body: Option<Value>| {
        let app = app.clone();
        let auth = auth.clone();
        async move {
            request(
                &app,
                method,
                &uri,
                &[("authorization", auth.as_str())],
                body,
            )
            .await
        }
    };

    let (status, authors) = call(
        "POST",
        "/api/collections".into(),
        Some(json!({ "name": "authors", "listRule": "", "viewRule": "",
                     "fields": [{ "name": "name", "type": "text" }, { "name": "bio", "type": "editor" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{authors}");
    let (status, posts) = call(
        "POST",
        "/api/collections".into(),
        Some(json!({ "name": "posts", "listRule": "", "viewRule": "", "createRule": "",
                     "fields": [
                         { "name": "title", "type": "text" },
                         { "name": "body", "type": "editor" },
                         { "name": "author", "type": "relation", "collectionId": authors["id"] },
                         { "name": "readers", "type": "relation", "collectionId": authors["id"], "maxSelect": 9 },
                     ] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{posts}");
    let (_, author) = call(
        "POST",
        "/api/collections/authors/records".into(),
        Some(json!({ "name": "Ann", "bio": "<p>Writes <i>a lot</i> about things.</p>" })),
    )
    .await;
    let (status, post) = call(
        "POST",
        "/api/collections/posts/records?fields=id,body:excerpt(12,true)".into(),
        Some(
            json!({ "title": "First", "body": "<h1>Hello</h1><p>wonderful world</p>",
                     "author": author["id"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{post}");
    // the create's answer is picked
    assert_eq!(post, json!({ "id": post["id"], "body": "Hello wonder..." }));
    call(
        "POST",
        "/api/collections/posts/records".into(),
        Some(json!({ "title": "Second", "body": "<p>x</p>" })),
    )
    .await;

    // lists: each item, nested into the expand, the envelope kept
    let (status, list) = call(
        "GET",
        "/api/collections/posts/records?sort=title&expand=author,readers&fields=title,expand.author.bio:excerpt(6,true)"
            .into(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["totalItems"], 2);
    assert_eq!(
        list["items"],
        json!([
            { "title": "First", "expand": { "author": { "bio": "Writes..." } } },
            // expanded with nothing found: an empty expand, as PocketBase
            { "title": "Second", "expand": {} },
        ])
    );
    // `*` keeps the rest; a relation with nothing found isn't in the expand
    let (_, list) = call(
        "GET",
        "/api/collections/posts/records?sort=title&expand=readers&fields=*,body:excerpt(3)".into(),
        None,
    )
    .await;
    assert_eq!(list["items"][0]["body"], "Hel");
    assert_eq!(list["items"][0]["collectionName"], "posts");
    assert_eq!(list["items"][0]["expand"], json!({}));
    // one record
    let id = post["id"].as_str().unwrap();
    let (_, one) = call(
        "GET",
        format!("/api/collections/posts/records/{id}?expand=author&fields=expand.author.name"),
        None,
    )
    .await;
    assert_eq!(one, json!({ "expand": { "author": { "name": "Ann" } } }));

    // any JSON answer: auth, settings, collections
    let (status, signed_in) = request(
        &app,
        "POST",
        "/api/collections/_superusers/auth-with-password?fields=record.email",
        &[],
        Some(json!({ "identity": ADMIN_EMAIL, "password": ADMIN_PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{signed_in}");
    assert_eq!(signed_in, json!({ "record": { "email": ADMIN_EMAIL } }));
    let (_, settings) = call("GET", "/api/settings?fields=batch.enabled".into(), None).await;
    assert_eq!(settings, json!({ "batch": { "enabled": false } }));
    let (_, collection) = call(
        "GET",
        "/api/collections/posts?fields=name,fields.name".into(),
        None,
    )
    .await;
    assert_eq!(collection["name"], "posts");
    assert!(
        collection["fields"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f.as_object().unwrap().len() == 1)
    );
    // errors aren't picked
    let (status, missing) = call(
        "GET",
        "/api/collections/posts/records/nope?fields=id".into(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(missing.get("message").is_some());

    // a `fields` that can't be parsed: a 400, as PocketBase
    for bad in [
        "body:excerpt(0)",
        "body:nope(1)",
        "id,(title",
        "body:excerpt(1,2,3)",
    ] {
        let q = form_urlencoded::byte_serialize(bad.as_bytes()).collect::<String>();
        let (status, res) = call(
            "GET",
            format!("/api/collections/posts/records?fields={q}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {res}");
        let (status, _) = call("GET", format!("/api/settings?fields={q}"), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    // and a change with one is refused before it's made (PocketBase makes it, then answers 400)
    let (status, _) = call(
        "POST",
        "/api/collections/posts/records?fields=body:excerpt(0)".into(),
        Some(json!({ "title": "never" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, list) = call(
        "GET",
        "/api/collections/posts/records?filter=title='never'&fields=id".into(),
        None,
    )
    .await;
    assert_eq!(list["totalItems"], 0, "{list}");

    // batch: each request's own `fields`, and the batch's over the results
    let (status, _) = call(
        "PATCH",
        "/api/settings".into(),
        Some(json!({ "batch": { "enabled": true } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, results) = call(
        "POST",
        "/api/batch".into(),
        Some(json!({ "requests": [
            { "method": "PATCH", "url": format!("/api/collections/posts/records/{id}?fields=title"),
              "body": { "title": "Renamed" } },
        ] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{results}");
    assert_eq!(
        results,
        json!([{ "status": 200, "body": { "title": "Renamed" } }])
    );
    let (_, results) = call(
        "POST",
        "/api/batch?fields=status".into(),
        Some(json!({ "requests": [
            { "method": "PATCH", "url": format!("/api/collections/posts/records/{id}"), "body": { "title": "Again" } },
        ] })),
    )
    .await;
    assert_eq!(results, json!([{ "status": 200 }]));
    // a request's `fields` that can't be parsed fails the batch before anything runs
    let (status, res) = call(
        "POST",
        "/api/batch".into(),
        Some(json!({ "requests": [
            { "method": "PATCH", "url": format!("/api/collections/posts/records/{id}"), "body": { "title": "Lost" } },
            { "method": "PATCH", "url": format!("/api/collections/posts/records/{id}?fields=x:excerpt()"), "body": {} },
        ] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    let (_, one) = call(
        "GET",
        format!("/api/collections/posts/records/{id}?fields=title"),
        None,
    )
    .await;
    assert_eq!(one, json!({ "title": "Again" }));
}
