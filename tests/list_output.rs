//! Lists and single records written straight from the rows (nothing changes them on the way
//! out) are the same, byte for byte, as those built record by record (`fields=*` and an empty
//! `expand` take that path). Needs PostgreSQL.

mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;

#[tokio::test]
async fn both_list_paths_write_the_same_bytes() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = format!("Bearer {token}");
    let admin = |method: &'static str, uri: String, body: Option<Value>| {
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

    let (status, users) = admin(
        "POST",
        "/api/collections".into(),
        Some(json!({
            "name": "people", "type": "auth", "listRule": "", "viewRule": "", "createRule": "",
            "fields": [{ "name": "nick", "type": "text" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{users}");
    let (status, res) = admin("POST", "/api/collections".into(), Some(json!({
        "name": "things", "listRule": "", "viewRule": "",
        "fields": [
            { "name": "title", "type": "text" },
            { "name": "count", "type": "number" },
            { "name": "done", "type": "bool" },
            { "name": "contact", "type": "email" },
            { "name": "site", "type": "url" },
            { "name": "body", "type": "editor" },
            { "name": "due", "type": "date" },
            { "name": "tags", "type": "select", "values": ["a", "b", "c"], "maxSelect": 3 },
            { "name": "meta", "type": "json" },
            { "name": "owners", "type": "relation", "collectionId": users["id"], "maxSelect": 5 },
            { "name": "place", "type": "geoPoint" },
            { "name": "files", "type": "file", "maxSelect": 3 }
        ],
    }))).await;
    assert_eq!(status, StatusCode::OK, "{res}");

    let mut people = Vec::new();
    for (i, visible) in [(0, true), (1, false), (2, false)] {
        let (status, person) = request(&app, "POST", "/api/collections/people/records", &[], Some(json!({
            "email": format!("p{i}@example.com"), "password": "password123",
            "passwordConfirm": "password123", "emailVisibility": visible, "nick": format!("n{i}"),
        }))).await;
        assert_eq!(status, StatusCode::OK, "{person}");
        people.push(person["id"].as_str().unwrap().to_string());
    }
    for (i, record) in [
        json!({ "title": "plain \"quoted\" \\ \u{1F418} \n", "count": 3, "done": true, "contact": "a@b.co",
                "site": "https://example.com/a?b=c", "body": "<p>hi</p>", "due": "2026-01-02 03:04:05.678Z",
                "tags": ["a", "c"], "meta": { "z": 1, "a": [1.5, null, "x"], "n": { "ok": true } },
                "owners": people.clone(), "place": { "lon": 12.5, "lat": -45.25 } }),
        json!({ "title": "", "count": -0.125, "meta": null }),
        json!({ "count": 1e21, "meta": [1, 2, 3] }),
        json!({ "count": 9007199254740993.0, "meta": "text", "tags": ["b"] }),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, res) = admin("POST", "/api/collections/things/records".into(), Some(record)).await;
        assert_eq!(status, StatusCode::OK, "record {i}: {res}");
    }

    let bytes = |uri: String, token: Option<String>| {
        let app = app.clone();
        async move {
            let auth = token.map(|t| format!("Bearer {t}"));
            let headers: Vec<(&str, &str)> =
                auth.iter().map(|a| ("authorization", a.as_str())).collect();
            let (status, _, body) = raw_request(&app, "GET", &uri, &headers, Vec::new()).await;
            assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
            body
        }
    };
    // a signed-in person sees their own email, the others' only if they show it
    let (_, login) = request(
        &app,
        "POST",
        "/api/collections/people/auth-with-password",
        &[],
        Some(json!({
            "identity": "p1@example.com", "password": "password123",
        })),
    )
    .await;
    let person = login["token"].as_str().unwrap().to_string();

    for (uri, caller) in [
        (
            "/api/collections/things/records?sort=title".to_string(),
            None,
        ),
        (
            "/api/collections/things/records?sort=-count&perPage=2&page=2".to_string(),
            None,
        ),
        (
            "/api/collections/things/records?skipTotal=1".to_string(),
            Some(token.clone()),
        ),
        (
            "/api/collections/people/records?sort=nick".to_string(),
            None,
        ),
        (
            "/api/collections/people/records?sort=nick".to_string(),
            Some(person.clone()),
        ),
        (
            "/api/collections/people/records?sort=nick".to_string(),
            Some(token.clone()),
        ),
    ] {
        let fast = bytes(uri.clone(), caller.clone()).await;
        let built = bytes(format!("{uri}&fields=*"), caller.clone()).await;
        assert_eq!(
            String::from_utf8_lossy(&fast),
            String::from_utf8_lossy(&built),
            "{uri} as {}",
            if caller.is_some() {
                "a signed-in caller"
            } else {
                "a guest"
            }
        );
    }
    // single records too: written from the row, or built (an empty `expand` takes that path)
    for (collection, caller) in [
        ("things", None),
        ("things", Some(token.clone())),
        ("people", None),
        ("people", Some(person.clone())),
        ("people", Some(token.clone())),
    ] {
        let list: Value = serde_json::from_slice(
            &bytes(
                format!("/api/collections/{collection}/records?perPage=50"),
                caller.clone(),
            )
            .await,
        )
        .unwrap();
        let items = list["items"].as_array().unwrap();
        assert!(!items.is_empty());
        for item in items {
            let uri = format!(
                "/api/collections/{collection}/records/{}",
                item["id"].as_str().unwrap()
            );
            let fast = bytes(uri.clone(), caller.clone()).await;
            let built = bytes(format!("{uri}?expand="), caller.clone()).await;
            assert_eq!(
                String::from_utf8_lossy(&fast),
                String::from_utf8_lossy(&built),
                "{uri}"
            );
        }
    }

    // and the hidden emails are hidden, with the other fields in their order
    let list: Value = serde_json::from_slice(
        &bytes("/api/collections/people/records?sort=nick".into(), None).await,
    )
    .unwrap();
    let items = list["items"].as_array().unwrap();
    assert_eq!(items[0]["email"], "p0@example.com");
    assert!(items[1].get("email").is_none());
    let keys: Vec<&str> = items[1]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let all: Vec<&str> = items[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .filter(|k| *k != "email")
        .collect();
    assert_eq!(keys, all, "the order of the other fields is kept");

    drop(app);
    tdb.drop_db().await;
}
