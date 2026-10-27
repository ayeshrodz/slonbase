//! File fields end to end: multipart uploads, serving, thumbnails, protected files and
//! cleanup. See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

fn png(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(width, height, image::Rgb([200, 30, 30]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

struct Env {
    tdb: TestDb,
    app: Arc<App>,
    admin: String,
    dir: tempfile::TempDir,
}

impl Env {
    fn auth(&self) -> String {
        format!("Bearer {}", self.admin)
    }

    async fn upload(
        &self,
        method: &str,
        uri: &str,
        fields: &[(&str, &str)],
        files: &[(&str, &str, &[u8])],
    ) -> (StatusCode, Value) {
        let (content_type, body) = multipart(fields, files);
        let auth = self.auth();
        let (status, _, bytes) = raw_request(
            &self.app,
            method,
            uri,
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

    async fn get(&self, uri: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        raw_request(&self.app, "GET", uri, &[], vec![]).await
    }

    fn stored_files(&self) -> Vec<String> {
        let root = self.dir.path().join("storage");
        let mut out = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(
                        path.strip_prefix(&root)
                            .unwrap()
                            .to_string_lossy()
                            .to_string(),
                    );
                }
            }
        }
        out.sort();
        out
    }
}

async fn setup() -> Option<Env> {
    let tdb = TestDb::create().await?;
    let (app, admin, dir) = ready_app(&tdb).await;
    let env = Env {
        tdb,
        app,
        admin,
        dir,
    };
    let auth = env.auth();
    let (status, body) = request(
        &env.app,
        "POST",
        "/api/collections",
        &[("authorization", auth.as_str())],
        Some(json!({
            "name": "docs",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "meta", "type": "json" },
                { "name": "avatar", "type": "file", "maxSize": 4096, "mimeTypes": ["image/png"], "thumbs": ["50x50", "0x20"] },
                { "name": "files", "type": "file", "maxSelect": 3 },
                { "name": "secret", "type": "file", "protected": true }
            ],
            "listRule": "", "viewRule": "@request.auth.id != '' || title = 'public'",
            "createRule": "", "updateRule": "", "deleteRule": ""
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    Some(env)
}

#[tokio::test]
async fn uploads_serving_and_thumbnails() {
    let Some(env) = setup().await else { return };
    let image = png(120, 60);

    let (status, record) = env
        .upload(
            "POST",
            "/api/collections/docs/records",
            &[("title", "public"), ("meta", r#"{"a": 1}"#)],
            &[
                ("avatar", "My Avatar.PNG", &image),
                ("files", "one.txt", b"first"),
                ("files", "two.txt", b"second"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{record}");
    assert_eq!(record["title"], "public");
    assert_eq!(
        record["meta"],
        json!({ "a": 1 }),
        "JSON fields are parsed from form values"
    );
    let avatar = record["avatar"].as_str().unwrap().to_string();
    assert!(
        avatar.starts_with("my_avatar_") && avatar.ends_with(".png"),
        "{avatar}"
    );
    assert_eq!(record["files"].as_array().unwrap().len(), 2);
    let id = record["id"].as_str().unwrap().to_string();
    let base = format!("/api/files/docs/{id}");

    // the original, inline, with protective headers
    let (status, headers, bytes) = env.get(&format!("{base}/{avatar}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, image);
    assert_eq!(headers["content-type"], "image/png");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("inline")
    );
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    assert_eq!(headers["x-content-type-options"], "nosniff");
    let (_, headers, _) = env.get(&format!("{base}/{avatar}?download=1")).await;
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
    let text = record["files"][0].as_str().unwrap();
    let (_, headers, bytes) = env.get(&format!("{base}/{text}")).await;
    assert_eq!(bytes, b"first");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment"),
        "text isn't inline"
    );

    // thumbnails: allowed sizes are generated once and cached
    for (thumb, expected) in [
        ("50x50", (50, 50)),
        ("0x20", (40, 20)),
        ("100x100", (100, 100)),
    ] {
        let (status, _, bytes) = env.get(&format!("{base}/{avatar}?thumb={thumb}")).await;
        assert_eq!(status, StatusCode::OK);
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), expected, "{thumb}");
    }
    assert!(
        env.stored_files()
            .iter()
            .any(|f| f.contains("thumbs_") && f.ends_with(&format!("50x50_{avatar}")))
    );
    let (_, _, bytes) = env.get(&format!("{base}/{avatar}?thumb=10x10")).await;
    assert_eq!(
        bytes, image,
        "sizes that aren't configured serve the original"
    );

    // unknown files and traversal attempts
    for bad in [
        format!("{base}/nope.png"),
        format!("{base}/..%2F..%2Fetc%2Fpasswd"),
        format!("/api/files/docs/missing/{avatar}"),
        format!("/api/files/nope/{id}/{avatar}"),
    ] {
        let (status, _, _) = env.get(&bad).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{bad}");
    }

    env.tdb.drop_db().await;
}

#[tokio::test]
async fn validation_updates_and_cleanup() {
    let Some(env) = setup().await else { return };
    let image = png(10, 10);

    // invalid uploads leave nothing behind
    for files in [
        vec![(
            "avatar",
            "big.png",
            [b"\x89PNG\r\n\x1a\n".as_slice(), &[0u8; 5000]].concat(),
        )],
        vec![("avatar", "fake.png", b"not an image".to_vec())],
        vec![
            ("files", "a.txt", b"a".to_vec()),
            ("files", "b.txt", b"b".to_vec()),
            ("files", "c.txt", b"c".to_vec()),
            ("files", "d.txt", b"d".to_vec()),
        ],
    ] {
        let refs: Vec<(&str, &str, &[u8])> = files
            .iter()
            .map(|(k, n, d)| (*k, *n, d.as_slice()))
            .collect();
        let (status, body) = env
            .upload("POST", "/api/collections/docs/records", &[], &refs)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["data"]["avatar"].is_object() || body["data"]["files"].is_object(),
            "{body}"
        );
    }
    assert!(env.stored_files().is_empty(), "{:?}", env.stored_files());

    let (_, record) = env
        .upload(
            "POST",
            "/api/collections/docs/records",
            &[],
            &[("avatar", "a.png", &image), ("files", "1.txt", b"1")],
        )
        .await;
    let id = record["id"].as_str().unwrap().to_string();
    let uri = format!("/api/collections/docs/records/{id}");
    let first_avatar = record["avatar"].as_str().unwrap().to_string();
    let one = record["files"][0].as_str().unwrap().to_string();

    // append, then remove by name, then replace the single file
    let (status, record) = env
        .upload(
            "PATCH",
            &uri,
            &[],
            &[("files+", "2.txt", b"2"), ("files+", "3.txt", b"3")],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{record}");
    assert_eq!(record["files"].as_array().unwrap().len(), 3);
    assert_eq!(record["files"][0], one.as_str());

    let auth = env.auth();
    let (status, record) = request(
        &env.app,
        "PATCH",
        &uri,
        &[("authorization", auth.as_str())],
        Some(json!({ "files-": [one] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{record}");
    assert_eq!(record["files"].as_array().unwrap().len(), 2);
    let (status, _, _) = env.get(&format!("/api/files/docs/{id}/{one}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "removed files are deleted");

    // a thumbnail of the old avatar is deleted with it
    env.get(&format!("/api/files/docs/{id}/{first_avatar}?thumb=50x50"))
        .await;
    let (_, record) = env
        .upload("PATCH", &uri, &[], &[("avatar", "b.png", &image)])
        .await;
    assert_ne!(record["avatar"], first_avatar.as_str());
    assert!(
        !env.stored_files().iter().any(|f| f.contains(&first_avatar)),
        "{:?}",
        env.stored_files()
    );

    // a failed update keeps the old files and adds none
    let before = env.stored_files();
    let (status, _) = env
        .upload("PATCH", &uri, &[], &[("avatar", "x.txt", b"text")])
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(env.stored_files(), before);

    // clearing with JSON
    let (_, record) = request(
        &env.app,
        "PATCH",
        &uri,
        &[("authorization", auth.as_str())],
        Some(json!({ "avatar": "" })),
    )
    .await;
    assert_eq!(record["avatar"], "");

    // deleting the record deletes its files
    let (status, _) = request(
        &env.app,
        "DELETE",
        &uri,
        &[("authorization", auth.as_str())],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        !env.stored_files().iter().any(|f| f.contains(&id)),
        "{:?}",
        env.stored_files()
    );

    env.tdb.drop_db().await;
}

#[tokio::test]
async fn protected_files() {
    let Some(env) = setup().await else { return };
    let (_, record) = env
        .upload(
            "POST",
            "/api/collections/docs/records",
            &[("title", "private")],
            &[("secret", "s.txt", b"top secret")],
        )
        .await;
    let id = record["id"].as_str().unwrap();
    let file = record["secret"].as_str().unwrap();
    let uri = format!("/api/files/docs/{id}/{file}");

    // the view rule requires auth: no token, a bad token -> not found
    assert_eq!(env.get(&uri).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        env.get(&format!("{uri}?token=garbage")).await.0,
        StatusCode::NOT_FOUND
    );
    // an auth token isn't a file token
    assert_eq!(
        env.get(&format!("{uri}?token={}", env.admin)).await.0,
        StatusCode::NOT_FOUND
    );

    let (status, _) = request(&env.app, "POST", "/api/files/token", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let auth = env.auth();
    let (_, token) = request(
        &env.app,
        "POST",
        "/api/files/token",
        &[("authorization", auth.as_str())],
        None,
    )
    .await;
    let token = token["token"].as_str().unwrap();
    let (status, headers, bytes) = env.get(&format!("{uri}?token={token}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"top secret");
    assert_eq!(headers["cache-control"], "private, no-store");

    env.tdb.drop_db().await;
}

#[tokio::test]
async fn byte_ranges() {
    let Some(env) = setup().await else { return };
    let content: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    let (status, record) = env
        .upload(
            "POST",
            "/api/collections/docs/records",
            &[("title", "ranges")],
            &[("files", "data.bin", &content)],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{record}");
    let id = record["id"].as_str().unwrap();
    let name = record["files"][0].as_str().unwrap();
    let uri = format!("/api/files/docs/{id}/{name}");
    let get = |range: &'static str| {
        let uri = uri.clone();
        let env = &env;
        async move { raw_request(&env.app, "GET", &uri, &[("range", range)], vec![]).await }
    };

    // the whole file says that ranges are accepted
    let (status, headers, bytes) = env.get(&uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["accept-ranges"], "bytes");
    assert_eq!(bytes, content);

    for (range, start, end) in [
        ("bytes=0-99", 0, 99),
        ("bytes=10000-10009", 10_000, 10_009),
        ("bytes=19990-", 19_990, 19_999),
        ("bytes=-5", 19_995, 19_999),
        ("bytes=19000-999999", 19_000, 19_999),
    ] {
        let (status, headers, bytes) = get(range).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT, "{range}");
        assert_eq!(
            headers["content-range"],
            format!("bytes {start}-{end}/20000").as_str(),
            "{range}"
        );
        assert_eq!(
            headers["content-length"],
            (end - start + 1).to_string().as_str()
        );
        assert_eq!(bytes, content[start..=end], "{range}");
        // the type is the file's, wherever the range starts
        assert_eq!(
            headers["content-type"], "application/octet-stream",
            "{range}"
        );
    }

    let (status, headers, _) = get("bytes=20000-").await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(headers["content-range"], "bytes */20000");

    // several ranges, or a malformed header: the whole file
    for range in ["bytes=0-1,5-6", "bytes=abc", "items=0-1"] {
        let (status, _, bytes) = get(range).await;
        assert_eq!(status, StatusCode::OK, "{range}");
        assert_eq!(bytes.len(), 20_000, "{range}");
    }
    env.tdb.drop_db().await;
}

#[tokio::test]
async fn removed_file_fields() {
    let Some(env) = setup().await else { return };
    let image = png(10, 10);
    let auth = env.auth();
    let headers = [("authorization", auth.as_str())];
    let mut names: Vec<(String, String)> = Vec::new(); // (field, file name)
    for _ in 0..2 {
        let (status, record) = env
            .upload(
                "POST",
                "/api/collections/docs/records",
                &[],
                &[
                    ("avatar", "a.png", &image),
                    ("files", "1.txt", b"1"),
                    ("files", "2.txt", b"2"),
                    ("secret", "s.txt", b"s"),
                ],
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{record}");
        let id = record["id"].as_str().unwrap();
        let avatar = record["avatar"].as_str().unwrap();
        env.get(&format!("/api/files/docs/{id}/{avatar}?thumb=50x50"))
            .await;
        names.push(("avatar".into(), avatar.into()));
        names.push(("secret".into(), record["secret"].as_str().unwrap().into()));
        for f in record["files"].as_array().unwrap() {
            names.push(("files".into(), f.as_str().unwrap().into()));
        }
    }
    let stored = |field: &str| -> usize {
        let files = env.stored_files();
        names
            .iter()
            .filter(|(f, _)| f == field)
            .filter(|(_, name)| files.iter().any(|stored| stored.contains(name.as_str())))
            .count()
    };
    assert_eq!(
        (stored("avatar"), stored("files"), stored("secret")),
        (2, 4, 2)
    );
    assert_eq!(
        env.stored_files()
            .iter()
            .filter(|f| f.contains("thumbs_"))
            .count(),
        2
    );
    let without = |collection: &Value, field: &str| -> Value {
        let fields: Vec<Value> = collection["fields"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["name"] != field)
            .cloned()
            .collect();
        Value::Array(fields)
    };

    // a failed update deletes nothing
    let (_, collection) = request(&env.app, "GET", "/api/collections/docs", &headers, None).await;
    let mut fields = without(&collection, "files");
    fields
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "bad", "type": "nope" }));
    let (status, _) = request(
        &env.app,
        "PATCH",
        "/api/collections/docs",
        &headers,
        Some(json!({ "fields": fields })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(stored("files"), 4);

    // removing a field deletes its files, and only its files
    let (status, body) = request(
        &env.app,
        "PATCH",
        "/api/collections/docs",
        &headers,
        Some(json!({ "fields": without(&collection, "files") })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (stored("avatar"), stored("files"), stored("secret")),
        (2, 0, 2)
    );

    // renaming one keeps them
    let mut fields = without(&body, "");
    for f in fields.as_array_mut().unwrap() {
        if f["name"] == "secret" {
            f["name"] = json!("private");
        }
    }
    let (status, body) = request(
        &env.app,
        "PATCH",
        "/api/collections/docs",
        &headers,
        Some(json!({ "fields": fields })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(stored("secret"), 2);

    // through an import too, thumbnails included
    let mut definition = body.clone();
    definition["fields"] = without(&body, "avatar");
    let (status, res) = request(
        &env.app,
        "PUT",
        "/api/collections/import",
        &headers,
        Some(json!({ "collections": [definition] })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    assert_eq!((stored("avatar"), stored("secret")), (0, 2));
    assert!(
        !env.stored_files().iter().any(|f| f.contains("thumbs_")),
        "{:?}",
        env.stored_files()
    );
    drop(env.app);
    env.tdb.drop_db().await;
}
