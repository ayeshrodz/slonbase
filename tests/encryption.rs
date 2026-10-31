//! The settings and the token signing secret encrypted in the database (`server.encryption_key`).
//! Needs PostgreSQL (see `setup_flow.rs`).

mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;

const KEY: &str = "0123456789abcdef0123456789abcdef";
const OTHER_KEY: &str = "fedcba9876543210fedcba9876543210";
const SMTP_PASSWORD: &str = "smtp-password-in-plain-sight";

async fn params(tdb: &TestDb) -> (Value, Value) {
    let client = tdb.pool().get().await.unwrap();
    let get = |key: &'static str| {
        let client = &client;
        async move {
            client
                .query_one("SELECT value FROM _slonbase.params WHERE key = $1", &[&key])
                .await
                .unwrap()
                .get::<_, Value>(0)
        }
    };
    (get("settings").await, get("auth_secret").await)
}

fn sealed(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|o| o.len() == 1 && o["sealed"].is_string())
}

async fn admin(
    app: &std::sync::Arc<slonbase::app::App>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    request(app, method, uri, &[("authorization", auth.as_str())], body).await
}

fn with_key(key: &'static str) -> impl FnOnce(&mut slonbase::config::Overrides) {
    move |o| o.encryption_key = Some(key.into())
}

#[tokio::test]
async fn settings_and_signing_secret_encrypted_at_rest() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    // a plain installation, with a secret in its settings
    let (plain, token, _dir) = ready_app(&tdb).await;
    let (status, res) = admin(
        &plain,
        &token,
        "PATCH",
        "/api/settings",
        Some(
            json!({ "smtp": { "enabled": false, "host": "smtp.example.com", "port": 587,
                               "username": "mailer", "password": SMTP_PASSWORD } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (settings, secret) = params(&tdb).await;
    assert!(settings.to_string().contains(SMTP_PASSWORD));
    assert!(!sealed(&secret));
    drop(plain);

    // started with a key: both are sealed, and everything keeps working (the same signing
    // secret, so earlier tokens stay valid)
    let (_dir2, config2) = temp_config();
    let keyed = new_app_configured(
        &config2,
        Some(tdb.url.clone()),
        Default::default(),
        with_key(KEY),
    );
    keyed.bootstrap().await.unwrap();
    let (settings, secret) = params(&tdb).await;
    assert!(sealed(&settings), "{settings}");
    assert!(sealed(&secret), "{secret}");
    assert!(!settings.to_string().contains(SMTP_PASSWORD));
    assert!(!settings.to_string().contains("smtp.example.com"));
    let (status, res) = admin(&keyed, &token, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["smtp"]["host"], "smtp.example.com");
    // saving keeps them sealed
    let (status, res) = admin(
        &keyed,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "meta": { "appName": "Sealed app" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (settings, _) = params(&tdb).await;
    assert!(sealed(&settings), "{settings}");
    assert!(!settings.to_string().contains("Sealed app"));
    // the SMTP password still is what was set
    let core = keyed.core().unwrap();
    assert_eq!(core.settings().smtp.password, SMTP_PASSWORD);

    // without the key, or with another, an instance doesn't start (and says why)
    let (_dir3, config3) = temp_config();
    let keyless = new_app(&config3, Some(tdb.url.clone()));
    let err = format!("{:#}", keyless.bootstrap().await.unwrap_err());
    assert!(
        err.contains("encrypted") && err.contains("SLONBASE_ENCRYPTION_KEY"),
        "{err}"
    );
    let (_dir4, config4) = temp_config();
    let wrong = new_app_configured(
        &config4,
        Some(tdb.url.clone()),
        Default::default(),
        with_key(OTHER_KEY),
    );
    let err = format!("{:#}", wrong.bootstrap().await.unwrap_err());
    assert!(err.contains("the encryption key is wrong"), "{err}");

    // a backup carries them sealed; an installation without the key refuses to restore it,
    // and nothing changes there
    let (status, res) = admin(
        &keyed,
        &token,
        "POST",
        "/api/backups",
        Some(json!({ "name": "sealed.zip" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let archive = std::fs::read(_dir2.path().join("backups").join("sealed.zip")).unwrap();
    let Some(other_db) = TestDb::create().await else {
        return;
    };
    let (other, other_token, _other_dir) = ready_app(&other_db).await;
    let (content_type, body) = multipart(&[], &[("file", "sealed.zip", &archive)]);
    let auth = format!("Bearer {other_token}");
    let (status, _, _) = raw_request(
        &other,
        "POST",
        "/api/backups/upload",
        &[
            ("content-type", content_type.as_str()),
            ("authorization", auth.as_str()),
        ],
        body,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, res) = admin(
        &other,
        &other_token,
        "POST",
        "/api/backups/sealed.zip/restore",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(
        res["message"]
            .as_str()
            .unwrap()
            .contains("can't be restored here"),
        "{res}"
    );
    let (status, _) = admin(&other, &other_token, "GET", "/api/collections", None).await;
    assert_eq!(status, StatusCode::OK, "the installation is untouched");

    // rekey: to another key, then back to none
    let mut client = tdb.pool().get().await.unwrap();
    let from = slonbase::secrets::SecretsKey::new(KEY).unwrap();
    let to = slonbase::secrets::SecretsKey::new(OTHER_KEY).unwrap();
    slonbase::secrets::rekey(&mut client, Some(&from), Some(&to))
        .await
        .unwrap();
    assert!(
        slonbase::secrets::rekey(&mut client, Some(&from), None)
            .await
            .is_err(),
        "the old key no longer opens them"
    );
    slonbase::secrets::rekey(&mut client, Some(&to), None)
        .await
        .unwrap();
    drop(client);
    let (settings, secret) = params(&tdb).await;
    assert!(!sealed(&settings) && !sealed(&secret));
    assert_eq!(settings["smtp"]["password"], SMTP_PASSWORD);
    let (_dir5, config5) = temp_config();
    let unsealed = new_app(&config5, Some(tdb.url.clone()));
    unsealed.bootstrap().await.unwrap();
    let (status, res) = admin(&unsealed, &token, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK, "earlier tokens still work: {res}");
    assert_eq!(res["meta"]["appName"], "Sealed app");

    drop((keyed, keyless, wrong, other, unsealed));
    other_db.drop_db().await;
    tdb.drop_db().await;
}

#[tokio::test]
async fn a_new_installation_starts_sealed() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app_configured(&tdb, Default::default(), with_key(KEY)).await;
    let (settings, secret) = params(&tdb).await;
    assert!(sealed(&settings), "{settings}");
    assert!(sealed(&secret), "{secret}");
    let (status, res) = admin(&app, &token, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(
        res["rateLimits"]["enabled"], true,
        "a new installation's defaults"
    );
    drop(app);
    tdb.drop_db().await;
}
