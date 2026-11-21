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
