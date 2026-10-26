//! JavaScript realtime, file, settings and backup hooks (`onRealtimeConnectRequest`,
//! `onFileDownloadRequest`, `onSettingsUpdateRequest`, `onBackupCreate`, ...). See
//! `setup_flow.rs` for how to point the tests at a database.
#![cfg(feature = "js")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use common::*;
use slonbase::app::App;
use slonbase::config::HooksConfig;
use slonbase::server;

const HOOKS: &str = r#"
onRealtimeConnectRequest((e) => {
  if (e.request.header.get("x-block") === "1") throw new ForbiddenError("No realtime.");
  if (!e.client.id()) throw new BadRequestError("No client id.");
  e.next();
});

onRealtimeSubscribeRequest((e) => {
  // nobody gets the secret notes
  e.subscriptions = e.subscriptions.filter((t) => !t.startsWith("secretnotes"));
  e.next();
});

onRealtimeMessageSend((e) => {
  if (e.message.name === "PB_CONNECT") return e.next();
  const data = JSON.parse(e.message.data);
  if (data.record.title === "skip") return; // not sent
  data.record.title += "!";
  data.record.client = e.client.id() === "" ? "none" : "set";
  e.message.data = JSON.stringify(data);
  e.next();
});

onFileDownloadRequest((e) => {
  if (e.servedName.startsWith("secret")) throw new ForbiddenError("No secrets.");
  if (e.record.getString("title") !== "doc") throw new BadRequestError("Wrong record.");
  e.servedName = "renamed-" + e.servedName;
  e.next();
}, "docs");

onFileTokenRequest((e) => {
  if (e.record.getString("name") === "nofiles") throw new ForbiddenError("No file tokens.");
  e.next();
}, "users");

onSettingsListRequest((e) => {
  e.settings.meta.appName = "Hooked";
  e.next();
});

onSettingsUpdateRequest((e) => {
  if (e.newSettings.meta.appName === "Bad") throw new BadRequestError("Bad name.");
  if (e.oldSettings.meta.appName === undefined) throw new BadRequestError("No old settings.");
  e.newSettings.meta.senderName = "From hook";
  e.next();
});

onSettingsReload((e) => {
  e.next();
  try {
    const log = new Record($app.findCollectionByNameOrId("log"));
    log.set("message", "reload:" + e.settings.meta.appName);
    $app.save(log);
  } catch (_) {
    // the log collection doesn't exist at startup
  }
});

onBackupCreate((e) => {
  e.name = "hooked_" + e.name;
  e.next();
});

onBackupRestore((e) => {
  throw new BadRequestError("Restores are off.");
});

routerAdd("POST", "/js/backup", (e) => {
  $app.createBackup("from_js.zip");
  return e.noContent(204);
});
"#;

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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_file_settings_and_backup_hooks() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let hooks_dir = tempfile::tempdir().unwrap();
    std::fs::write(hooks_dir.path().join("main.pb.js"), HOOKS).unwrap();
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
    let (app, admin, _dir) = ready_app_with(&tdb, ext).await;
    let bearer = format!("Bearer {admin}");
    let as_admin = [("authorization", bearer.as_str())];
    let post = |path: &'static str, body: Value| {
        let app = app.clone();
        let bearer = bearer.clone();
        async move {
            request(
                &app,
                "POST",
                path,
                &[("authorization", bearer.as_str())],
                Some(body),
            )
            .await
        }
    };

    for body in [
        json!({ "name": "notes", "listRule": "", "viewRule": "", "fields": [{ "name": "title", "type": "text" }] }),
        json!({ "name": "secretnotes", "listRule": "", "viewRule": "", "fields": [{ "name": "title", "type": "text" }] }),
        json!({ "name": "log", "listRule": "", "fields": [{ "name": "message", "type": "text" }] }),
        json!({
            "name": "docs", "viewRule": "",
            "fields": [{ "name": "title", "type": "text" }, { "name": "file", "type": "file", "maxSelect": 1 }],
        }),
        json!({
            "name": "users", "type": "auth", "createRule": "",
            "fields": [{ "name": "name", "type": "text" }],
        }),
    ] {
        let (status, res) = post("/api/collections", body).await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }

    // realtime: a refused connection, filtered subscriptions, rewritten and skipped messages
    let refused = server::router(app.clone())
        .oneshot(
            Request::get("/api/realtime")
                .header("x-block", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);

    let mut stream = Stream::open(&app).await;
    let (status, res) = request(
        &app,
        "POST",
        "/api/realtime",
        &[],
        Some(
            json!({ "clientId": stream.client_id, "subscriptions": ["notes/*", "secretnotes/*"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    for (collection, title) in [
        ("secretnotes", "hidden"),
        ("notes", "skip"),
        ("notes", "hello"),
    ] {
        let (status, res) = post(
            if collection == "notes" {
                "/api/collections/notes/records"
            } else {
                "/api/collections/secretnotes/records"
            },
            json!({ "title": title }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    let (event, data) = stream.next(Duration::from_secs(5)).await.expect("an event");
    assert_eq!(
        event, "notes/*",
        "the secret note and the skipped one aren't sent"
    );
    assert_eq!(data["record"]["title"], "hello!");
    assert_eq!(data["record"]["client"], "set");
    assert!(stream.next(Duration::from_millis(700)).await.is_none());

    // files: the download hook can refuse a file or rename it
    let mut ids = Vec::new();
    for name in ["public.txt", "secret.txt"] {
        let (content_type, body) = multipart(&[("title", "doc")], &[("file", name, b"content")]);
        let (status, _, bytes) = raw_request(
            &app,
            "POST",
            "/api/collections/docs/records",
            &[
                ("content-type", content_type.as_str()),
                ("authorization", bearer.as_str()),
            ],
            body,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let record: Value = serde_json::from_slice(&bytes).unwrap();
        ids.push((
            record["id"].as_str().unwrap().to_string(),
            record["file"].as_str().unwrap().to_string(),
        ));
    }
    let (status, headers, body) = raw_request(
        &app,
        "GET",
        &format!("/api/files/docs/{}/{}", ids[0].0, ids[0].1),
        &[],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"content");
    let disposition = headers["content-disposition"].to_str().unwrap();
    assert!(disposition.contains("renamed-public"), "{disposition}");
    let (status, _, _) = raw_request(
        &app,
        "GET",
        &format!("/api/files/docs/{}/{}", ids[1].0, ids[1].1),
        &[],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // file tokens: refused for one user, issued for another
    for (email, name) in [
        ("ann@example.com", "ann"),
        ("nofiles@example.com", "nofiles"),
    ] {
        let (status, res) = request(
            &app,
            "POST",
            "/api/collections/users/records",
            &[],
            Some(json!({ "email": email, "name": name, "password": "password-123", "passwordConfirm": "password-123" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    let token_of = |email: &'static str| {
        let app = app.clone();
        async move {
            let (_, auth) = request(
                &app,
                "POST",
                "/api/collections/users/auth-with-password",
                &[],
                Some(json!({ "identity": email, "password": "password-123" })),
            )
            .await;
            let bearer = format!("Bearer {}", auth["token"].as_str().unwrap());
            request(
                &app,
                "POST",
                "/api/files/token",
                &[("authorization", bearer.as_str())],
                None,
            )
            .await
        }
    };
    let (status, res) = token_of("ann@example.com").await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert!(res["token"].as_str().unwrap().len() > 20, "{res}");
    let (status, res) = token_of("nofiles@example.com").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");
    // superusers' tokens aren't limited by a handler for "users"
    let (status, _) = request(&app, "POST", "/api/files/token", &as_admin, None).await;
    assert_eq!(status, StatusCode::OK);

    // settings: the list hook changes the response, the update hook validates and changes
    // what's saved, and the reload hook runs after the save
    let (status, settings) = request(&app, "GET", "/api/settings", &as_admin, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["meta"]["appName"], "Hooked");
    let (status, res) = request(
        &app,
        "PATCH",
        "/api/settings",
        &as_admin,
        Some(json!({ "meta": { "appName": "Bad" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["message"], "Bad name.");
    let (status, res) = request(
        &app,
        "PATCH",
        "/api/settings",
        &as_admin,
        Some(json!({ "meta": { "appName": "Good" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["meta"]["appName"], "Good");
    assert_eq!(res["meta"]["senderName"], "From hook");
    let mut reloaded = false;
    for _ in 0..100 {
        let (_, log) = request(&app, "GET", "/api/collections/log/records", &[], None).await;
        if log["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["message"] == "reload:Good")
        {
            reloaded = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(reloaded, "onSettingsReload ran after the update");

    // backups: the create hook renames them (from the API and from JavaScript), the restore
    // hook refuses
    let (status, res) = post("/api/backups", json!({ "name": "b1.zip" })).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (status, res) = post("/js/backup", json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (_, list) = request(&app, "GET", "/api/backups", &as_admin, None).await;
    let keys: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"hooked_b1.zip"), "{keys:?}");
    assert!(keys.contains(&"hooked_from_js.zip"), "{keys:?}");
    assert!(!keys.contains(&"b1.zip"), "{keys:?}");
    let (status, res) = post("/api/backups/hooked_b1.zip/restore", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["message"], "Restores are off.");

    drop(stream);
    drop(app);
    tdb.drop_db().await;
}
