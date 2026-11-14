//! The settings and the token signing secret encrypted in the database (`server.encryption_key`).
//! Needs PostgreSQL (see `setup_flow.rs`).

mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;

const KEY: &str = "0123456789abcdef0123456789abcdef";
const OTHER_KEY: &str = "fedcba9876543210fedcba9876543210";
const SMTP_PASSWORD: &str = "smtp-password-in-plain-sight";

const CLIENT_SECRET: &str = "oauth2-client-secret-in-plain-sight";

/// The stored definition of the `members` collection.
async fn members_data(tdb: &TestDb) -> Value {
    let client = tdb.pool().get().await.unwrap();
    client
        .query_one(
            "SELECT data FROM _slonbase.collections WHERE name = 'members'",
            &[],
        )
        .await
        .unwrap()
        .get(0)
}

/// The client secret the running app uses.
fn secret_in_use(app: &std::sync::Arc<slonbase::app::App>) -> String {
    let core = app.core().unwrap();
    let registry = core.registry();
    let c = registry.get("members").unwrap();
    c.auth_options().unwrap().oauth2.providers[0]
        .client_secret
        .clone()
}

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
    (get("settings:smtp").await, get("auth_secret").await)
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
    // and an OAuth2 client secret in a collection
    let (status, res) = admin(
        &plain,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "members", "type": "auth",
            "oauth2": { "enabled": true, "providers": [{
                "name": "github", "clientId": "client-1", "clientSecret": CLIENT_SECRET,
            }] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert!(members_data(&tdb).await.to_string().contains(CLIENT_SECRET));
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
    // the client secret too, and it's still the one in use
    let stored = members_data(&tdb).await;
    assert!(!stored.to_string().contains(CLIENT_SECRET), "{stored}");
    assert!(
        sealed(&stored["oauth2"]["providers"][0]["clientSecret"]),
        "{stored}"
    );
    assert_eq!(secret_in_use(&keyed), CLIENT_SECRET);
    // changing the collection keeps it sealed (an empty secret keeps the stored one)
    let (status, res) = admin(
        &keyed,
        &token,
        "PATCH",
        "/api/collections/members",
        Some(json!({ "oauth2": { "enabled": true, "providers": [{
            "name": "github", "clientId": "client-2", "clientSecret": "",
        }] } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(
        res["oauth2"]["providers"][0]["clientSecret"], "",
        "never shown"
    );
    let stored = members_data(&tdb).await;
    assert!(
        sealed(&stored["oauth2"]["providers"][0]["clientSecret"]),
        "{stored}"
    );
    assert_eq!(stored["oauth2"]["providers"][0]["clientId"], "client-2");
    assert_eq!(secret_in_use(&keyed), CLIENT_SECRET);
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
    assert_eq!(settings["password"], SMTP_PASSWORD);
    assert_eq!(
        members_data(&tdb).await["oauth2"]["providers"][0]["clientSecret"],
        CLIENT_SECRET
    );
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

#[tokio::test]
async fn project_role_passwords_encrypted_at_rest() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    // the main installation's role may create databases and roles (projects with their own)
    let main_role = format!("{}_main", tdb.name);
    admin_exec(
        &tdb.admin_url,
        &format!(
            "CREATE ROLE {main_role} LOGIN PASSWORD 'main-role-password' CREATEDB CREATEROLE;
             ALTER DATABASE {} OWNER TO {main_role};",
            tdb.name
        ),
    )
    .await;
    let main_url = {
        let (scheme, rest) = tdb.url.split_once("://").unwrap();
        let rest = rest.split_once('@').map(|(_, host)| host).unwrap_or(rest);
        format!("{scheme}://{main_role}:main-role-password@{rest}")
    };
    let (_dir, config) = temp_config();
    let app = new_app_configured(
        &config,
        Some(main_url.clone()),
        Default::default(),
        with_key(KEY),
    );
    app.bootstrap().await.unwrap();
    let (status, body) = call(
        &app,
        "POST",
        "/api/setup/complete",
        Some(app.setup_token()),
        Some(json!({ "email": ADMIN_EMAIL, "password": ADMIN_PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, auth) = request(
        &app,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        &[],
        Some(json!({ "identity": ADMIN_EMAIL, "password": ADMIN_PASSWORD })),
    )
    .await;
    let token = auth["token"].as_str().unwrap().to_string();
    let (status, project) = admin(
        &app,
        &token,
        "POST",
        "/api/projects",
        Some(json!({ "name": "vault", "superuserEmail": "p@example.com",
                     "superuserPassword": "project-password-1", "ownRole": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{project}");

    // stored sealed
    let client = tdb.pool().get().await.unwrap();
    let stored: String = client
        .query_one(
            "SELECT role_password FROM _slonbase.projects WHERE name = 'vault'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(stored.starts_with("sealed:"), "{stored}");

    // and the project, served with the role's password, works; again after a restart
    let sign_in = |app: std::sync::Arc<slonbase::app::App>| async move {
        for _ in 0..100 {
            let (status, _) = request(
                &app,
                "POST",
                "/p/vault/api/collections/_superusers/auth-with-password",
                &[],
                Some(json!({ "identity": "p@example.com", "password": "project-password-1" })),
            )
            .await;
            if status == StatusCode::OK {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        false
    };
    assert!(sign_in(app.clone()).await, "the project answers");
    drop(app);
    let (_dir2, config2) = temp_config();
    let again = new_app_configured(&config2, Some(main_url), Default::default(), with_key(KEY));
    again.bootstrap().await.unwrap();
    assert!(
        sign_in(again.clone()).await,
        "the project answers after a restart"
    );

    // rekeying covers it
    let mut main = tdb.pool().get().await.unwrap();
    let key = slonbase::secrets::SecretsKey::new(KEY).unwrap();
    slonbase::secrets::rekey(&mut main, Some(&key), None)
        .await
        .unwrap();
    let stored: String = client
        .query_one(
            "SELECT role_password FROM _slonbase.projects WHERE name = 'vault'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!stored.starts_with("sealed:"));

    drop((again, client, main));
    let admin_url = tdb.admin_url.clone();
    let database = format!("{}_vault", tdb.name);
    admin_exec(
        &admin_url,
        &format!("DROP DATABASE IF EXISTS {database} WITH (FORCE)"),
    )
    .await;
    tdb.drop_db().await;
    admin_exec(
        &admin_url,
        &format!("DROP ROLE IF EXISTS {database}; DROP ROLE IF EXISTS {main_role};"),
    )
    .await;
}
