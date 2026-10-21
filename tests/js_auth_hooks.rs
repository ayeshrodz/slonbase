//! JavaScript auth request hooks (`onRecordAuthRequest`, `onRecordAuthWithPasswordRequest`,
//! ...). See `setup_flow.rs` for how to point the tests at a database.
#![cfg(feature = "js")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::config::HooksConfig;
use slonbase::mail::{MemoryMailer, Message};

const HOOKS: &str = r#"
// sign in with "@name" as well as the email
onRecordAuthWithPasswordRequest((e) => {
  if (e.identity.startsWith("@")) {
    try {
      e.record = $app.findFirstRecordByData("members", "name", e.identity.slice(1));
    } catch (err) {
      e.record = null;
    }
  }
  if (e.identity.includes("banned")) throw new ForbiddenError("Banned.");
  e.next();
}, "members");

onRecordAuthRequest((e) => {
  e.meta = { method: e.authMethod, tokenLength: e.token.length };
  // shown in the response only
  e.record.set("name", e.record.getString("name").toUpperCase());
  e.next();
}, "members");

onRecordAuthRefreshRequest((e) => {
  if (e.record.getString("name") === "frozen") throw new BadRequestError("No refresh.");
  e.next();
}, "members");

onRecordRequestOTPRequest((e) => {
  e.password = "424242";
  e.next();
}, "members");

onRecordAuthWithOTPRequest((e) => {
  if (!e.otp.id || e.otp.recordRef !== e.record.id) throw new BadRequestError("Bad OTP event.");
  e.next();
}, "members");

onRecordRequestPasswordResetRequest((e) => {
  // no reset emails for this domain
  if (e.record.email().endsWith("@blocked.test")) return e.noContent(204);
  e.next();
}, "members");

onRecordRequestEmailChangeRequest((e) => {
  e.newEmail = e.newEmail.toLowerCase();
  e.next();
}, "members");

onRecordConfirmEmailChangeRequest((e) => {
  e.next();
  const log = new Record($app.findCollectionByNameOrId("log"));
  log.set("message", "email changed to " + e.newEmail);
  $app.save(log);
}, "members");

onRecordRequestVerificationRequest((e) => {
  e.json(200, { custom: e.record.email() });
}, "members");
"#;

struct Env {
    app: Arc<slonbase::app::App>,
    mailer: Arc<MemoryMailer>,
    token: String,
    tdb: TestDb,
}

impl Env {
    async fn post(&self, token: Option<&str>, path: &str, body: Value) -> (StatusCode, Value) {
        let bearer = token.map(|t| format!("Bearer {t}"));
        let headers: Vec<(&str, &str)> = bearer
            .as_deref()
            .map(|b| vec![("authorization", b)])
            .unwrap_or_default();
        request(&self.app, "POST", path, &headers, Some(body)).await
    }

    async fn mail_after(&self, skip: usize, to: &str) -> Message {
        for _ in 0..200 {
            if let Some(m) = self
                .mailer
                .sent
                .lock()
                .unwrap()
                .iter()
                .skip(skip)
                .find(|m| m.to == to)
            {
                return m.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("no email sent to {to}");
    }

    fn mail_count(&self) -> usize {
        self.mailer.sent.lock().unwrap().len()
    }
}

/// The token in an email's link (`.../{TOKEN}`).
fn token_from_mail(html: &str) -> String {
    let start = html.find("/confirm-").expect("a confirm link") + 1;
    let link = &html[start..];
    let link = &link[..link.find('"').unwrap()];
    link.rsplit('/').next().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_request_hooks() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let hooks_dir = tempfile::tempdir().unwrap();
    std::fs::write(hooks_dir.path().join("main.pb.js"), HOOKS).unwrap();
    let mut ext = slonbase::Extensions::default();
    slonbase::js::load(
        &HooksConfig {
            dir: hooks_dir.path().to_path_buf(),
            pool_size: 2,
            watch: false,
        },
        &mut ext,
    )
    .await
    .unwrap();
    let (app, token, _dir) = ready_app_with(&tdb, ext).await;
    let mailer = Arc::new(MemoryMailer::default());
    app.core().unwrap().set_mailer(mailer.clone());
    let env = Env {
        app,
        mailer,
        token,
        tdb,
    };
    let admin = env.token.clone();

    for body in [
        json!({
            "name": "members", "type": "auth", "createRule": "",
            "fields": [{ "name": "name", "type": "text" }],
            "otp": { "enabled": true },
        }),
        json!({ "name": "log", "listRule": "", "fields": [{ "name": "message", "type": "text" }] }),
    ] {
        let (status, res) = env.post(Some(&admin), "/api/collections", body).await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    for (email, name) in [
        ("ann@example.com", "ann"),
        ("bob@blocked.test", "frozen"),
        ("banned@example.com", "banned"),
    ] {
        let (status, res) = env
            .post(
                None,
                "/api/collections/members/records",
                json!({ "email": email, "name": name, "password": "password-123", "passwordConfirm": "password-123" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    let sign_in = |identity: &str| {
        let body = json!({ "identity": identity, "password": "password-123" });
        env.post(None, "/api/collections/members/auth-with-password", body)
    };

    // a handler finds the record its own way; onRecordAuthRequest changes the response
    let (status, res) = sign_in("@ann").await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["record"]["email"], "ann@example.com", "{res}");
    assert_eq!(res["record"]["name"], "ANN", "{res}");
    assert_eq!(res["meta"]["method"], "password", "{res}");
    assert!(res["meta"]["tokenLength"].as_u64().unwrap() > 20, "{res}");
    let ann_token = res["token"].as_str().unwrap().to_string();
    // the password still has to match, and unknown names fail like unknown emails
    let (status, _) = env
        .post(
            None,
            "/api/collections/members/auth-with-password",
            json!({ "identity": "@ann", "password": "wrong-password" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = sign_in("@nobody").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, res) = sign_in("banned@example.com").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");
    assert_eq!(res["message"], "Banned.");
    // the change was in the response only
    let (_, list) = request(
        &env.app,
        "GET",
        "/api/collections/members/records?filter=name='ann'",
        &[("authorization", format!("Bearer {admin}").as_str())],
        None,
    )
    .await;
    assert_eq!(list["totalItems"], 1, "{list}");

    // refresh: its hook, then onRecordAuthRequest (without an auth method)
    let (status, res) = env
        .post(
            Some(&ann_token),
            "/api/collections/members/auth-refresh",
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["meta"]["method"], "", "{res}");
    let (_, frozen) = sign_in("bob@blocked.test").await;
    let frozen_token = frozen["token"].as_str().unwrap();
    let (status, res) = env
        .post(
            Some(frozen_token),
            "/api/collections/members/auth-refresh",
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["message"], "No refresh.");

    // OTPs: the handler picks the password, and the sign-in hook sees the OTP
    let skip = env.mail_count();
    let (status, res) = env
        .post(
            None,
            "/api/collections/members/request-otp",
            json!({ "email": "ann@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let otp_id = res["otpId"].as_str().unwrap().to_string();
    let mail = env.mail_after(skip, "ann@example.com").await;
    assert!(mail.html.contains("424242"), "{}", mail.html);
    let (status, res) = env
        .post(
            None,
            "/api/collections/members/auth-with-otp",
            json!({ "otpId": otp_id, "password": "424242" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["meta"]["method"], "otp", "{res}");

    // a handler that answers itself skips the email
    let skip = env.mail_count();
    let (status, _) = env
        .post(
            None,
            "/api/collections/members/request-password-reset",
            json!({ "email": "bob@blocked.test" }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = env
        .post(
            None,
            "/api/collections/members/request-password-reset",
            json!({ "email": "ann@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    env.mail_after(skip, "ann@example.com").await;
    assert!(
        env.mailer.sent.lock().unwrap()[skip..]
            .iter()
            .all(|m| m.to != "bob@blocked.test")
    );
    let (status, res) = env
        .post(
            None,
            "/api/collections/members/request-verification",
            json!({ "email": "ann@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(res["custom"], "ann@example.com");

    // email change: the handler's newEmail is used, and the confirm hook runs after it
    let skip = env.mail_count();
    let (status, res) = env
        .post(
            Some(&ann_token),
            "/api/collections/members/request-email-change",
            json!({ "newEmail": "Ann.New@Example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let mail = env.mail_after(skip, "ann.new@example.com").await;
    let token = token_from_mail(&mail.html);
    let (status, res) = env
        .post(
            None,
            "/api/collections/members/confirm-email-change",
            json!({ "token": token, "password": "password-123" }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (status, _) = sign_in("ann.new@example.com").await;
    assert_eq!(status, StatusCode::OK);
    let (_, log) = request(&env.app, "GET", "/api/collections/log/records", &[], None).await;
    assert_eq!(
        log["items"][0]["message"], "email changed to ann.new@example.com",
        "{log}"
    );

    drop(env.app);
    env.tdb.drop_db().await;
}
