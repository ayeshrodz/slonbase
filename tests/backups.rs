//! Backups: create, list, download, upload, delete, restore (also into another
//! installation), automatic backups, and backups in S3 (when `SLONBASE_TEST_S3_ENDPOINT`
//! is set). See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

async fn admin(
    app: &Arc<App>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    request(app, method, uri, &[("authorization", auth.as_str())], body).await
}

async fn upload_record(app: &Arc<App>, token: &str, title: &str, file: &[u8]) -> Value {
    let (content_type, body) = multipart(&[("title", title)], &[("attachment", "notes.txt", file)]);
    let auth = format!("Bearer {token}");
    let (status, _, bytes) = raw_request(
        app,
        "POST",
        "/api/collections/posts/records",
        &[
            ("content-type", content_type.as_str()),
            ("authorization", auth.as_str()),
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
    serde_json::from_slice(&bytes).unwrap()
}

async fn backups(app: &Arc<App>, token: &str) -> Vec<Value> {
    let (status, list) = admin(app, token, "GET", "/api/backups", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list.as_array().unwrap().clone()
}

async fn download(app: &Arc<App>, token: &str, key: &str) -> Vec<u8> {
    let (_, file_token) = admin(app, token, "POST", "/api/files/token", None).await;
    let uri = format!(
        "/api/backups/{key}?token={}",
        file_token["token"].as_str().unwrap()
    );
    let (status, headers, body) = raw_request(app, "GET", &uri, &[], vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/zip");
    body
}

async fn upload_backup(
    app: &Arc<App>,
    token: &str,
    name: &str,
    data: &[u8],
) -> (StatusCode, Value) {
    let (content_type, body) = multipart(&[], &[("file", name, data)]);
    let auth = format!("Bearer {token}");
    let (status, _, bytes) = raw_request(
        app,
        "POST",
        "/api/backups/upload",
        &[
            ("content-type", content_type.as_str()),
            ("authorization", auth.as_str()),
        ],
        body,
    )
    .await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn backup_and_restore() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, dir) = ready_app(&tdb).await;

    // some data: a collection with files, ordered records, settings
    let (status, _) = admin(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "posts", "listRule": "", "viewRule": "",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "meta", "type": "json" },
                { "name": "attachment", "type": "file" },
            ],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let first = upload_record(&app, &token, "first, with \"quotes\"\tand a tab", b"hello").await;
    let second = upload_record(&app, &token, "second\nline", b"world").await;
    admin(
        &app,
        &token,
        "PATCH",
        &format!(
            "/api/collections/posts/records/{}",
            first["id"].as_str().unwrap()
        ),
        Some(json!({ "meta": { "a": [1, null, "\\\\x"] } })),
    )
    .await;
    admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "meta": { "appName": "Before" } })),
    )
    .await;

    // superusers only
    let (status, _) = request(&app, "GET", "/api/backups", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/backups",
        Some(json!({ "name": "Bad Name.zip" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");

    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/backups",
        Some(json!({ "name": "before.zip" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/backups",
        Some(json!({ "name": "before.zip" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    let (status, _) = admin(&app, &token, "POST", "/api/backups", Some(json!({}))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let list = backups(&app, &token).await;
    assert_eq!(list.len(), 2);
    let named = list.iter().find(|b| b["key"] == "before.zip").unwrap();
    assert!(named["size"].as_u64().unwrap() > 100);
    assert!(named["modified"].as_str().unwrap().ends_with('Z'));
    assert!(
        list.iter()
            .any(|b| b["key"].as_str().unwrap().starts_with("slonbase_backup_"))
    );
    assert!(dir.path().join("backups/before.zip").exists());

    // changes after the backup
    admin(
        &app,
        &token,
        "DELETE",
        &format!(
            "/api/collections/posts/records/{}",
            second["id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    upload_record(&app, &token, "third", b"!").await;
    admin(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "extra" })),
    )
    .await;
    admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "meta": { "appName": "After" } })),
    )
    .await;

    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/backups/before.zip/restore",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");

    // the same installation: the token still works
    let (_, posts) = admin(
        &app,
        &token,
        "GET",
        "/api/collections/posts/records?sort=@rowid",
        None,
    )
    .await;
    let titles: Vec<&str> = posts["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        vec!["first, with \"quotes\"\tand a tab", "second\nline"],
        "in the original order"
    );
    assert_eq!(
        posts["items"][0]["meta"],
        json!({ "a": [1, null, "\\\\x"] })
    );
    let (status, _) = admin(&app, &token, "GET", "/api/collections/extra", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, settings) = admin(&app, &token, "GET", "/api/settings", None).await;
    assert_eq!(settings["meta"]["appName"], "Before");
    let file_uri = format!(
        "/api/files/posts/{}/{}",
        second["id"].as_str().unwrap(),
        second["attachment"].as_str().unwrap()
    );
    let (status, _, body) = raw_request(&app, "GET", &file_uri, &[], vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"world");
    // new records continue the order
    upload_record(&app, &token, "after restore", b"x").await;
    let (_, posts) = admin(
        &app,
        &token,
        "GET",
        "/api/collections/posts/records?sort=-@rowid",
        None,
    )
    .await;
    assert_eq!(posts["items"][0]["title"], "after restore");

    // download needs a superuser file token
    let (status, _, _) = raw_request(&app, "GET", "/api/backups/before.zip", &[], vec![]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let archive = download(&app, &token, "before.zip").await;
    assert!(archive.starts_with(b"PK"));

    // upload: only backups
    let (status, _) = upload_backup(&app, &token, "copy.zip", &archive).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, res) = upload_backup(&app, &token, "copy.zip", &archive).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    let (status, res) = upload_backup(&app, &token, "junk.zip", b"not a zip").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["message"], "The file isn't a slonbase backup.");
    let (status, _) = upload_backup(&app, &token, "Bad.zip", &archive).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(backups(&app, &token).await.len(), 3);

    let (status, _) = admin(&app, &token, "DELETE", "/api/backups/copy.zip", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = admin(&app, &token, "DELETE", "/api/backups/copy.zip", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = admin(
        &app,
        &token,
        "POST",
        "/api/backups/missing.zip/restore",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // into another installation: its data and auth secret are replaced
    let Some(other_db) = TestDb::create().await else {
        return;
    };
    let (other, other_token, _other_dir) = ready_app(&other_db).await;
    let (status, res) = upload_backup(&other, &other_token, "imported.zip", &archive).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (status, res) = admin(
        &other,
        &other_token,
        "POST",
        "/api/backups/imported.zip/restore",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (status, _) = admin(&other, &other_token, "GET", "/api/collections", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "tokens of the replaced installation stop working"
    );
    let (status, login) = request(
        &other,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        &[],
        Some(json!({ "identity": ADMIN_EMAIL, "password": ADMIN_PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let restored_token = login["token"].as_str().unwrap();
    let (_, posts) = admin(
        &other,
        restored_token,
        "GET",
        "/api/collections/posts/records",
        None,
    )
    .await;
    assert_eq!(posts["totalItems"], 2);
    let (status, _, body) = raw_request(&other, "GET", &file_uri, &[], vec![]).await;
    assert_eq!(status, StatusCode::OK, "files were restored");
    assert_eq!(body, b"world");

    drop((app, other));
    tdb.drop_db().await;
    other_db.drop_db().await;
}

#[tokio::test]
async fn automatic_backups() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "backups": { "cron": "nope" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    let (_, crons) = admin(&app, &token, "GET", "/api/crons", None).await;
    assert_eq!(crons, json!([]));

    admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "backups": { "cron": "0 3 * * *", "cronMaxKeep": 2 } })),
    )
    .await;
    let (_, crons) = admin(&app, &token, "GET", "/api/crons", None).await;
    assert_eq!(
        crons,
        json!([{ "id": "__pbAutoBackup__", "expression": "0 3 * * *" }])
    );
    for _ in 0..3 {
        let (status, res) = admin(&app, &token, "POST", "/api/crons/__pbAutoBackup__", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
        // names carry the time to the second
        tokio::time::sleep(Duration::from_millis(1100)).await;
    }
    let list = backups(&app, &token).await;
    assert_eq!(
        list.len(),
        2,
        "only cronMaxKeep automatic backups are kept: {list:?}"
    );
    assert!(
        list.iter()
            .all(|b| b["key"].as_str().unwrap().starts_with("auto_backup_"))
    );
    let (status, _) = admin(&app, &token, "POST", "/api/crons/unknown", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn backups_in_s3() {
    let Ok(endpoint) = std::env::var("SLONBASE_TEST_S3_ENDPOINT") else {
        return;
    };
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, dir) = ready_app(&tdb).await;
    let bucket = format!("slonbase-backups-{}", slonbase::ids::random_id(8));
    let config = slonbase::s3::Config {
        endpoint: endpoint.clone(),
        bucket: bucket.clone(),
        region: "us-east-1".into(),
        access_key: std::env::var("SLONBASE_TEST_S3_ACCESS_KEY").unwrap_or_default(),
        secret: std::env::var("SLONBASE_TEST_S3_SECRET").unwrap_or_default(),
        force_path_style: true,
    };
    let client = slonbase::s3::Client::new(config.clone()).unwrap();
    client.create_bucket().await.unwrap();
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "backups": { "s3": {
        "enabled": true, "bucket": bucket, "region": "us-east-1", "endpoint": endpoint,
        "accessKey": config.access_key, "secret": config.secret, "forcePathStyle": true,
    } } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/settings/test/s3",
        Some(json!({ "filesystem": "backups" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");

    admin(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "notes", "fields": [{ "name": "text", "type": "text" }] })),
    )
    .await;
    admin(
        &app,
        &token,
        "POST",
        "/api/collections/notes/records",
        Some(json!({ "text": "kept" })),
    )
    .await;
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/backups",
        Some(json!({ "name": "in-s3.zip" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    assert_eq!(client.list("").await.unwrap(), vec!["in-s3.zip"]);
    assert!(!dir.path().join("backups/in-s3.zip").exists());
    let list = backups(&app, &token).await;
    assert_eq!(list[0]["key"], "in-s3.zip");
    assert!(list[0]["size"].as_u64().unwrap() > 100);

    admin(
        &app,
        &token,
        "POST",
        "/api/collections/notes/records",
        Some(json!({ "text": "lost" })),
    )
    .await;
    let (status, res) = admin(&app, &token, "POST", "/api/backups/in-s3.zip/restore", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (_, notes) = admin(&app, &token, "GET", "/api/collections/notes/records", None).await;
    assert_eq!(notes["totalItems"], 1);
    let archive = download(&app, &token, "in-s3.zip").await;
    assert!(archive.starts_with(b"PK"));

    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn encrypted_backups() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let key = "a long test passphrase";
    let (app, token, dir) = ready_app_configured(&tdb, Default::default(), |o| {
        o.backups_encryption_key = Some(key.into())
    })
    .await;
    let (status, _) = admin(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "notes", "fields": [{ "name": "text", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, note) = admin(
        &app,
        &token,
        "POST",
        "/api/collections/notes/records",
        Some(json!({ "text": "a secret note" })),
    )
    .await;
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/backups",
        Some(json!({ "name": "sealed.zip" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");

    // stored, and downloaded, encrypted
    let stored = std::fs::read(dir.path().join("backups/sealed.zip")).unwrap();
    assert!(stored.starts_with(slonbase::backup_crypt::MAGIC));
    let archive = download(&app, &token, "sealed.zip").await;
    assert_eq!(archive, stored);

    // restored with the key
    admin(
        &app,
        &token,
        "DELETE",
        &format!(
            "/api/collections/notes/records/{}",
            note["id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/backups/sealed.zip/restore",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (_, list) = admin(&app, &token, "GET", "/api/collections/notes/records", None).await;
    assert_eq!(list["items"][0]["text"], "a secret note");

    // another installation needs the same key
    let Some(other_db) = TestDb::create().await else {
        return;
    };
    let (other, other_token, _other_dir) = ready_app(&other_db).await;
    let (status, res) = upload_backup(&other, &other_token, "sealed.zip", &archive).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        res["message"].as_str().unwrap().contains("encrypted"),
        "{res}"
    );
    let Some(third_db) = TestDb::create().await else {
        return;
    };
    let (third, third_token, _third_dir) =
        ready_app_configured(&third_db, Default::default(), |o| {
            o.backups_encryption_key = Some("a different passphrase".into())
        })
        .await;
    let (status, res) = upload_backup(&third, &third_token, "sealed.zip", &archive).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        res["message"]
            .as_str()
            .unwrap()
            .contains("couldn't be decrypted"),
        "{res}"
    );

    // a plain backup uploaded where there's a key is stored encrypted
    let (status, _) = admin(
        &other,
        &other_token,
        "POST",
        "/api/backups",
        Some(json!({ "name": "plain.zip" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let plain = download(&other, &other_token, "plain.zip").await;
    assert!(plain.starts_with(b"PK"));
    let (status, res) = upload_backup(&app, &token, "plain.zip", &plain).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let stored = std::fs::read(dir.path().join("backups/plain.zip")).unwrap();
    assert!(stored.starts_with(slonbase::backup_crypt::MAGIC));

    // a short key is refused at startup
    let (_config, path) = temp_config();
    let overrides = slonbase::config::Overrides {
        backups_encryption_key: Some("short".into()),
        ..Default::default()
    };
    assert!(slonbase::config::load(&path, overrides).is_err());

    drop((app, other, third));
    tdb.drop_db().await;
    other_db.drop_db().await;
    third_db.drop_db().await;
}
