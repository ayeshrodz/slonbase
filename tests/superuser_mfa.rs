//! OTP and MFA for superusers, including the one-time password the `slonbase superuser otp`
//! command issues when emails can't be sent. See `setup_flow.rs` for how to point the tests at
//! a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use common::*;
use slonbase::mail::MemoryMailer;

#[tokio::test]
async fn superuser_otp_and_mfa() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let mailer = Arc::new(MemoryMailer::default());
    app.core().unwrap().set_mailer(mailer.clone());
    let bearer = format!("Bearer {token}");
    let (status, res) = request(
        &app,
        "PATCH",
        "/api/collections/_superusers",
        &[("authorization", bearer.as_str())],
        Some(json!({ "otp": { "enabled": true }, "mfa": { "enabled": true } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    let password_step = || async {
        request(
            &app,
            "POST",
            "/api/collections/_superusers/auth-with-password",
            &[],
            Some(json!({ "identity": ADMIN_EMAIL, "password": ADMIN_PASSWORD })),
        )
        .await
    };
    let otp_step = |otp_id: String, password: String, mfa_id: String| {
        let app = app.clone();
        async move {
            request(
                &app,
                "POST",
                "/api/collections/_superusers/auth-with-otp",
                &[],
                Some(json!({ "otpId": otp_id, "password": password, "mfaId": mfa_id })),
            )
            .await
        }
    };

    // the password alone isn't enough any more
    let (status, res) = password_step().await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{res}");
    let mfa_id = res["mfaId"].as_str().unwrap().to_string();

    // an emailed OTP completes the sign-in
    let (status, res) = request(
        &app,
        "POST",
        "/api/collections/_superusers/request-otp",
        &[],
        Some(json!({ "email": ADMIN_EMAIL })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let otp_id = res["otpId"].as_str().unwrap().to_string();
    let mut code = String::new();
    for _ in 0..100 {
        if let Some(m) = mailer.sent.lock().unwrap().last() {
            let html = &m.html;
            let start = html.find("<strong>").unwrap() + "<strong>".len();
            code = html[start..start + html[start..].find('<').unwrap()].to_string();
        }
        if !code.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let (status, res) = otp_step(otp_id, code, mfa_id).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert!(res["token"].as_str().is_some_and(|t| !t.is_empty()));

    // without email: the command's OTP does the same
    let core = app.core().unwrap();
    let (otp_id, code) = slonbase::records::otp::issue(&core, "_superusers", ADMIN_EMAIL)
        .await
        .unwrap();
    let (_, res) = password_step().await;
    let mfa_id = res["mfaId"].as_str().unwrap().to_string();
    let (status, res) = otp_step(otp_id.clone(), code.clone(), mfa_id.clone()).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    // single use
    let (status, _) = otp_step(otp_id, code, mfa_id).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    assert!(
        slonbase::records::otp::issue(&core, "_superusers", "nobody@example.com")
            .await
            .is_err()
    );
    drop(core);
    drop(app);
    tdb.drop_db().await;
}
