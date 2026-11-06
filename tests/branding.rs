//! Branding (2.0): the image library (`/api/branding/assets`), the settings that use it (the
//! logo, the browser icon, the sign-in page), what `/_/context.js` tells the dashboard before
//! anyone signs in, and that assets go with backups.
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

async fn upload(app: &Arc<App>, token: &str, name: &str, data: &[u8]) -> (StatusCode, Value) {
    let (content_type, body) = multipart(&[], &[("file", name, data)]);
    let auth = format!("Bearer {token}");
    let (status, _, bytes) = raw_request(
        app,
        "POST",
        "/api/branding/assets",
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

const SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#1f63a8"/></svg>"##;
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// The branding `/_/context.js` gives the dashboard.
async fn context(app: &Arc<App>, path: &str) -> Value {
    let (status, headers, body) = raw_request(app, "GET", path, &[], vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "no-store");
    let text = String::from_utf8(body).unwrap();
    let json = text
        .trim()
        .strip_prefix("window.SLONBASE_CONTEXT = ")
        .and_then(|t| t.strip_suffix(';'))
        .unwrap_or_else(|| panic!("{text}"));
    serde_json::from_str(json).unwrap()
}

#[tokio::test]
async fn image_library_and_branding() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;

    // only superusers manage images
    let (status, _) = request(&app, "GET", "/api/branding/assets", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (content_type, body) = multipart(&[], &[("file", "logo.svg", SVG)]);
    let (status, _, _) = raw_request(
        &app,
        "POST",
        "/api/branding/assets",
        &[("content-type", content_type.as_str())],
        body,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // what an upload says it is doesn't count: its bytes do
    let (status, logo) = upload(&app, &token, "../../logo.svg", SVG).await;
    assert_eq!(status, StatusCode::OK, "{logo}");
    assert_eq!(logo["name"], "logo.svg");
    assert_eq!(logo["mime"], "image/svg+xml");
    assert_eq!(logo["usedBy"], json!([]));
    let logo_id = logo["id"].as_str().unwrap().to_string();
    let (status, icon) = upload(&app, &token, "icon.svg", PNG).await;
    assert_eq!(status, StatusCode::OK, "{icon}");
    assert_eq!(icon["mime"], "image/png");
    let icon_id = icon["id"].as_str().unwrap().to_string();
    for (name, data, message) in [
        (
            "x.svg",
            &br#"<svg onload="alert(1)"/>"#[..],
            "event handlers",
        ),
        (
            "x.svg",
            br#"<svg><script>alert(1)</script></svg>"#,
            "scripts",
        ),
        (
            "x.svg",
            br#"<svg><image href="https://tracker.example/p.png"/></svg>"#,
            "link inside",
        ),
        ("page.html", b"<html><body>hi</body></html>", "PNG, JPEG"),
        ("empty.png", b"", "empty"),
    ] {
        let (status, res) = upload(&app, &token, name, data).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {res}");
        assert!(
            res["data"]["file"]["message"]
                .as_str()
                .unwrap()
                .contains(message),
            "{res}"
        );
    }
    let big = vec![0x89u8; slonbase::assets::MAX_SIZE + 1];
    let (status, res) = upload(&app, &token, "big.png", &big).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");

    // anyone can view an image, which can't run anything even opened on its own
    let uri = format!("/api/branding/assets/{logo_id}");
    let (status, headers, body) = raw_request(&app, "GET", &uri, &[], vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, SVG);
    assert_eq!(headers["content-type"], "image/svg+xml");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    assert!(
        headers["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    let etag = headers["etag"].to_str().unwrap().to_string();
    let (status, _, body) = raw_request(
        &app,
        "GET",
        &uri,
        &[("if-none-match", etag.as_str())],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
    for missing in [
        "/api/branding/assets/aaaaaaaaaaaaaaa",
        "/api/branding/assets/..%2Fx",
    ] {
        let (status, _, _) = raw_request(&app, "GET", missing, &[], vec![]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    }

    // renaming keeps names plain
    let (status, renamed) = admin(
        &app,
        &token,
        "PATCH",
        &uri,
        Some(json!({ "name": "Acme <logo>.svg" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "Acme logo.svg");

    // the settings use images that exist
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "meta": { "logo": "zzzzzzzzzzzzzzz" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"]["meta"]["logo"].is_object(), "{res}");
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({
            "meta": { "appName": "Acme Ops", "accentColor": "ocean", "logo": logo_id, "favicon": icon_id },
            "login": {
                "layout": "centered",
                "background": "grid",
                "title": "Acme operations",
                "notice": "Ask #platform for access.",
                "footerLinks": [
                    { "label": "Status", "url": "https://status.example.com" },
                    { "label": "Help", "url": "mailto:help@example.com" }
                ]
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["meta"]["accentColor"], "ocean");
    assert_eq!(res["login"]["layout"], "centered");

    // and say where each is used; one in use can't be deleted
    let (_, list) = admin(&app, &token, "GET", "/api/branding/assets", None).await;
    let used = |id: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == id)
            .unwrap()["usedBy"]
            .clone()
    };
    assert_eq!(used(&logo_id), json!(["the logo"]));
    assert_eq!(used(&icon_id), json!(["the browser icon"]));
    let (status, res) = admin(&app, &token, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["message"].as_str().unwrap().contains("the logo"));

    // the dashboard knows the branding before anyone signs in, and nothing secret
    let ctx = context(&app, "/_/context.js").await;
    let b = &ctx["branding"];
    assert_eq!(b["appName"], "Acme Ops");
    assert_eq!(b["accent"], "ocean");
    assert_eq!(b["logo"], format!("/api/branding/assets/{logo_id}"));
    assert_eq!(b["favicon"], format!("/api/branding/assets/{icon_id}"));
    assert_eq!(b["logoDark"], Value::Null);
    assert_eq!(b["login"]["title"], "Acme operations");
    assert_eq!(b["login"]["background"], "grid");
    assert_eq!(
        b["login"]["footerLinks"][1]["url"],
        "mailto:help@example.com"
    );
    let text = ctx.to_string();
    for secret in [
        "\"smtp\"",
        "password",
        "\"s3\"",
        "superuserIPs",
        "senderAddress",
    ] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }

    // what the sign-in page may show is checked
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({
            "meta": { "accentColor": "red", "appName": "x".repeat(61) },
            "login": {
                "layout": "fancy",
                "background": "asset",
                "notice": "n".repeat(501),
                "footerLinks": [
                    { "label": "Run", "url": "javascript:alert(1)" },
                    { "label": "", "url": "http://plain.example.com" },
                    { "label": "Ok", "url": "https://a.example.com/\"onmouseover=" },
                    { "label": "Four", "url": "https://four.example.com" }
                ]
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    let errors = &res["data"];
    for path in [
        "/meta/accentColor",
        "/meta/appName",
        "/login/layout",
        "/login/backgroundAsset",
        "/login/notice",
        "/login/footerLinks/0/url",
        "/login/footerLinks/1/label",
        "/login/footerLinks/1/url",
        "/login/footerLinks/2/url",
    ] {
        assert!(errors.pointer(path).is_some(), "{path}: {res}");
    }
    assert!(res["data"]["login"]["footerLinks"].is_object());
    // hex colors and the earlier default still work
    for accent in ["#ff0000", "#1055c9", "sage"] {
        let (status, res) = admin(
            &app,
            &token,
            "PATCH",
            "/api/settings",
            Some(json!({ "meta": { "accentColor": accent } })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{accent}: {res}");
    }

    // once nothing uses it, an image can go
    let (status, _) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "meta": { "favicon": "" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin(
        &app,
        &token,
        "DELETE",
        &format!("/api/branding/assets/{icon_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = raw_request(
        &app,
        "GET",
        &format!("/api/branding/assets/{icon_id}"),
        &[],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    tdb.drop_db().await;
}

#[tokio::test]
async fn assets_go_with_backups() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, kept) = upload(&app, &token, "kept.svg", SVG).await;
    assert_eq!(status, StatusCode::OK);
    let kept = kept["id"].as_str().unwrap().to_string();
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/backups",
        Some(json!({ "name": "brand.zip" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");

    // after the backup: one deleted, one added
    admin(
        &app,
        &token,
        "DELETE",
        &format!("/api/branding/assets/{kept}"),
        None,
    )
    .await;
    let (_, later) = upload(&app, &token, "later.png", PNG).await;
    let later = later["id"].as_str().unwrap().to_string();

    let (status, res) = admin(&app, &token, "POST", "/api/backups/brand.zip/restore", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (_, list) = admin(&app, &token, "GET", "/api/branding/assets", None).await;
    let ids: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![kept.as_str()], "{later} is gone, {kept} is back");
    let (status, _, body) = raw_request(
        &app,
        "GET",
        &format!("/api/branding/assets/{kept}"),
        &[],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, SVG);
    tdb.drop_db().await;
}
