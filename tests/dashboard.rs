//! The admin dashboard and the API it uses: serving the UI, scaffolds, OAuth2 providers,
//! settings, the test email, collection import and truncate, crons and the SQL console.
//! See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;
use slonbase::mail::MemoryMailer;

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

#[tokio::test]
async fn serves_the_dashboard() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, _, _dir) = ready_app(&tdb).await;

    let (status, headers, body) = raw_request(
        &app,
        "GET",
        "/_/",
        &[("accept-encoding", "gzip, br")],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-encoding"], "gzip");
    assert_eq!(headers["cache-control"], "no-cache");
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    assert!(body.starts_with(&[0x1f, 0x8b]), "gzipped");

    // decompressed for clients that don't accept gzip
    let (_, headers, body) = raw_request(&app, "GET", "/_/", &[], vec![]).await;
    assert!(headers.get("content-encoding").is_none());
    let html = String::from_utf8(body).unwrap();
    let asset = html
        .split("src=\"./assets/")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("a script asset");
    let (status, headers, _) =
        raw_request(&app, "GET", &format!("/_/assets/{asset}"), &[], vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "text/javascript; charset=utf-8");
    assert!(
        headers["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );

    let (status, _, body) = raw_request(&app, "GET", "/_/extensions.js", &[], vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());
    let (status, _, _) = raw_request(&app, "GET", "/_/missing.js", &[], vec![]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = raw_request(&app, "GET", "/_/../Cargo.toml", &[], vec![]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // the setup page is still there
    let (status, _, _) = raw_request(&app, "GET", "/_/setup", &[], vec![]).await;
    assert_eq!(status, StatusCode::OK);

    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn dashboard_endpoints() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let mailer = Arc::new(MemoryMailer::default());
    app.core().unwrap().set_mailer(mailer.clone());

    // superusers only
    for uri in [
        "/api/collections/meta/scaffolds",
        "/api/collections/meta/oauth2-providers",
        "/api/crons",
    ] {
        let (status, _) = request(&app, "GET", uri, &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }

    let (status, scaffolds) =
        admin(&app, &token, "GET", "/api/collections/meta/scaffolds", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(scaffolds["base"]["fields"][0]["name"], "id");
    assert_eq!(scaffolds["base"]["id"], "");
    let auth_fields: Vec<&str> = scaffolds["auth"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    for name in [
        "id",
        "password",
        "tokenKey",
        "email",
        "emailVisibility",
        "verified",
    ] {
        assert!(auth_fields.contains(&name), "{name}");
    }
    // the system indexes are managed by slonbase, not listed
    assert_eq!(scaffolds["auth"]["indexes"], json!([]));
    assert_eq!(scaffolds["auth"]["passwordAuth"]["enabled"], true);
    assert_eq!(scaffolds["view"]["type"], "view");

    let (_, providers) = admin(
        &app,
        &token,
        "GET",
        "/api/collections/meta/oauth2-providers",
        None,
    )
    .await;
    let names: Vec<&str> = providers
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"github") && names.contains(&"oidc"),
        "{names:?}"
    );
    assert!(providers[0]["logo"].as_str().unwrap().contains("<svg"));

    let (_, crons) = admin(&app, &token, "GET", "/api/crons", None).await;
    assert_eq!(crons, json!([]));

    // the health check tells superusers what the dashboard shows
    let (_, health) = admin(&app, &token, "GET", "/api/health", None).await;
    assert_eq!(health["data"]["canBackup"], true);
    assert!(health["data"].get("realIP").is_some());
    let (_, guest_health) = request(&app, "GET", "/api/health", &[], None).await;
    assert_eq!(guest_health["data"], json!({}));

    // settings: every section, merged recursively, secrets kept
    let (_, settings) = admin(&app, &token, "GET", "/api/settings", None).await;
    for key in [
        "meta",
        "smtp",
        "s3",
        "backups",
        "logs",
        "batch",
        "rateLimits",
        "trustedProxy",
        "superuserIPs",
    ] {
        assert!(settings.get(key).is_some(), "{key}");
    }
    let (status, updated) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({
            "meta": { "accentColor": "#ff0000" },
            "smtp": { "password": "smtp-secret", "authMethod": "LOGIN" },
            "backups": { "s3": { "secret": "backup-secret" } },
            "rateLimits": { "enabled": true, "rules": [{ "label": "/api/", "duration": 1, "maxRequests": 10 }] },
            "trustedProxy": { "headers": ["X-Forwarded-For"] },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["meta"]["accentColor"], "#ff0000");
    assert_eq!(updated["meta"]["appName"], settings["meta"]["appName"]);
    assert_eq!(updated["smtp"]["password"], "");
    assert_eq!(updated["backups"]["s3"]["secret"], "");
    assert_eq!(updated["rateLimits"]["rules"].as_array().unwrap().len(), 1);
    let core = app.core().unwrap();
    assert_eq!(core.settings().smtp.password, "smtp-secret");
    let (_, again) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "backups": { "s3": { "bucket": "b", "secret": "" } }, "smtp": { "password": "" } })),
    )
    .await;
    assert_eq!(again["backups"]["s3"]["bucket"], "b");
    assert_eq!(core.settings().backups.s3.secret, "backup-secret");
    assert_eq!(core.settings().smtp.password, "smtp-secret");
    for (body, key) in [
        (json!({ "superuserIPs": ["nope"] }), "/data/superuserIPs/0"),
        (
            json!({ "rateLimits": { "rules": [{ "label": "", "duration": 0, "maxRequests": 1 }] } }),
            "/data/rateLimits/rules/0/label",
        ),
        (
            json!({ "smtp": { "authMethod": "CRAM" } }),
            "/data/smtp/authMethod",
        ),
        (
            json!({ "trustedProxy": { "headers": ["bad header"] } }),
            "/data/trustedProxy/headers/0",
        ),
    ] {
        let (status, res) = admin(&app, &token, "PATCH", "/api/settings", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
        assert!(res.pointer(key).is_some(), "{key}: {res}");
    }

    // the test email uses the collection's template
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/settings/test/email",
        Some(json!({ "email": "someone@example.com", "template": "password-reset" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let sent = mailer.sent.lock().unwrap().last().cloned().unwrap();
    assert_eq!(sent.to, "someone@example.com");
    assert!(sent.html.contains("TEST_TOKEN"), "{}", sent.html);
    let (status, _) = admin(
        &app,
        &token,
        "POST",
        "/api/settings/test/email",
        Some(json!({ "email": "someone@example.com", "template": "nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // the SQL console is off unless the server enables it
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/sql",
        Some(json!({ "query": "SELECT 1" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");
    assert!(res["message"].as_str().unwrap().contains("--sql-console"));

    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn sql_console_when_enabled() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (first, token, _dir) = ready_app(&tdb).await;
    let (_dir2, config) = temp_config();
    std::fs::write(&config, "[server]\nsql_console = true\n").unwrap();
    let app = new_app(&config, Some(tdb.url.clone()));
    app.bootstrap().await.unwrap();

    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/sql",
        Some(json!({ "query": "SELECT email, 41 + 1 AS answer, NULL AS nothing FROM slonbase.\"_superusers\"" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["columns"][0]["name"], "email");
    assert_eq!(res["columns"][1]["type"], "int4");
    assert_eq!(res["rows"][0], json!([ADMIN_EMAIL, "42", null]));
    assert_eq!(res["affectedRows"], 0);

    // reads run read-only
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/sql",
        Some(json!({ "query": "WITH x AS (DELETE FROM _slonbase.params RETURNING 1) SELECT count(*) FROM x" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(
        res["message"].as_str().unwrap().contains("read-only"),
        "{res}"
    );

    // writes commit
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/sql",
        Some(json!({ "query": "CREATE TABLE public.console_test (n int); INSERT INTO public.console_test VALUES (1), (2)" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["affectedRows"], 2);
    let (_, res) = admin(
        &app,
        &token,
        "POST",
        "/api/sql",
        Some(json!({ "query": "SELECT sum(n) FROM public.console_test" })),
    )
    .await;
    assert_eq!(res["rows"][0][0], "3");

    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/sql",
        Some(json!({ "query": "SELEC nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        res["message"]
            .as_str()
            .unwrap()
            .starts_with("Failed to execute query.")
    );
    let (status, _) = request(
        &app,
        "POST",
        "/api/sql",
        &[],
        Some(json!({ "query": "SELECT 1" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    drop((first, app));
    tdb.drop_db().await;
}

#[tokio::test]
async fn import_and_truncate() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, dir) = ready_app(&tdb).await;

    let (status, _) = admin(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "old_stuff", "fields": [{ "name": "n", "type": "number" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, exported) = admin(&app, &token, "GET", "/api/collections?perPage=100", None).await;
    let mut collections: Vec<Value> = exported["items"].as_array().unwrap().clone();

    // new collections referencing each other, an update, and the rest deleted
    collections.retain(|c| c["system"] == true);
    collections.push(json!({
        "id": "pbc_authors00001", "name": "authors", "type": "base",
        "fields": [{ "name": "name", "type": "text" }], "listRule": "",
    }));
    collections.push(json!({
        "name": "books", "type": "base",
        "fields": [
            { "name": "title", "type": "text", "required": true },
            { "name": "author", "type": "relation", "collectionId": "pbc_authors00001", "maxSelect": 1 },
            { "name": "cover", "type": "file" },
        ],
    }));
    let (status, res) = admin(
        &app,
        &token,
        "PUT",
        "/api/collections/import",
        Some(json!({ "collections": collections, "deleteMissing": true })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (_, list) = admin(&app, &token, "GET", "/api/collections?perPage=100", None).await;
    let names: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"books") && names.contains(&"authors"),
        "{names:?}"
    );
    assert!(!names.contains(&"old_stuff"), "deleted: {names:?}");
    assert!(names.contains(&"_superusers"));

    // an invalid import changes nothing
    let (status, res) = admin(
        &app,
        &token,
        "PUT",
        "/api/collections/import",
        Some(json!({ "collections": [
            { "name": "valid_one", "type": "base" },
            { "name": "broken", "type": "base", "fields": [{ "name": "x", "type": "relation", "collectionId": "missing" }] },
        ] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res.pointer("/data/collections/1/fields").is_some(), "{res}");
    let (status, _) = admin(&app, &token, "GET", "/api/collections/valid_one", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // truncate deletes the records with their files
    let (_, author) = admin(
        &app,
        &token,
        "POST",
        "/api/collections/authors/records",
        Some(json!({ "name": "Ann" })),
    )
    .await;
    let (content_type, body) = multipart(
        &[
            ("title", "First"),
            ("author", author["id"].as_str().unwrap()),
        ],
        &[("cover", "c.txt", b"hello")],
    );
    let auth = format!("Bearer {token}");
    let (status, _, created) = raw_request(
        &app,
        "POST",
        "/api/collections/books/records",
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
        String::from_utf8_lossy(&created)
    );
    let book: Value = serde_json::from_slice(&created).unwrap();
    let book_dir = dir
        .path()
        .join("storage")
        .join(book["collectionId"].as_str().unwrap())
        .join(book["id"].as_str().unwrap());
    assert!(book_dir.exists());

    let (status, _) = request(&app, "DELETE", "/api/collections/books/truncate", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = admin(
        &app,
        &token,
        "DELETE",
        "/api/collections/books/truncate",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, books) = admin(&app, &token, "GET", "/api/collections/books/records", None).await;
    assert_eq!(books["totalItems"], 0);
    assert!(!book_dir.exists(), "files deleted");
    let (status, _) = admin(
        &app,
        &token,
        "DELETE",
        "/api/collections/_superusers/truncate",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    drop(app);
    tdb.drop_db().await;
}
