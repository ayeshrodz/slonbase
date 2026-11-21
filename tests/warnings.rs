//! Configuration warnings: settings that are valid and allowed but easy to get wrong are
//! listed (and logged), and change nothing. See `setup_flow.rs` for how to point the tests at a
//! database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

async fn call(
    app: &Arc<App>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    request(app, method, uri, &[("authorization", auth.as_str())], body).await
}

/// The codes and subjects of the warnings now.
async fn warnings(app: &Arc<App>, token: &str) -> Vec<(String, String)> {
    let (status, res) = call(app, token, "GET", "/api/settings/warnings", None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    res["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w["code"].as_str().unwrap().to_string(),
                w["subject"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_new_installation_has_no_warnings() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    assert!(warnings(&app, &token).await.is_empty());
    // only superusers see them
    let (status, _) = request(&app, "GET", "/api/settings/warnings", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    tdb.drop_db().await;
}

#[tokio::test]
async fn proxy_headers_without_peers_are_a_warning() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "trustedProxy": { "headers": ["X-Forwarded-For"] } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(
        warnings(&app, &token).await,
        [("proxyPeers".to_string(), "trustedProxy".to_string())]
    );
    // the health page lists it with the other checks
    let (status, res) = call(&app, &token, "GET", "/api/system/status", None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let check = res["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "config-proxyPeers-trustedProxy")
        .unwrap_or_else(|| panic!("no check in {res}"));
    assert_eq!(check["status"], "warn");
    // the proxies named: no warning
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "trustedProxy": { "peers": ["10.0.0.0/8"] } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert!(warnings(&app, &token).await.is_empty());
    tdb.drop_db().await;
}

#[tokio::test]
async fn open_rules_on_accounts_and_unverified_openid_tokens_are_warnings() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let provider = |extra: Value| {
        json!({
            "enabled": true,
            "providers": [{
                "name": "oidc", "clientId": "c", "clientSecret": "s",
                "authURL": "https://id.example.com/authorize",
                "tokenURL": "https://id.example.com/token",
                "userInfoURL": "https://id.example.com/userinfo",
                "extra": extra,
            }],
        })
    };
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "members", "type": "auth",
            "manageRule": "", "updateRule": "", "deleteRule": "id = @request.auth.id",
            "oauth2": provider(json!({})),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let mut found = warnings(&app, &token).await;
    found.sort();
    assert_eq!(
        found,
        [
            ("oidcSignature".to_string(), "members: oidc".to_string()),
            (
                "openManageRule".to_string(),
                "members: manageRule".to_string()
            ),
            (
                "openWriteRule".to_string(),
                "members: updateRule".to_string()
            ),
        ]
    );

    // requiring the signature without its keys is refused when saving
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/collections/members",
        Some(json!({ "oauth2": provider(json!({ "requireSignature": true })) })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    // with its keys it is accepted, and the warning goes
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/collections/members",
        Some(json!({ "oauth2": provider(json!({
            "requireSignature": true, "jwksURL": "https://id.example.com/jwks",
        })) })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert!(
        !warnings(&app, &token)
            .await
            .iter()
            .any(|(code, _)| code == "oidcSignature")
    );
    tdb.drop_db().await;
}

/// A stored template or automation that would not be saved under today's rules is listed (not
/// changed, not switched off); valid ones are not.
#[tokio::test]
async fn stored_templates_and_automations_that_no_longer_validate_are_listed() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "invoices", "fields": [
            { "name": "customer", "type": "text" },
            { "name": "total", "type": "number" },
        ] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let collection = res;
    let (status, template) = call(
        &app,
        &token,
        "POST",
        "/api/templates",
        Some(json!({ "name": "Reminder", "subject": "Overdue", "body": "<p>Hello</p>" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{template}");
    let (status, _) = call(
        &app,
        &token,
        "POST",
        "/api/templates",
        Some(json!({ "name": "Fine", "subject": "Fine", "body": "<p>Fine</p>" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/automations",
        Some(
            json!({ "name": "Overdue", "expression": "0 0 29 2 *", "type": "steps",
            "config": { "steps": [
                { "name": "find", "type": "query",
                  "config": { "collection": "invoices", "sort": "customer" } },
            ] } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // valid: nothing to say (the check runs in the background, so wait for one that ends)
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(warnings(&app, &token).await.is_empty());

    // a template saved under looser rules (a subject of 250 characters, constant)
    let core = app.core().unwrap();
    let id = template["id"].as_str().unwrap().to_string();
    slonbase::settings::change_unchecked(&core, |settings| {
        let template = settings.templates.iter_mut().find(|t| t.id == id).unwrap();
        template.subject = "s".repeat(250);
    })
    .await
    .unwrap();
    // an automation whose sort field was renamed since
    let mut renamed = collection.clone();
    renamed["fields"][1]["name"] = json!("customer_name");
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/collections/invoices",
        Some(json!({ "fields": renamed["fields"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    let mut found = Vec::new();
    for _ in 0..100 {
        found = warnings(&app, &token).await;
        if found.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    found.sort();
    assert_eq!(
        found,
        [
            (
                "invalidAutomation".to_string(),
                "automation Overdue".to_string()
            ),
            (
                "invalidTemplate".to_string(),
                "template Reminder".to_string()
            ),
        ]
    );
    // nothing was changed or switched off
    let (_, list) = call(&app, &token, "GET", "/api/templates", None).await;
    assert_eq!(list["items"].as_array().map(Vec::len), Some(2), "{list}");
    assert!(list["items"].to_string().contains(&"s".repeat(250)));
    tdb.drop_db().await;
}

#[cfg(feature = "js")]
#[tokio::test]
async fn hook_files_without_a_time_or_memory_limit_are_a_warning() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let hooks = tempfile::tempdir().unwrap();
    std::fs::write(hooks.path().join("a.pb.js"), "// no handlers\n").unwrap();
    let dir = hooks.path().to_path_buf();

    // the database URL comes from the environment, so setup writes no limits
    let (app, token, _dir) = ready_app_configured(&tdb, slonbase::Extensions::default(), |o| {
        o.hooks_dir = Some(dir.clone());
    })
    .await;
    let found = warnings(&app, &token).await;
    assert!(
        found.iter().any(|(code, _)| code == "hookLimits"),
        "{found:?}"
    );

    // a limit, either one, is the operator's choice and no warning
    let Some(tdb2) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir2) = ready_app_configured(&tdb2, slonbase::Extensions::default(), |o| {
        o.hooks_dir = Some(dir.clone());
        o.hooks_timeout = Some(60);
    })
    .await;
    assert!(
        !warnings(&app, &token)
            .await
            .iter()
            .any(|(code, _)| code == "hookLimits")
    );
    tdb.drop_db().await;
    tdb2.drop_db().await;
}
