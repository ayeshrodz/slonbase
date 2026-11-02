//! JSON answers are compressed for clients that take gzip or brotli, except small ones and
//! those carrying tokens or secrets (BREACH). Needs PostgreSQL.

mod common;

use std::io::Read;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;

#[tokio::test]
async fn compressed_json_answers() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = format!("Bearer {token}");
    let admin = [("authorization", auth.as_str())];
    let (status, _) = request(
        &app,
        "POST",
        "/api/collections",
        &admin,
        Some(json!({ "name": "notes", "listRule": "", "fields": [{ "name": "text", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for i in 0..40 {
        request(
            &app,
            "POST",
            "/api/collections/notes/records",
            &admin,
            Some(json!({ "text": format!("note number {i}, with some words") })),
        )
        .await;
    }

    let get = |encoding: &'static str, uri: &'static str| {
        let app = app.clone();
        async move {
            raw_request(
                &app,
                "GET",
                uri,
                &[("accept-encoding", encoding)],
                Vec::new(),
            )
            .await
        }
    };
    let (_, headers, plain) = get("identity", "/api/collections/notes/records?perPage=40").await;
    assert!(headers.get("content-encoding").is_none());
    let expected: Value = serde_json::from_slice(&plain).unwrap();

    let (_, headers, gz) = get("gzip", "/api/collections/notes/records?perPage=40").await;
    assert_eq!(headers["content-encoding"], "gzip");
    assert!(
        gz.len() * 4 < plain.len(),
        "{} vs {}",
        gz.len(),
        plain.len()
    );
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(&gz[..])
        .read_to_end(&mut out)
        .unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&out).unwrap(), expected);

    let (_, headers, br) = get(
        "br;q=1, gzip;q=0.5",
        "/api/collections/notes/records?perPage=40",
    )
    .await;
    assert_eq!(headers["content-encoding"], "br");
    assert!(br.len() * 4 < plain.len());

    // small answers aren't worth it
    let (_, headers, _) = get("gzip", "/api/collections/notes/records?perPage=1").await;
    assert!(headers.get("content-encoding").is_none());

    // answers with tokens never are, however large
    let body = json!({ "identity": ADMIN_EMAIL, "password": ADMIN_PASSWORD })
        .to_string()
        .into_bytes();
    let (status, headers, _) = raw_request(
        &app,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        &[
            ("accept-encoding", "gzip, br"),
            ("content-type", "application/json"),
        ],
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("content-encoding").is_none());
    let (status, headers, _) = raw_request(
        &app,
        "POST",
        "/api/collections/_superusers/auth-refresh",
        &[
            ("accept-encoding", "gzip, br"),
            ("authorization", auth.as_str()),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("content-encoding").is_none());
}
