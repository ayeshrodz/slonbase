//! File storage in S3 end to end. Needs PostgreSQL (see `setup_flow.rs`) and an
//! S3-compatible server: `SLONBASE_TEST_S3_ENDPOINT` (path-style, e.g. MinIO at
//! `http://127.0.0.1:9000`), `SLONBASE_TEST_S3_ACCESS_KEY` and `SLONBASE_TEST_S3_SECRET`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;
use slonbase::s3;

fn s3_config(bucket: &str) -> Option<s3::Config> {
    Some(s3::Config {
        endpoint: std::env::var("SLONBASE_TEST_S3_ENDPOINT").ok()?,
        bucket: bucket.into(),
        region: std::env::var("SLONBASE_TEST_S3_REGION").unwrap_or_else(|_| "us-east-1".into()),
        access_key: std::env::var("SLONBASE_TEST_S3_ACCESS_KEY").unwrap_or_default(),
        secret: std::env::var("SLONBASE_TEST_S3_SECRET").unwrap_or_default(),
        force_path_style: true,
    })
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(width, height, image::Rgb([10, 120, 200]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

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

async fn upload(
    app: &Arc<App>,
    token: &str,
    method: &str,
    uri: &str,
    files: &[(&str, &str, &[u8])],
) -> (StatusCode, Value) {
    let (content_type, body) = multipart(&[("title", "x")], files);
    let auth = format!("Bearer {token}");
    let (status, _, bytes) = raw_request(
        app,
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

async fn sorted(client: &s3::Client, prefix: &str) -> Vec<String> {
    let mut keys = client.list(prefix).await.unwrap();
    keys.sort();
    keys
}

#[tokio::test]
async fn files_in_s3() {
    let bucket = format!("slonbase-test-{}", slonbase::ids::random_id(8));
    let Some(config) = s3_config(&bucket) else {
        eprintln!("skipping: SLONBASE_TEST_S3_ENDPOINT is not set");
        return;
    };
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let bucket_client = s3::Client::new(config.clone()).unwrap();
    bucket_client.create_bucket().await.unwrap();

    let (app, token, dir) = ready_app(&tdb).await;
    // a second instance on the same database
    let (_dir2, config2) = temp_config();
    let second = new_app(&config2, Some(tdb.url.clone()));
    second.bootstrap().await.unwrap();

    // disabled by default
    let (_, settings) = admin(&app, &token, "GET", "/api/settings", None).await;
    assert_eq!(settings["s3"]["enabled"], false);
    let (status, _) = admin(
        &app,
        &token,
        "POST",
        "/api/settings/test/s3",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // validation
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "s3": { "enabled": true, "endpoint": "ftp://nope" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    for key in [
        "/data/s3/bucket",
        "/data/s3/region",
        "/data/s3/accessKey",
        "/data/s3/secret",
    ] {
        assert!(res.pointer(key).is_some(), "{key}: {res}");
    }

    let s3_settings = json!({
        "enabled": true,
        "bucket": config.bucket,
        "region": config.region,
        "endpoint": config.endpoint,
        "accessKey": config.access_key,
        "secret": config.secret,
        "forcePathStyle": true,
    });
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "s3": s3_settings })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["s3"]["secret"], "", "the secret is never returned");
    assert_eq!(res["s3"]["accessKey"], config.access_key.as_str());
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/settings/test/s3",
        Some(json!({ "filesystem": "storage" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    assert!(
        sorted(&bucket_client, "").await.is_empty(),
        "the test file is removed"
    );

    // uploads go to the bucket
    admin(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "docs",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "image", "type": "file", "maxSelect": 1, "thumbs": ["50x50"] }
            ],
            "listRule": "", "viewRule": ""
        })),
    )
    .await;
    let (_, collection) = admin(&app, &token, "GET", "/api/collections/docs", None).await;
    let cid = collection["id"].as_str().unwrap().to_string();
    let original = png(300, 200);
    let (status, record) = upload(
        &app,
        &token,
        "POST",
        "/api/collections/docs/records",
        &[("image", "photo.png", &original)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{record}");
    let rid = record["id"].as_str().unwrap().to_string();
    let name = record["image"].as_str().unwrap().to_string();
    assert_eq!(
        sorted(&bucket_client, &format!("{cid}/")).await,
        vec![format!("{cid}/{rid}/{name}")]
    );
    assert!(
        !dir.path().join("storage").join(&cid).exists(),
        "nothing is written to the local directory"
    );

    // served by the other instance once it reloads the settings
    let uri = format!("/api/files/{cid}/{rid}/{name}");
    let mut served = None;
    for _ in 0..100 {
        let (status, headers, body) = raw_request(&second, "GET", &uri, &[], vec![]).await;
        if status == StatusCode::OK {
            served = Some((headers, body));
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let (headers, body) = served.expect("the second instance serves the file");
    assert_eq!(body, original);
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(
        headers["content-length"],
        original.len().to_string().as_str()
    );

    // a byte range from S3 (the type still comes from the file's start)
    let (status, headers, body) =
        raw_request(&second, "GET", &uri, &[("range", "bytes=10-29")], vec![]).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, original[10..30]);
    assert_eq!(
        headers["content-range"],
        format!("bytes 10-29/{}", original.len()).as_str()
    );
    assert_eq!(headers["content-type"], "image/png");

    // thumbnails are generated once and stored next to the file
    for _ in 0..2 {
        let (status, headers, body) =
            raw_request(&app, "GET", &format!("{uri}?thumb=50x50"), &[], vec![]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["content-type"], "image/png");
        let thumb = image::load_from_memory(&body).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (50, 50));
    }
    let keys = sorted(&bucket_client, &format!("{cid}/{rid}/")).await;
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert!(keys.iter().any(|k| k.contains("/thumbs_")), "{keys:?}");
    let (status, _, _) = raw_request(
        &app,
        "GET",
        &format!("/api/files/{cid}/{rid}/missing.png"),
        &[],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // replacing the file deletes the old one and its thumbnails
    let (status, updated) = upload(
        &app,
        &token,
        "PATCH",
        &format!("/api/collections/docs/records/{rid}"),
        &[("image", "other.png", &png(20, 20))],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let new_name = updated["image"].as_str().unwrap();
    assert_eq!(
        sorted(&bucket_client, &format!("{cid}/{rid}/")).await,
        vec![format!("{cid}/{rid}/{new_name}")]
    );

    // an empty secret keeps the stored one
    let (status, _) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "s3": { "secret": "" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/settings/test/s3",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");

    // wrong credentials: the test fails, and uploads fail without saving a record
    admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "s3": { "secret": "wrong-secret" } })),
    )
    .await;
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/settings/test/s3",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        res["message"]
            .as_str()
            .unwrap()
            .starts_with("Failed to initialize the S3 storage."),
        "{res}"
    );
    let (status, _) = upload(
        &app,
        &token,
        "POST",
        "/api/collections/docs/records",
        &[("image", "photo.png", &original)],
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let (_, list) = admin(&app, &token, "GET", "/api/collections/docs/records", None).await;
    assert_eq!(list["totalItems"], 1);
    admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "s3": { "secret": config.secret } })),
    )
    .await;

    // deleting the record, then the collection, removes their files
    let (status, second_record) = upload(
        &app,
        &token,
        "POST",
        "/api/collections/docs/records",
        &[("image", "photo.png", &original)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second_record}");
    let (status, _) = admin(
        &app,
        &token,
        "DELETE",
        &format!("/api/collections/docs/records/{rid}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        sorted(&bucket_client, &format!("{cid}/{rid}/"))
            .await
            .is_empty()
    );
    assert_eq!(sorted(&bucket_client, &format!("{cid}/")).await.len(), 1);
    let (status, _) = admin(&app, &token, "DELETE", "/api/collections/docs", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(sorted(&bucket_client, &format!("{cid}/")).await.is_empty());

    // switching back to local storage
    admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "s3": { "enabled": false } })),
    )
    .await;
    admin(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "local_docs", "fields": [
            { "name": "title", "type": "text" },
            { "name": "image", "type": "file" }
        ] })),
    )
    .await;
    let (status, record) = upload(
        &app,
        &token,
        "POST",
        "/api/collections/local_docs/records",
        &[("image", "photo.png", &original)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{record}");
    assert!(sorted(&bucket_client, "").await.is_empty());
    let local = dir
        .path()
        .join("storage")
        .join(record["collectionId"].as_str().unwrap())
        .join(record["id"].as_str().unwrap())
        .join(record["image"].as_str().unwrap());
    assert!(local.exists(), "{}", local.display());

    drop((app, second));
    tdb.drop_db().await;
}
