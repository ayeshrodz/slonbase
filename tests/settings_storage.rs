//! How the settings are stored: a row per section (`settings:<section>`), only the changed
//! sections written, and settings stored as one value (up to 2.6) converted at the first start.
//! See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;

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

async fn db(tdb: &TestDb) -> tokio_postgres::Client {
    let (client, conn) = tokio_postgres::connect(&tdb.url, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(conn);
    client
}

/// The params keys that hold settings, sorted.
async fn keys(client: &tokio_postgres::Client) -> Vec<String> {
    client
        .query(
            "SELECT key FROM _slonbase.params WHERE key LIKE 'settings%' ORDER BY key",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect()
}

/// Puts the settings back the way 2.6 stored them: one value under `settings`.
async fn store_as_one_value(client: &tokio_postgres::Client) {
    client
        .batch_execute(
            "INSERT INTO _slonbase.params (key, value)
             SELECT 'settings', jsonb_object_agg(substr(key, 10), value)
             FROM _slonbase.params WHERE key LIKE 'settings:%';
             DELETE FROM _slonbase.params WHERE key LIKE 'settings:%';",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn each_section_is_a_row_and_only_changed_sections_are_written() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let client = db(&tdb).await;

    let mut expected: Vec<String> = slonbase::settings::SECTIONS
        .iter()
        .map(|s| format!("settings:{}", s.name))
        .collect();
    expected.sort();
    assert_eq!(keys(&client).await, expected);

    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "smtp": { "host": "mail.acme.test", "password": "s3cret" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["smtp"]["password"], "", "never sent back");
    let updated = async |key: &str| -> String {
        client
            .query_one(
                "SELECT updated::text FROM _slonbase.params WHERE key = $1",
                &[&key],
            )
            .await
            .unwrap()
            .get(0)
    };
    let smtp: Value = client
        .query_one(
            "SELECT value FROM _slonbase.params WHERE key = 'settings:smtp'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(smtp["password"], "s3cret");
    assert_eq!(smtp["host"], "mail.acme.test");

    // a change elsewhere doesn't rewrite the mail server's row, and keeps its password
    let (smtp_before, meta_before) = (
        updated("settings:smtp").await,
        updated("settings:meta").await,
    );
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "meta": { "appName": "Shop" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(updated("settings:smtp").await, smtp_before);
    assert_ne!(updated("settings:meta").await, meta_before);
    assert_eq!(app.core().unwrap().settings().smtp.password, "s3cret");
    assert_eq!(keys(&client).await, expected, "no other row appears");
}

#[tokio::test]
async fn settings_stored_as_one_value_are_converted_at_the_next_start() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let client = db(&tdb).await;
    for body in [
        json!({ "smtp": { "host": "mail.acme.test", "password": "s3cret" } }),
        json!({ "meta": { "appName": "Shop" }, "logs": { "maxDays": 3 } }),
    ] {
        let (status, res) = admin(&app, &token, "PATCH", "/api/settings", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/team/keys",
        Some(json!({ "name": "ci", "role": "viewer" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let before = app.core().unwrap().settings();
    assert_eq!(before.api_keys.len(), 1);

    store_as_one_value(&client).await;
    assert_eq!(keys(&client).await, ["settings"]);

    // the next start reads them, and stores them a section a row
    let (_config, path) = temp_config();
    let second = new_app(&path, Some(tdb.url.clone()));
    second.bootstrap().await.unwrap();
    let after = second.core().unwrap().settings();
    assert_eq!(*after, *before);
    let stored = keys(&client).await;
    assert_eq!(
        stored.len(),
        slonbase::settings::SECTIONS.len(),
        "{stored:?}"
    );
    assert!(!stored.contains(&"settings".to_string()), "{stored:?}");

    // and they are changed as before
    let (status, res) = admin(
        &second,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "meta": { "appName": "Store" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["meta"]["appName"], "Store");
    assert_eq!(res["logs"]["maxDays"], 3);
    assert_eq!(second.core().unwrap().settings().smtp.password, "s3cret");
}

#[tokio::test]
async fn one_value_left_by_a_restored_backup_is_read_and_goes_at_the_next_save() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let client = db(&tdb).await;
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "meta": { "appName": "Shop" }, "smtp": { "password": "s3cret" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // (what restoring a backup made by 2.6 leaves while the instance runs)
    store_as_one_value(&client).await;
    let pool = tdb.pool();
    let loaded = slonbase::settings::Settings::load(&pool.get().await.unwrap(), None)
        .await
        .unwrap();
    assert_eq!(loaded.meta.app_name, "Shop");
    assert_eq!(loaded.smtp.password, "s3cret");

    // a section's own row counts over the one value
    client
        .batch_execute(
            "INSERT INTO _slonbase.params (key, value)
             SELECT 'settings:meta', value->'meta' || '{\"appName\": \"Newer\"}'
             FROM _slonbase.params WHERE key = 'settings'",
        )
        .await
        .unwrap();
    let loaded = slonbase::settings::Settings::load(&pool.get().await.unwrap(), None)
        .await
        .unwrap();
    assert_eq!(loaded.meta.app_name, "Newer");
    assert_eq!(loaded.smtp.password, "s3cret");

    // the next save converts: nothing of the one value is lost
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "logs": { "maxDays": 9 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let stored = keys(&client).await;
    assert_eq!(
        stored.len(),
        slonbase::settings::SECTIONS.len(),
        "{stored:?}"
    );
    assert!(!stored.contains(&"settings".to_string()), "{stored:?}");
    let settings = app.core().unwrap().settings();
    assert_eq!(settings.smtp.password, "s3cret");
    assert_eq!(settings.logs.max_days, 9);
}
