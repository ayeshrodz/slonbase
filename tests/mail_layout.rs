//! Emails (2.0): the layout around every email, the placeholders slonbase adds (`{APP_LOGO}`,
//! `{APP_LOGO_URL}`, `{ACCENT}`, `{RECORD:field}`), the preview, and the test email per
//! collection. See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

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

async fn last_mail(mailer: &MemoryMailer, to: &str) -> slonbase::mail::Message {
    for _ in 0..100 {
        if let Some(m) = mailer
            .sent
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|m| m.to == to)
            .cloned()
        {
            return m;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("no email to {to}");
}

#[tokio::test]
async fn layout_placeholders_preview_and_tests() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let mailer = Arc::new(MemoryMailer::default());
    app.core().unwrap().set_mailer(mailer.clone());

    let (status, users) = admin(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "users", "type": "auth", "createRule": "",
            "fields": [
                { "name": "name", "type": "text" },
                { "name": "secret", "type": "text", "hidden": true }
            ],
            "verificationTemplate": {
                "subject": "Hi {RECORD:name}, verify for {APP_NAME}",
                "body": "<p>{APP_LOGO}</p><p>Hello {RECORD:name} [{RECORD:secret}] [{RECORD:password}] [{RECORD:tokenKey}] [{RECORD:nope}]</p><a style=\"color:{ACCENT}\" href=\"{APP_URL}/_/#/auth/confirm-verification/{TOKEN}\">Verify</a>"
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{users}");
    let (status, res) = request(
        &app,
        "POST",
        "/api/collections/users/records",
        &[],
        Some(json!({
            "email": "ada@example.com", "password": "first-password", "passwordConfirm": "first-password",
            "name": "Ada <b>{TOKEN}</b>", "secret": "s3cr3t-value"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (content_type, body) = multipart(
        &[],
        &[(
            "file",
            "logo.svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        )],
    );
    let auth = format!("Bearer {token}");
    let (_, _, uploaded) = raw_request(
        &app,
        "POST",
        "/api/branding/assets",
        &[
            ("content-type", content_type.as_str()),
            ("authorization", auth.as_str()),
        ],
        body,
    )
    .await;
    let logo: Value = serde_json::from_slice(&uploaded).unwrap();
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "meta": { "appName": "Acme", "appURL": "https://api.example.com/", "accentColor": "ocean", "logo": logo["id"] } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    request(
        &app,
        "POST",
        "/api/collections/users/request-verification",
        &[],
        Some(json!({ "email": "ada@example.com" })),
    )
    .await;
    let mail = last_mail(&mailer, "ada@example.com").await;
    // the subject: placeholders, the record's name escaped (and not a placeholder in turn)
    assert_eq!(
        mail.subject,
        "Hi Ada &lt;b&gt;{TOKEN}&lt;/b&gt;, verify for Acme"
    );
    let html = &mail.html;
    assert!(html.starts_with("<!doctype html>"), "in the layout: {html}");
    assert!(html.contains("Sent by Acme"), "{html}");
    assert!(
        html.contains("Hello Ada &lt;b&gt;{TOKEN}&lt;/b&gt; [] [] [] []"),
        "{html}"
    );
    assert!(!html.contains("s3cr3t-value"), "hidden fields stay out");
    assert!(
        html.contains(&format!(
            "<img src=\"https://api.example.com/api/branding/assets/{}\"",
            logo["id"].as_str().unwrap()
        )),
        "{html}"
    );
    assert!(html.contains("color:#1f63a8"), "the accent: {html}");
    assert!(html.contains("border-top:3px solid #1f63a8"), "{html}");
    assert!(
        html.contains("https://api.example.com/_/#/auth/confirm-verification/ey"),
        "the token in the link: {html}"
    );

    // the preview: what would be sent, with the draft standing in for the saved one
    let (status, preview) = admin(
        &app,
        &token,
        "POST",
        "/api/settings/mail/preview",
        Some(json!({
            "collection": "users", "template": "verification",
            "body": "<p>Draft {APP_NAME} {TOKEN}</p>",
            "layout": { "enabled": true, "html": "<main>{CONTENT}</main>" }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["html"], "<main><p>Draft Acme TEST_TOKEN</p></main>");
    assert_eq!(
        preview["subject"], "Hi , verify for Acme",
        "no record in a preview"
    );
    let (status, _) = request(
        &app,
        "POST",
        "/api/settings/mail/preview",
        &[],
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // the layout must have {CONTENT}; off, emails go as they are
    let (status, res) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "mailLayout": { "html": "<div>no slot</div>" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(res["data"]["mailLayout"]["html"].is_object(), "{res}");
    let (status, _) = admin(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "mailLayout": { "enabled": false } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // the test email, from a collection's own template
    let (status, res) = admin(
        &app,
        &token,
        "POST",
        "/api/settings/test/email",
        Some(
            json!({ "email": "qa@example.com", "template": "verification", "collection": "users" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let mail = last_mail(&mailer, "qa@example.com").await;
    assert!(
        mail.html.starts_with("<p><img"),
        "not in the layout: {}",
        mail.html
    );
    assert!(mail.html.contains("TEST_TOKEN"), "{}", mail.html);
    tdb.drop_db().await;
}
