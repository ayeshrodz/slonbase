//! Integration tests against a real PostgreSQL server.
//!
//! Set `SLONBASE_TEST_DATABASE_URL` to a URL of a role allowed to create databases,
//! e.g. `postgres://postgres:postgres@127.0.0.1:5432/postgres`. Each test creates and
//! drops its own database. Without the variable the tests are skipped.

mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::*;
use slonbase::app::Mode;
use slonbase::config::{self, Overrides};
use slonbase::{db, migrations};

#[tokio::test]
async fn wizard_flow_without_configured_database() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (_dir, config_path) = temp_config();

    let app = new_app(&config_path, None);
    app.bootstrap().await.unwrap();
    assert!(matches!(app.mode(), Mode::Setup(ref s) if s.pool.is_none()));
    let token = app.setup_token().to_string();

    // health works in setup mode
    let (status, body) = call(&app, "GET", "/api/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["setupRequired"], true);

    // the token is required
    let (status, _) = call(&app, "GET", "/api/setup", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(&app, "GET", "/api/setup", Some("wrong"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = call(&app, "GET", "/api/setup", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["needsDatabase"], true);

    // unreachable database
    let (status, body) = call(
        &app,
        "POST",
        "/api/setup/database/test",
        Some(&token),
        Some(json!({ "url": "postgres://nobody@127.0.0.1:1/none?sslmode=disable" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // reachable database
    let (status, body) = call(
        &app,
        "POST",
        "/api/setup/database/test",
        Some(&token),
        Some(json!({ "url": tdb.url })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["database"], tdb.name);
    assert_eq!(body["existingInstall"], false);

    // validation errors
    let (status, body) = call(
        &app,
        "POST",
        "/api/setup/complete",
        Some(&token),
        Some(json!({ "databaseUrl": tdb.url, "email": "nope", "password": "short" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["data"]["email"].is_object());
    assert!(body["data"]["password"].is_object());

    // success
    let (status, body) = call(
        &app,
        "POST",
        "/api/setup/complete",
        Some(&token),
        Some(json!({ "databaseUrl": tdb.url, "email": "admin@example.com", "password": "0123456789" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(matches!(app.mode(), Mode::Ready { .. }));

    // the database URL was saved
    let saved = config::load(&config_path, Overrides::default()).unwrap();
    assert_eq!(saved.file.database.url.as_deref(), Some(tdb.url.as_str()));

    // setup is now disabled, even with the token
    let (status, _) = call(&app, "GET", "/api/setup", Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        "POST",
        "/api/setup/complete",
        Some(&token),
        Some(json!({ "databaseUrl": tdb.url, "email": "evil@example.com", "password": "0123456789" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = call(&app, "GET", "/api/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "API is healthy.");

    // a restart with the saved config goes straight to ready
    let restarted = new_app(&config_path, None);
    restarted.bootstrap().await.unwrap();
    assert!(matches!(restarted.mode(), Mode::Ready { .. }));

    drop((app, restarted));
    tdb.drop_db().await;
}

#[tokio::test]
async fn reconnecting_requires_existing_superuser_credentials() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };

    // first installation
    let (_dir1, path1) = temp_config();
    let first = new_app(&path1, None);
    first.bootstrap().await.unwrap();
    let (status, _) = call(
        &first,
        "POST",
        "/api/setup/complete",
        Some(first.setup_token()),
        Some(json!({ "databaseUrl": tdb.url, "email": "owner@example.com", "password": "0123456789" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // a second server pointed at the same database
    let (_dir2, path2) = temp_config();
    let second = new_app(&path2, None);
    second.bootstrap().await.unwrap();
    let token = second.setup_token().to_string();

    let (status, body) = call(
        &second,
        "POST",
        "/api/setup/database/test",
        Some(&token),
        Some(json!({ "url": tdb.url })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["existingInstall"], true);
    assert_eq!(body["hasSuperusers"], true);

    for (email, password) in [
        ("intruder@example.com", "0123456789"),
        ("owner@example.com", "wrong-password"),
    ] {
        let (status, _) = call(
            &second,
            "POST",
            "/api/setup/complete",
            Some(&token),
            Some(json!({ "databaseUrl": tdb.url, "email": email, "password": password })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(matches!(second.mode(), Mode::Setup(_)));
    }
    assert!(!path2.exists(), "config must not be saved on failure");

    // email matching is case-insensitive
    let (status, _) = call(
        &second,
        "POST",
        "/api/setup/complete",
        Some(&token),
        Some(json!({ "databaseUrl": tdb.url, "email": "Owner@Example.com", "password": "0123456789" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(matches!(second.mode(), Mode::Ready { .. }));

    let count = tdb
        .pool()
        .get()
        .await
        .unwrap()
        .query_one("SELECT count(*) FROM _slonbase.superusers", &[])
        .await
        .unwrap()
        .get::<_, i64>(0);
    assert_eq!(count, 1);

    drop((first, second));
    tdb.drop_db().await;
}

#[tokio::test]
async fn externally_configured_database_skips_the_database_step() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (_dir, config_path) = temp_config();

    let app = new_app(&config_path, Some(tdb.url.clone()));
    app.bootstrap().await.unwrap();
    assert!(matches!(app.mode(), Mode::Setup(ref s) if s.pool.is_some()));
    let token = app.setup_token().to_string();

    let (_, body) = call(&app, "GET", "/api/setup", Some(&token), None).await;
    assert_eq!(body["needsDatabase"], false);

    // a different URL can't be injected
    let (status, _) = call(
        &app,
        "POST",
        "/api/setup/complete",
        Some(&token),
        Some(json!({ "databaseUrl": "postgres://elsewhere/db", "email": "admin@example.com", "password": "0123456789" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = call(
        &app,
        "POST",
        "/api/setup/complete",
        Some(&token),
        Some(json!({ "email": "admin@example.com", "password": "0123456789" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(matches!(app.mode(), Mode::Ready { .. }));
    // env provided values are never persisted
    assert!(!config_path.exists());

    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn concurrent_migrations_apply_once() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let pool = tdb.pool();
        tasks.push(tokio::spawn(async move { migrations::run(&pool).await }));
    }
    let mut total_applied = 0;
    for t in tasks {
        total_applied += t.await.unwrap().unwrap().len();
    }
    assert_eq!(total_applied, migrations::MIGRATIONS.len());

    // running again is a no-op
    assert!(migrations::run(&tdb.pool()).await.unwrap().is_empty());

    tdb.drop_db().await;
}

#[tokio::test]
async fn refuses_newer_schema_and_foreign_schema() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let pool = tdb.pool();

    // a "_slonbase" schema we didn't create
    pool.get()
        .await
        .unwrap()
        .batch_execute("CREATE SCHEMA _slonbase")
        .await
        .unwrap();
    let err = db::check_server(&pool).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("not created by slonbase"),
        "{err:#}"
    );
    pool.get()
        .await
        .unwrap()
        .batch_execute("DROP SCHEMA _slonbase")
        .await
        .unwrap();

    // a schema migrated by a newer slonbase
    migrations::run(&pool).await.unwrap();
    pool.get()
        .await
        .unwrap()
        .batch_execute("INSERT INTO _slonbase.migrations (version, name) VALUES (9999, 'future')")
        .await
        .unwrap();
    let err = migrations::run(&pool).await.unwrap_err();
    assert!(format!("{err:#}").contains("newer"), "{err:#}");

    drop(pool);
    tdb.drop_db().await;
}
