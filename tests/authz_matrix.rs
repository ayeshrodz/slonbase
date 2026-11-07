//! One authorization model, checked through every way a client can reach a record: list, view,
//! filters (with relation paths and `@collection`), `expand` (forward and back relations),
//! protected files, realtime, update, delete and batch. Each path must agree with the rule:
//! a record a client may not view is never returned, counted, matched, expanded, served or
//! announced to them. See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use common::*;
use slonbase::app::App;
use slonbase::server;

fn enc(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

async fn send(
    app: &Arc<App>,
    token: Option<&str>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let a = token.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = a.iter().map(|a| ("authorization", a.as_str())).collect();
    request(app, method, uri, &headers, body).await
}

async fn get(app: &Arc<App>, token: Option<&str>, uri: &str) -> (StatusCode, Value) {
    send(app, token, "GET", uri, None).await
}

async fn admin_post(app: &Arc<App>, admin: &str, uri: &str, body: Value) -> Value {
    let (status, res) = send(app, Some(admin), "POST", uri, Some(body)).await;
    assert!(status.is_success(), "POST {uri} -> {status}: {res}");
    res
}

fn ids(list: &Value) -> BTreeSet<String> {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

/// A connected SSE client.
struct Stream {
    body: Body,
    buffer: String,
    client_id: String,
}

impl Stream {
    async fn open(app: &Arc<App>) -> Self {
        let res = server::router(app.clone())
            .oneshot(Request::get("/api/realtime").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let mut stream = Self {
            body: res.into_body(),
            buffer: String::new(),
            client_id: String::new(),
        };
        let (event, data) = stream
            .next(Duration::from_secs(5))
            .await
            .expect("PB_CONNECT");
        assert_eq!(event, "PB_CONNECT");
        stream.client_id = data["clientId"].as_str().unwrap().to_string();
        stream
    }

    async fn next(&mut self, timeout: Duration) -> Option<(String, Value)> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                let (mut event, mut data) = (String::new(), String::new());
                for line in block.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        event = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim());
                    }
                }
                if event.is_empty() {
                    continue;
                }
                return Some((event, serde_json::from_str(&data).unwrap_or(Value::Null)));
            }
            let frame = tokio::time::timeout_at(deadline, self.body.frame())
                .await
                .ok()??
                .ok()?;
            if let Ok(bytes) = frame.into_data() {
                self.buffer.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
    }

    /// The ids of the documents announced within a short while.
    async fn docs_announced(&mut self) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        while let Some((event, data)) = self.next(Duration::from_millis(1500)).await {
            if event.starts_with("docs") {
                seen.insert(
                    data["record"]["id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        seen
    }
}

#[tokio::test]
async fn every_path_agrees_with_the_rules() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app(&tdb).await;
    // batch requests are off by default: without this, every batch below is refused for that
    // reason alone, and says nothing about the rules
    let (status, res) = send(
        &app,
        Some(&admin),
        "PATCH",
        "/api/settings",
        Some(json!({ "batch": { "enabled": true } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // users see only themselves, and their emails are hidden
    let users = admin_post(
        &app,
        &admin,
        "/api/collections",
        json!({
            "name": "users", "type": "auth",
            "listRule": "id = @request.auth.id", "viewRule": "id = @request.auth.id"
        }),
    )
    .await;
    // documents are private to their owner, with a protected file
    let docs = admin_post(
        &app,
        &admin,
        "/api/collections",
        json!({
            "name": "docs",
            "listRule": "owner = @request.auth.id", "viewRule": "owner = @request.auth.id",
            "updateRule": "owner = @request.auth.id", "deleteRule": "owner = @request.auth.id",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "secret", "type": "text" },
                { "name": "owner", "type": "relation", "collectionId": users["id"], "maxSelect": 1 },
                { "name": "attachment", "type": "file", "protected": true, "maxSelect": 1 }
            ]
        }),
    )
    .await;
    // comments are public and point at documents
    admin_post(
        &app,
        &admin,
        "/api/collections",
        json!({
            "name": "comments", "listRule": "", "viewRule": "",
            "fields": [
                { "name": "body", "type": "text" },
                { "name": "doc", "type": "relation", "collectionId": docs["id"], "maxSelect": 1 }
            ]
        }),
    )
    .await;

    let mut people = Vec::new();
    for email in ["alice@example.com", "bob@example.com"] {
        let user = admin_post(
            &app,
            &admin,
            "/api/collections/users/records",
            json!({ "email": email, "password": "password-123", "passwordConfirm": "password-123" }),
        )
        .await;
        let (_, signed) = send(
            &app,
            None,
            "POST",
            "/api/collections/users/auth-with-password",
            Some(json!({ "identity": email, "password": "password-123" })),
        )
        .await;
        people.push((
            user["id"].as_str().unwrap().to_string(),
            signed["token"].as_str().unwrap().to_string(),
        ));
    }
    let (alice_id, alice) = people[0].clone();
    let (bob_id, bob) = people[1].clone();

    // one document each, with a file, and a public comment on each
    let mut doc_ids = Vec::new();
    let admin_auth = format!("Bearer {admin}");
    for (owner, secret) in [(&alice_id, "alpha"), (&bob_id, "bravo")] {
        let (ctype, body) = multipart(
            &[("title", secret), ("secret", secret), ("owner", owner)],
            &[("attachment", "a.txt", secret.as_bytes())],
        );
        let (status, _, bytes) = raw_request(
            &app,
            "POST",
            "/api/collections/docs/records",
            &[("authorization", &admin_auth), ("content-type", &ctype)],
            body,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let doc: Value = serde_json::from_slice(&bytes).unwrap();
        doc_ids.push((
            doc["id"].as_str().unwrap().to_string(),
            doc["attachment"].as_str().unwrap().to_string(),
        ));
        admin_post(
            &app,
            &admin,
            "/api/collections/comments/records",
            json!({ "body": format!("on {secret}"), "doc": doc["id"] }),
        )
        .await;
    }
    let (alpha, alpha_file) = doc_ids[0].clone();
    let (bravo, bravo_file) = doc_ids[1].clone();

    // (who, token, the documents they may see)
    let principals: Vec<(&str, Option<&str>, BTreeSet<String>)> = vec![
        ("guest", None, BTreeSet::new()),
        ("alice", Some(&alice), BTreeSet::from([alpha.clone()])),
        ("bob", Some(&bob), BTreeSet::from([bravo.clone()])),
    ];

    for (who, token, allowed) in &principals {
        // list and count
        let (status, list) = get(&app, *token, "/api/collections/docs/records").await;
        assert_eq!(status, StatusCode::OK, "{who}: list");
        assert_eq!(&ids(&list), allowed, "{who}: list");
        assert_eq!(
            list["totalItems"].as_u64().unwrap() as usize,
            allowed.len(),
            "{who}: count"
        );

        // view
        for id in [&alpha, &bravo] {
            let (status, _) =
                get(&app, *token, &format!("/api/collections/docs/records/{id}")).await;
            let want = if allowed.contains(id) {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            };
            assert_eq!(status, want, "{who}: view {id}");
        }

        // filters on the collection, through a relation, through @collection and two levels deep
        for (secret, owner, id) in [("alpha", "alice", &alpha), ("bravo", "bob", &bravo)] {
            let visible = usize::from(allowed.contains(id));
            for (collection, filter) in [
                ("docs", format!("secret = '{secret}'")),
                ("comments", format!("doc.secret = '{secret}'")),
                (
                    "comments",
                    format!("@collection.docs.secret ?= '{secret}' && body = 'on {secret}'"),
                ),
                ("comments", format!("doc.owner.email ~ '{owner}@'")),
            ] {
                let (status, res) = get(
                    &app,
                    *token,
                    &format!(
                        "/api/collections/{collection}/records?filter={}",
                        enc(&filter)
                    ),
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{who}: {filter}: {res}");
                let n = res["totalItems"].as_u64().unwrap() as usize;
                assert!(
                    n <= visible,
                    "{who}: {collection} filter {filter} matched {n}, may see {visible}"
                );
            }
        }

        // expand, forward (comment -> doc)
        let (_, comments) = get(&app, *token, "/api/collections/comments/records?expand=doc").await;
        for c in comments["items"].as_array().unwrap() {
            let doc = c["doc"].as_str().unwrap();
            let expanded = !c["expand"]["doc"].is_null();
            assert_eq!(
                expanded,
                allowed.contains(doc),
                "{who}: expand comment.doc {doc}"
            );
        }
        // and back (user <- docs), on the client's own user record
        if let Some(t) = token {
            let me = if *who == "alice" { &alice_id } else { &bob_id };
            let (status, user) = get(
                &app,
                Some(t),
                &format!("/api/collections/users/records/{me}?expand=docs_via_owner"),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{who}: own user");
            let via: BTreeSet<String> = user["expand"]["docs_via_owner"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|d| d["id"].as_str().unwrap().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                via.is_subset(allowed),
                "{who}: back-relation expand {via:?}"
            );
        }

        // other users: not listed, and their emails can't be probed
        let (_, users_list) = get(&app, *token, "/api/collections/users/records").await;
        assert!(ids(&users_list).len() <= 1, "{who}: users list");
        let (_, probe) = get(
            &app,
            *token,
            &format!(
                "/api/collections/users/records?filter={}",
                enc("email ~ 'bob@'")
            ),
        )
        .await;
        assert_eq!(
            probe["totalItems"].as_u64().unwrap() as usize,
            usize::from(*who == "bob"),
            "{who}: email probe"
        );

        // protected files, with the client's own file token
        let file_token = match token {
            Some(t) => {
                let (status, res) = send(&app, Some(t), "POST", "/api/files/token", None).await;
                assert_eq!(status, StatusCode::OK, "{who}: file token");
                res["token"].as_str().unwrap().to_string()
            }
            None => String::new(),
        };
        for (id, file) in [(&alpha, &alpha_file), (&bravo, &bravo_file)] {
            let (status, _, _) = raw_request(
                &app,
                "GET",
                &format!("/api/files/docs/{id}/{file}?token={file_token}"),
                &[],
                Vec::new(),
            )
            .await;
            let want = if allowed.contains(id) {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            };
            assert_eq!(status, want, "{who}: file of {id}");
        }

        // writes to documents they can't see fail, directly and in a batch
        for id in [&alpha, &bravo] {
            if allowed.contains(id) {
                continue;
            }
            let uri = format!("/api/collections/docs/records/{id}");
            let (status, _) = send(
                &app,
                *token,
                "PATCH",
                &uri,
                Some(json!({ "title": "hijacked" })),
            )
            .await;
            assert!(status.is_client_error(), "{who}: update {id}: {status}");
            let (status, _) = send(&app, *token, "DELETE", &uri, None).await;
            assert!(status.is_client_error(), "{who}: delete {id}: {status}");
            let (status, res) = send(
                &app,
                *token,
                "POST",
                "/api/batch",
                Some(json!({ "requests": [
                    { "method": "PATCH", "url": uri, "body": { "title": "hijacked" } }
                ]})),
            )
            .await;
            assert!(
                !status.is_success(),
                "{who}: batch update {id}: {status} {res}"
            );
        }
    }
    // and changed nothing
    for id in [&alpha, &bravo] {
        let (_, doc) = get(
            &app,
            Some(&admin),
            &format!("/api/collections/docs/records/{id}"),
        )
        .await;
        assert_ne!(doc["title"], "hijacked", "{id} was changed");
    }

    // realtime: each subscriber hears only about the documents it may view
    let mut streams = Vec::new();
    for (who, token, allowed) in &principals {
        let stream = Stream::open(&app).await;
        let (status, res) = send(
            &app,
            *token,
            "POST",
            "/api/realtime",
            Some(json!({ "clientId": stream.client_id, "subscriptions": ["docs"] })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{who}: subscribe {res}");
        streams.push((who.to_string(), allowed.clone(), stream));
    }
    for id in [&alpha, &bravo] {
        let (status, res) = send(
            &app,
            Some(&admin),
            "PATCH",
            &format!("/api/collections/docs/records/{id}"),
            Some(json!({ "title": "edited" })),
        )
        .await;
        assert!(status.is_success(), "{res}");
    }
    for (who, allowed, mut stream) in streams {
        assert_eq!(stream.docs_announced().await, allowed, "{who}: realtime");
    }

    tdb.drop_db().await;
}
