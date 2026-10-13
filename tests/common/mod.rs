//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use slonbase::app::App;
use slonbase::config::{self, DatabaseConfig, Overrides};
use slonbase::{db, ids, server};

pub struct TestDb {
    pub admin_url: String,
    pub name: String,
    pub url: String,
}

impl TestDb {
    pub async fn create() -> Option<Self> {
        let Ok(admin_url) = std::env::var("SLONBASE_TEST_DATABASE_URL") else {
            eprintln!("SLONBASE_TEST_DATABASE_URL not set, skipping");
            return None;
        };
        let name = format!("slonbase_test_{}", ids::random_id(10));
        admin_exec(&admin_url, &format!("CREATE DATABASE {name}")).await;

        let url = with_database(&admin_url, &name);
        Some(Self {
            admin_url,
            name,
            url,
        })
    }

    pub fn pool(&self) -> deadpool_postgres::Pool {
        db::connect(&self.url, &DatabaseConfig::default()).unwrap()
    }

    pub async fn drop_db(self) {
        admin_exec(
            &self.admin_url,
            &format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name),
        )
        .await;
    }
}

/// Replaces the database name in the admin URL.
pub fn with_database(admin_url: &str, name: &str) -> String {
    let (base, query) = match admin_url.split_once('?') {
        Some((b, q)) => (b, format!("?{q}")),
        None => (admin_url, String::new()),
    };
    let base = match base.rfind('/') {
        Some(i) if i > "postgres://".len() => &base[..i],
        _ => base,
    };
    format!("{base}/{name}{query}")
}

pub async fn admin_exec(admin_url: &str, sql: &str) {
    let (client, conn) = tokio_postgres::connect(admin_url, tokio_postgres::NoTls)
        .await
        .expect("connect to SLONBASE_TEST_DATABASE_URL");
    tokio::spawn(conn);
    client.batch_execute(sql).await.unwrap();
}

pub fn new_app(config_path: &Path, database_url: Option<String>) -> Arc<App> {
    new_app_with(config_path, database_url, slonbase::Extensions::default())
}

pub fn new_app_with(
    config_path: &Path,
    database_url: Option<String>,
    extensions: slonbase::Extensions,
) -> Arc<App> {
    let loaded = config::load(
        config_path,
        Overrides {
            database_url,
            // uploads go next to the test's config file
            storage_path: Some(config_path.parent().unwrap().join("storage")),
            backups_path: Some(config_path.parent().unwrap().join("backups")),
            ..Default::default()
        },
    )
    .unwrap();
    App::with_extensions(loaded, None, extensions).unwrap()
}

pub async fn call(
    app: &Arc<App>,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        req = req.header("x-setup-token", token);
    }
    let req = match body {
        Some(body) => req
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();

    let res = server::router(app.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

pub fn temp_config() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    (dir, path)
}

/// Sends a request with optional headers and JSON body.
pub async fn request(
    app: &Arc<App>,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let req = match body {
        Some(body) => req
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();

    let res = server::router(app.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// Sends a request with a raw body; returns the status, headers and body bytes.
pub async fn raw_request(
    app: &Arc<App>,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let res = server::router(app.clone())
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, headers, bytes)
}

/// A multipart/form-data body: text fields and `(key, filename, content)` files.
pub fn multipart(fields: &[(&str, &str)], files: &[(&str, &str, &[u8])]) -> (String, Vec<u8>) {
    let boundary = "----slonbase-test-boundary";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    for (name, filename, content) in files {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

pub const ADMIN_EMAIL: &str = "admin@example.com";
pub const ADMIN_PASSWORD: &str = "0123456789";

/// A set-up app with a superuser, plus that superuser's auth token.
pub async fn ready_app(tdb: &TestDb) -> (Arc<App>, String, tempfile::TempDir) {
    ready_app_with(tdb, slonbase::Extensions::default()).await
}

/// [`ready_app`] with hooks and routes.
pub async fn ready_app_with(
    tdb: &TestDb,
    extensions: slonbase::Extensions,
) -> (Arc<App>, String, tempfile::TempDir) {
    let (dir, config_path) = temp_config();
    let app = new_app_with(&config_path, Some(tdb.url.clone()), extensions);
    app.bootstrap().await.unwrap();
    let token = app.setup_token().to_string();
    let (status, body) = call(
        &app,
        "POST",
        "/api/setup/complete",
        Some(&token),
        Some(json!({ "email": ADMIN_EMAIL, "password": ADMIN_PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = request(
        &app,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        &[],
        Some(json!({ "identity": ADMIN_EMAIL, "password": ADMIN_PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let auth_token = body["token"].as_str().unwrap().to_string();
    (app, auth_token, dir)
}
