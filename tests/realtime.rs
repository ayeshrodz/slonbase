//! End-to-end tests of realtime subscriptions against PostgreSQL.
//! See `setup_flow.rs` for how to point them at a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use common::*;
use slonbase::app::{App, Mode};
use slonbase::server;

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
        assert_eq!(res.headers()["x-accel-buffering"], "no");
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

    /// The next event within `timeout`, skipping keep-alive comments.
    async fn next(&mut self, timeout: Duration) -> Option<(String, Value)> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                let mut event = String::new();
                let mut data = String::new();
                for line in block.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        event = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim());
                    }
                }
                if event.is_empty() {
                    continue; // a comment / keep-alive
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

    async fn expect(&mut self) -> (String, Value) {
        self.next(Duration::from_secs(5)).await.expect("an event")
    }

    async fn expect_none(&mut self) {
        if let Some(event) = self.next(Duration::from_millis(700)).await {
            panic!("unexpected event {event:?}");
        }
    }
}

async fn subscribe(
    app: &Arc<App>,
    token: Option<&str>,
    client_id: &str,
    topics: &[&str],
) -> StatusCode {
    let auth = token.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.iter().map(|a| ("authorization", a.as_str())).collect();
    let (status, body) = request(
        app,
        "POST",
        "/api/realtime",
        &headers,
        Some(json!({ "clientId": client_id, "subscriptions": topics })),
    )
    .await;
    assert!(
        status == StatusCode::NO_CONTENT || status.is_client_error(),
        "{status}: {body}"
    );
    status
}

async fn as_admin(
    app: &Arc<App>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Value {
    let auth = format!("Bearer {token}");
    let (status, res) = request(app, method, uri, &[("authorization", auth.as_str())], body).await;
    assert!(status.is_success(), "{method} {uri} -> {status}: {res}");
    res
}

fn topic_with_options(base: &str, options: Value) -> String {
    format!(
        "{base}?options={}",
        form_urlencoded::byte_serialize(options.to_string().as_bytes()).collect::<String>()
    )
}

#[tokio::test]
async fn delivers_changes_according_to_rules() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app(&tdb).await;
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "posts",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "public", "type": "bool" },
                { "name": "secret", "type": "text", "hidden": true }
            ],
            "listRule": "public = true",
            "viewRule": "public = true",
            "createRule": "public = true"
        })),
    )
    .await;

    let (status, _) = request(
        &app,
        "POST",
        "/api/realtime",
        &[],
        Some(json!({ "clientId": "nope", "subscriptions": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let mut all = Stream::open(&app).await;
    let mut filtered = Stream::open(&app).await;
    assert_ne!(all.client_id, filtered.client_id);
    subscribe(&app, None, &all.client_id, &["posts/*"]).await;
    let filtered_topic = topic_with_options(
        "posts/*",
        json!({ "query": { "filter": "title ~ 'news'", "fields": "id,title" } }),
    );
    subscribe(&app, None, &filtered.client_id, &[&filtered_topic]).await;

    // a public record reaches both (the filtered one only matching titles)
    let created = as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections/posts/records",
        Some(json!({ "title": "big news", "public": true, "secret": "s" })),
    )
    .await;
    let (event, data) = all.expect().await;
    assert_eq!(event, "posts/*");
    assert_eq!(data["action"], "create");
    assert_eq!(data["record"]["title"], "big news");
    assert_eq!(data["record"]["collectionName"], "posts");
    assert!(
        data["record"].get("secret").is_none(),
        "hidden fields aren't sent to guests"
    );
    let (event, data) = filtered.expect().await;
    assert_eq!(event, filtered_topic, "the event name is the subscription");
    assert_eq!(
        data["record"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["id", "title"]
    );

    // a private record reaches nobody, a non-matching one only the unfiltered client
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections/posts/records",
        Some(json!({ "title": "private news" })),
    )
    .await;
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections/posts/records",
        Some(json!({ "title": "other", "public": true })),
    )
    .await;
    let (_, data) = all.expect().await;
    assert_eq!(data["record"]["title"], "other");
    filtered.expect_none().await;

    // a rejected (rolled back) create is never announced
    let (status, _) = request(
        &app,
        "POST",
        "/api/collections/posts/records",
        &[],
        Some(json!({ "title": "x", "public": false })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    all.expect_none().await;

    // update and delete, also to a single-record topic
    let id = created["id"].as_str().unwrap();
    let mut single = Stream::open(&app).await;
    subscribe(&app, None, &single.client_id, &[&format!("posts/{id}")]).await;
    let uri = format!("/api/collections/posts/records/{id}");
    as_admin(
        &app,
        &admin,
        "PATCH",
        &uri,
        Some(json!({ "title": "updated news" })),
    )
    .await;
    let (event, data) = single.expect().await;
    assert_eq!(event, format!("posts/{id}"));
    assert_eq!(
        (data["action"].as_str(), data["record"]["title"].as_str()),
        (Some("update"), Some("updated news"))
    );
    assert_eq!(all.expect().await.1["action"], "update");

    as_admin(&app, &admin, "DELETE", &uri, None).await;
    let (_, data) = single.expect().await;
    assert_eq!(data["action"], "delete");
    assert_eq!(
        data["record"]["id"], id,
        "deleted records are checked and delivered"
    );
    assert_eq!(all.expect().await.1["action"], "delete");

    // unsubscribing (an empty list) stops delivery
    subscribe(&app, None, &all.client_id, &[]).await;
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections/posts/records",
        Some(json!({ "title": "late", "public": true })),
    )
    .await;
    all.expect_none().await;

    // disconnected clients are removed
    let core = app.core().unwrap();
    let before = core.realtime.client_count();
    drop(single);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(core.realtime.client_count(), before - 1);

    drop((all, filtered, app));
    tdb.drop_db().await;
}

#[tokio::test]
async fn auth_subscriptions() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app(&tdb).await;
    let users = as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections",
        Some(json!({ "name": "users", "type": "auth", "createRule": "", "viewRule": "id = @request.auth.id" })),
    )
    .await;
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "notes",
            "fields": [
                { "name": "text", "type": "text" },
                { "name": "owner", "type": "relation", "collectionId": users["id"] }
            ],
            "listRule": "owner = @request.auth.id"
        })),
    )
    .await;

    let mut ids = Vec::new();
    let mut tokens = Vec::new();
    for email in ["a@example.com", "b@example.com"] {
        let (_, user) = request(
            &app,
            "POST",
            "/api/collections/users/records",
            &[],
            Some(json!({ "email": email, "password": "12345678", "passwordConfirm": "12345678" })),
        )
        .await;
        ids.push(user["id"].as_str().unwrap().to_string());
        let (_, auth) = request(
            &app,
            "POST",
            "/api/collections/users/auth-with-password",
            &[],
            Some(json!({ "identity": email, "password": "12345678" })),
        )
        .await;
        tokens.push(auth["token"].as_str().unwrap().to_string());
    }

    let mut a = Stream::open(&app).await;
    assert_eq!(
        subscribe(
            &app,
            Some(&tokens[0]),
            &a.client_id,
            &["notes/*", &format!("users/{}", ids[0])]
        )
        .await,
        StatusCode::NO_CONTENT
    );
    // the auth of a client can't be switched to another user
    assert_eq!(
        subscribe(&app, Some(&tokens[1]), &a.client_id, &["notes/*"]).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        subscribe(&app, None, &a.client_id, &["notes/*"]).await,
        StatusCode::FORBIDDEN
    );

    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections/notes/records",
        Some(json!({ "text": "for b", "owner": ids[1] })),
    )
    .await;
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections/notes/records",
        Some(json!({ "text": "for a", "owner": ids[0] })),
    )
    .await;
    let (_, data) = a.expect().await;
    assert_eq!(data["record"]["text"], "for a", "only the user's own notes");

    // own auth record: email visible to itself, never the password or token key
    as_admin(
        &app,
        &admin,
        "PATCH",
        &format!("/api/collections/users/records/{}", ids[0]),
        Some(json!({ "verified": true })),
    )
    .await;
    let (event, data) = a.expect().await;
    assert_eq!(event, format!("users/{}", ids[0]));
    assert_eq!(data["record"]["email"], "a@example.com");
    assert!(data["record"].get("password").is_none() && data["record"].get("tokenKey").is_none());

    drop((a, app));
    tdb.drop_db().await;
}

#[tokio::test]
async fn instances_share_events_and_collections() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (first, admin, _dir1) = ready_app(&tdb).await;

    // a second instance on the same database
    let (_dir2, config2) = temp_config();
    let second = new_app(&config2, Some(tdb.url.clone()));
    second.bootstrap().await.unwrap();
    assert!(matches!(second.mode(), Mode::Ready { .. }));

    // a collection created through the first instance appears in the second
    as_admin(&first, &admin, "POST", "/api/collections", Some(json!({ "name": "items", "fields": [{ "name": "n", "type": "number" }], "listRule": "" }))).await;
    let core2 = second.core().unwrap();
    let mut found = false;
    for _ in 0..100 {
        if core2.registry().get("items").is_some() {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(found, "the second instance reloaded its collections");

    // a change made through the first instance reaches subscribers of the second
    let mut stream = Stream::open(&second).await;
    subscribe(&second, None, &stream.client_id, &["items/*"]).await;
    as_admin(
        &first,
        &admin,
        "POST",
        "/api/collections/items/records",
        Some(json!({ "n": 42 })),
    )
    .await;
    let (_, data) = stream.expect().await;
    assert_eq!(data["record"]["n"], 42);

    // settings too
    as_admin(
        &first,
        &admin,
        "PATCH",
        "/api/settings",
        Some(json!({ "meta": { "appName": "Shared" } })),
    )
    .await;
    let mut synced = false;
    for _ in 0..100 {
        if core2.settings().meta.app_name == "Shared" {
            synced = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(synced, "the second instance reloaded its settings");

    drop((stream, first, second, core2));
    tdb.drop_db().await;
}

/// Changes made while the listener connection is down are delivered after it reconnects.
#[tokio::test]
async fn listener_catches_up_after_reconnecting() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app(&tdb).await;
    as_admin(&app, &admin, "POST", "/api/collections", Some(json!({ "name": "items", "fields": [{ "name": "n", "type": "number" }], "listRule": "" }))).await;
    let mut stream = Stream::open(&app).await;
    subscribe(&app, None, &stream.client_id, &["items/*"]).await;

    // kill the listener's connection, then change data before it reconnects
    let killed: i64 = tdb
        .pool()
        .get()
        .await
        .unwrap()
        .query_one(
            "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity
             WHERE datname = current_database() AND query = $1",
            &[&format!("LISTEN {}", slonbase::realtime::CHANNEL)],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(killed, 1, "one listener connection");
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections/items/records",
        Some(json!({ "n": 1 })),
    )
    .await;
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections/items/records",
        Some(json!({ "n": 2 })),
    )
    .await;

    let mut received = Vec::new();
    while received.len() < 2 {
        let (_, data) = stream
            .next(Duration::from_secs(10))
            .await
            .expect("caught up events");
        received.push(data["record"]["n"].as_i64().unwrap());
    }
    received.sort();
    assert_eq!(received, [1, 2]);
    stream.expect_none().await; // and nothing twice

    drop((stream, app));
    tdb.drop_db().await;
}

#[tokio::test]
async fn delivers_changes_made_outside_the_api() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app(&tdb).await;
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "notes",
            "listRule": "",
            "viewRule": "",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "n", "type": "number" },
                { "name": "done", "type": "bool" },
                { "name": "due", "type": "date" },
                { "name": "tags", "type": "select", "values": ["a", "b"], "maxSelect": 2 },
                { "name": "meta", "type": "json" },
                { "name": "place", "type": "geoPoint" },
                { "name": "created", "type": "autodate", "onCreate": true },
            ],
        })),
    )
    .await;
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections",
        Some(json!({ "name": "people", "type": "auth", "createRule": "", "fields": [] })),
    )
    .await;
    let mut stream = Stream::open(&app).await;
    subscribe(&app, None, &stream.client_id, &["notes/*"]).await;

    // through the API: one event, the record as the API returns it
    let created = as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections/notes/records",
        Some(json!({
            "title": "api", "n": 1.5, "done": true, "due": "2026-01-02 03:04:05.678Z",
            "tags": ["a", "b"], "meta": { "k": [1, null] }, "place": { "lon": 1.25, "lat": 2 },
        })),
    )
    .await;
    let (_, data) = stream.expect().await;
    assert_eq!(data["action"], "create");
    assert_eq!(data["record"], created, "events carry the API's values");
    stream.expect_none().await;

    // with SQL, outside the API
    let pool = tdb.pool();
    let client = pool.get().await.unwrap();
    client
        .batch_execute(
            "INSERT INTO slonbase.notes (id, title, n, due) \
             VALUES ('sqlnote0000001', 'from sql', 3, '2026-05-06 07:08:09.5+02')",
        )
        .await
        .unwrap();
    let (_, data) = stream.expect().await;
    assert_eq!(data["action"], "create");
    assert_eq!(data["record"]["title"], "from sql");
    assert_eq!(data["record"]["n"], 3);
    assert_eq!(data["record"]["due"], "2026-05-06 05:08:09.500Z");
    assert_eq!(
        data["record"]["created"], "",
        "an empty date is empty, as in the API"
    );
    let (_, fetched) = request(
        &app,
        "GET",
        "/api/collections/notes/records/sqlnote0000001",
        &[],
        None,
    )
    .await;
    assert_eq!(data["record"], fetched);

    client
        .batch_execute("UPDATE slonbase.notes SET title = 'changed' WHERE id = 'sqlnote0000001'")
        .await
        .unwrap();
    let (_, data) = stream.expect().await;
    assert_eq!(
        (data["action"].as_str(), data["record"]["title"].as_str()),
        (Some("update"), Some("changed"))
    );
    client
        .batch_execute("DELETE FROM slonbase.notes WHERE id = 'sqlnote0000001'")
        .await
        .unwrap();
    let (_, data) = stream.expect().await;
    assert_eq!(
        (data["action"].as_str(), data["record"]["id"].as_str()),
        (Some("delete"), Some("sqlnote0000001"))
    );

    // a rolled-back change, and a session that turned them off, send nothing
    client
        .batch_execute(
            "BEGIN; INSERT INTO slonbase.notes (id, title) VALUES ('sqlnote0000002', 'undone'); ROLLBACK",
        )
        .await
        .unwrap();
    client
        .batch_execute(
            "SET slonbase.realtime = off; \
             INSERT INTO slonbase.notes (id, title) VALUES ('sqlnote0000003', 'quiet'); \
             RESET slonbase.realtime",
        )
        .await
        .unwrap();
    stream.expect_none().await;

    // the trigger follows schema changes: a new date field, renamed fields, a renamed table
    let notes = as_admin(&app, &admin, "GET", "/api/collections/notes", None).await;
    let mut fields = notes["fields"].as_array().unwrap().clone();
    for f in fields.iter_mut() {
        if f["name"] == "due" {
            f["name"] = json!("deadline");
        }
    }
    fields.push(json!({ "name": "remind", "type": "date" }));
    as_admin(
        &app,
        &admin,
        "PATCH",
        "/api/collections/notes",
        Some(json!({ "name": "tasks", "fields": fields })),
    )
    .await;
    subscribe(&app, None, &stream.client_id, &["tasks/*"]).await;
    client
        .batch_execute(
            "INSERT INTO slonbase.tasks (id, title, deadline, remind) \
             VALUES ('sqlnote0000004', 'renamed', '2026-01-01 00:00:00Z', '2026-02-03 04:05:06.007Z')",
        )
        .await
        .unwrap();
    let (event, data) = stream.expect().await;
    assert_eq!(event, "tasks/*");
    assert_eq!(data["record"]["collectionName"], "tasks");
    assert_eq!(data["record"]["deadline"], "2026-01-01 00:00:00.000Z");
    assert_eq!(data["record"]["remind"], "2026-02-03 04:05:06.007Z");

    // secrets never reach the outbox
    request(
        &app,
        "POST",
        "/api/collections/people/records",
        &[],
        Some(json!({ "email": "p@example.com", "password": "password-123", "passwordConfirm": "password-123" })),
    )
    .await;
    let leaked: i64 = client
        .query_one(
            "SELECT count(*) FROM _slonbase.events WHERE record ? 'password' OR record ? 'tokenKey'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(leaked, 0);
    let people: i64 = client
        .query_one(
            "SELECT count(*) FROM _slonbase.events e JOIN _slonbase.collections c ON c.id = e.collection_id \
             WHERE c.name = 'people'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(people, 1);
}

/// Subscribe requests that reach another instance than the client's stream (no client
/// affinity at the load balancer) are forwarded to the stream's instance.
#[tokio::test]
async fn subscriptions_reach_the_streams_instance() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (first, admin, _dir1) = ready_app(&tdb).await;
    as_admin(
        &first,
        &admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "items", "fields": [{ "name": "n", "type": "number" }],
            "listRule": "@request.auth.id != ''",
        })),
    )
    .await;

    // alone, an unknown client is refused at once
    let started = std::time::Instant::now();
    assert_eq!(
        subscribe(&first, None, "nope", &["items/*"]).await,
        StatusCode::NOT_FOUND
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "no other instance to wait for"
    );

    let (_dir2, config2) = temp_config();
    let second = new_app(&config2, Some(tdb.url.clone()));
    second.bootstrap().await.unwrap();
    let pool = tdb.pool();
    let client = pool.get().await.unwrap();
    for _ in 0..200 {
        let live: i64 = client
            .query_one("SELECT count(*) FROM _slonbase.instances", &[])
            .await
            .unwrap()
            .get(0);
        if live == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // the stream is on the first instance, its subscriptions are set through the second, with
    // the request's auth (checked by the first)
    let mut stream = Stream::open(&first).await;
    assert_eq!(
        subscribe(&second, Some(&admin), &stream.client_id, &["items/*"]).await,
        StatusCode::NO_CONTENT
    );
    as_admin(
        &second,
        &admin,
        "POST",
        "/api/collections/items/records",
        Some(json!({ "n": 7 })),
    )
    .await;
    let (_, data) = stream.expect().await;
    assert_eq!(data["record"]["n"], 7);

    // the owner's answers come back as they are: another auth is refused there
    let (status, res) = request(
        &second,
        "POST",
        "/api/realtime",
        &[],
        Some(json!({ "clientId": stream.client_id, "subscriptions": ["items/*"] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");

    // an unknown client, with another instance live: refused once it said so
    let started = std::time::Instant::now();
    assert_eq!(
        subscribe(&second, None, "nope", &["items/*"]).await,
        StatusCode::NOT_FOUND
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the other instance answered"
    );
    let left: i64 = client
        .query_one("SELECT count(*) FROM _slonbase.realtime_forwards", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(left, 0, "forwarded requests are removed");

    drop((stream, first, second, client, pool));
    tdb.drop_db().await;
}

/// Events are checked and rendered several at a time, but each client gets them in order.
#[tokio::test]
async fn keeps_events_in_order() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app(&tdb).await;
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "items", "listRule": "", "viewRule": "", "createRule": "",
            "updateRule": "", "deleteRule": "",
            "fields": [{ "name": "n", "type": "number" }],
        })),
    )
    .await;
    let mut stream = Stream::open(&app).await;
    let client_id = stream.client_id.clone();
    assert_eq!(
        subscribe(&app, None, &client_id, &["items/*"]).await,
        StatusCode::NO_CONTENT
    );

    let mut expected = Vec::new();
    let mut ids = Vec::new();
    for n in 0..40 {
        let (_, record) = request(
            &app,
            "POST",
            "/api/collections/items/records",
            &[],
            Some(json!({ "n": n })),
        )
        .await;
        let id = record["id"].as_str().unwrap().to_string();
        expected.push(("create".to_string(), id.clone(), n));
        ids.push(id);
    }
    for (n, id) in ids.iter().enumerate() {
        request(
            &app,
            "PATCH",
            &format!("/api/collections/items/records/{id}"),
            &[],
            Some(json!({ "n": n + 100 })),
        )
        .await;
        expected.push(("update".to_string(), id.clone(), n as i64 + 100));
    }
    for (n, id) in ids.iter().enumerate() {
        request(
            &app,
            "DELETE",
            &format!("/api/collections/items/records/{id}"),
            &[],
            None,
        )
        .await;
        expected.push(("delete".to_string(), id.clone(), n as i64 + 100));
    }
    let mut got = Vec::new();
    while got.len() < expected.len() {
        let (event, data) = stream.expect().await;
        assert_eq!(event, "items/*");
        got.push((
            data["action"].as_str().unwrap().to_string(),
            data["record"]["id"].as_str().unwrap().to_string(),
            data["record"]["n"].as_i64().unwrap(),
        ));
    }
    assert_eq!(got, expected);
    stream.expect_none().await;
    drop(stream);
    tdb.drop_db().await;
}
