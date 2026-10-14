//! End-to-end tests of auth collections and the auth endpoints against PostgreSQL.
//! See `setup_flow.rs` for how to point them at a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;
use slonbase::mail::MemoryMailer;

struct Env {
    tdb: TestDb,
    app: Arc<App>,
    admin: String,
    mailer: Arc<MemoryMailer>,
    _dir: tempfile::TempDir,
}

impl Env {
    async fn new() -> Option<Self> {
        let tdb = TestDb::create().await?;
        let (app, admin, dir) = ready_app(&tdb).await;
        let mailer = Arc::new(MemoryMailer::default());
        app.core().unwrap().set_mailer(mailer.clone());
        Some(Self {
            tdb,
            app,
            admin,
            mailer,
            _dir: dir,
        })
    }

    async fn call(
        &self,
        token: Option<&str>,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let header = token.map(|t| format!("Bearer {t}"));
        let headers: Vec<(&str, &str)> = header
            .iter()
            .map(|h| ("authorization", h.as_str()))
            .collect();
        request(&self.app, method, uri, &headers, body).await
    }

    async fn admin(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.call(Some(&self.admin.clone()), method, uri, body)
            .await
    }

    async fn ok(&self, token: Option<&str>, method: &str, uri: &str, body: Option<Value>) -> Value {
        let (status, res) = self.call(token, method, uri, body).await;
        assert!(status.is_success(), "{method} {uri} -> {status}: {res}");
        res
    }

    async fn signup(&self, email: &str, password: &str) -> Value {
        self.ok(
            None,
            "POST",
            "/api/collections/users/records",
            Some(json!({ "email": email, "password": password, "passwordConfirm": password })),
        )
        .await
    }

    async fn login(&self, identity: &str, password: &str) -> (StatusCode, Value) {
        self.call(
            None,
            "POST",
            "/api/collections/users/auth-with-password",
            Some(json!({ "identity": identity, "password": password })),
        )
        .await
    }

    async fn token(&self, identity: &str, password: &str) -> String {
        let (status, body) = self.login(identity, password).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["token"].as_str().unwrap().to_string()
    }

    /// Waits for the background mailer and returns the latest email to `to`.
    async fn last_mail(&self, to: &str) -> slonbase::mail::Message {
        self.mail_after(0, to).await
    }

    /// Waits for an email to `to` sent after the first `skip` emails.
    async fn mail_after(&self, skip: usize, to: &str) -> slonbase::mail::Message {
        for _ in 0..100 {
            if let Some(m) = self
                .mailer
                .sent
                .lock()
                .unwrap()
                .iter()
                .skip(skip)
                .rev()
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

    async fn finish(self) {
        drop(self.app);
        self.tdb.drop_db().await;
    }
}

/// The token in an email's action link (`.../confirm-xxx/{TOKEN}"`).
fn token_from_mail(html: &str) -> String {
    let start = html.find("/confirm-").expect("action link");
    let rest = &html[start..];
    let token_start = rest.find('/').unwrap() + 1;
    let rest = &rest[token_start..];
    let rest = &rest[rest.find('/').unwrap() + 1..];
    rest[..rest.find('"').unwrap()].to_string()
}

async fn users_collection(env: &Env, extra: Value) -> Value {
    let mut body = json!({
        "name": "users",
        "type": "auth",
        "fields": [
            { "name": "name", "type": "text" },
            { "name": "role", "type": "select", "values": ["member", "admin"] }
        ],
        "listRule": "id = @request.auth.id",
        "viewRule": "",
        "createRule": "",
        "updateRule": "id = @request.auth.id",
        "deleteRule": "id = @request.auth.id"
    });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    env.ok(
        Some(&env.admin.clone()),
        "POST",
        "/api/collections",
        Some(body),
    )
    .await
}

#[tokio::test]
async fn auth_collection_definition() {
    let Some(env) = Env::new().await else { return };
    let users = users_collection(&env, json!({})).await;

    let names: Vec<&str> = users["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    for system in [
        "id",
        "password",
        "tokenKey",
        "email",
        "emailVisibility",
        "verified",
        "name",
        "role",
    ] {
        assert!(names.contains(&system), "{system} in {names:?}");
    }
    let field = |name: &str| {
        users["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(field("password")["hidden"], true);
    assert_eq!(field("password")["type"], "password");
    assert_eq!(field("tokenKey")["hidden"], true);
    assert_eq!(users["authRule"], "");
    assert_eq!(users["manageRule"], Value::Null);
    assert_eq!(users["passwordAuth"]["identityFields"], json!(["email"]));
    assert_eq!(users["authToken"]["duration"], 432000);

    // the type can't change and system fields can't be removed
    let (status, _) = env
        .admin(
            "PATCH",
            "/api/collections/users",
            Some(json!({ "type": "base" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let without_email: Vec<Value> = users["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["name"] != "email")
        .cloned()
        .collect();
    let (status, _) = env
        .admin(
            "PATCH",
            "/api/collections/users",
            Some(json!({ "fields": without_email })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // password fields only exist in auth collections, identity fields must be unique
    let (status, _) = env
        .admin(
            "POST",
            "/api/collections",
            Some(json!({ "name": "x", "fields": [{ "name": "pw", "type": "password" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = env
        .admin(
            "PATCH",
            "/api/collections/users",
            Some(json!({ "passwordAuth": { "enabled": true, "identityFields": ["name"] } })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["data"]["passwordAuth"]["identityFields"].is_object(),
        "{body}"
    );

    // base collections don't carry auth options
    let base = env
        .ok(
            Some(&env.admin.clone()),
            "POST",
            "/api/collections",
            Some(json!({ "name": "notes" })),
        )
        .await;
    assert!(base.get("authRule").is_none());

    env.finish().await;
}

#[tokio::test]
async fn signup_login_and_rules() {
    let Some(env) = Env::new().await else { return };
    users_collection(&env, json!({})).await;

    // sign up: password rules, confirmation, unique emails, protected fields
    for (body, field) in [
        (
            json!({ "email": "a@example.com", "password": "short", "passwordConfirm": "short" }),
            "password",
        ),
        (
            json!({ "email": "a@example.com", "password": "12345678", "passwordConfirm": "nope" }),
            "passwordConfirm",
        ),
        (
            json!({ "email": "not-an-email", "password": "12345678", "passwordConfirm": "12345678" }),
            "email",
        ),
        (json!({ "email": "a@example.com" }), "password"),
    ] {
        let (status, res) = env
            .call(None, "POST", "/api/collections/users/records", Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
        assert!(res["data"][field].is_object(), "{field}: {res}");
    }
    let alice = env
        .ok(
            None,
            "POST",
            "/api/collections/users/records",
            Some(json!({ "email": "Alice@Example.com", "password": "alice-password", "passwordConfirm": "alice-password",
                         "verified": true, "name": "Alice" })),
        )
        .await;
    assert!(
        alice.get("password").is_none() && alice.get("tokenKey").is_none(),
        "{alice}"
    );
    assert!(
        alice.get("email").is_none(),
        "emails are private by default: {alice}"
    );
    assert_eq!(alice["verified"], false, "guests can't verify themselves");

    let (status, res) = env
        .call(
            None,
            "POST",
            "/api/collections/users/records",
            Some(json!({ "email": "alice@example.COM", "password": "12345678", "passwordConfirm": "12345678" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["data"]["email"]["message"], "Value must be unique.");

    let bob = env.signup("bob@example.com", "bob-password").await;
    let bob_id = bob["id"].as_str().unwrap();

    // sign in
    let (status, _) = env.login("alice@example.com", "wrong-password").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = env.login("nobody@example.com", "alice-password").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, auth) = env.login("ALICE@example.com", "alice-password").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        auth["record"]["email"], "Alice@Example.com",
        "the record sees its own email"
    );
    assert!(auth["record"].get("password").is_none());
    let alice_token = auth["token"].as_str().unwrap().to_string();

    let refreshed = env
        .ok(
            Some(&alice_token),
            "POST",
            "/api/collections/users/auth-refresh",
            None,
        )
        .await;
    assert!(refreshed["token"].is_string());
    let (status, _) = env
        .call(None, "POST", "/api/collections/users/auth-refresh", None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let methods = env
        .ok(None, "GET", "/api/collections/users/auth-methods", None)
        .await;
    assert_eq!(methods["password"]["enabled"], true);

    // @request.auth.id in rules
    let list = env
        .ok(
            Some(&alice_token),
            "GET",
            "/api/collections/users/records",
            None,
        )
        .await;
    assert_eq!(list["totalItems"], 1, "only themselves");
    assert_eq!(list["items"][0]["email"], "Alice@Example.com");
    let list = env
        .ok(None, "GET", "/api/collections/users/records", None)
        .await;
    assert_eq!(list["totalItems"], 0);

    // another user's email is hidden unless they make it visible
    let viewed = env
        .ok(
            Some(&alice_token),
            "GET",
            &format!("/api/collections/users/records/{bob_id}"),
            None,
        )
        .await;
    assert!(viewed.get("email").is_none(), "{viewed}");
    let bob_token = env.token("bob@example.com", "bob-password").await;
    env.ok(
        Some(&bob_token),
        "PATCH",
        &format!("/api/collections/users/records/{bob_id}"),
        Some(json!({ "emailVisibility": true })),
    )
    .await;
    let viewed = env
        .ok(
            Some(&alice_token),
            "GET",
            &format!("/api/collections/users/records/{bob_id}"),
            None,
        )
        .await;
    assert_eq!(viewed["email"], "bob@example.com");
    let (_, viewed) = env
        .admin(
            "GET",
            &format!(
                "/api/collections/users/records/{}",
                alice["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(
        viewed["email"], "Alice@Example.com",
        "superusers see emails"
    );

    // guests can't be filtered into secrets
    let (status, _) = env
        .call(
            Some(&alice_token),
            "GET",
            "/api/collections/users/records?filter=password%3D''",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // relations to the auth user
    let users_id = env
        .ok(
            Some(&env.admin.clone()),
            "GET",
            "/api/collections/users",
            None,
        )
        .await["id"]
        .clone();
    env.ok(
        Some(&env.admin.clone()),
        "POST",
        "/api/collections",
        Some(json!({
            "name": "posts",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "owner", "type": "relation", "collectionId": users_id, "required": true }
            ],
            "listRule": "owner = @request.auth.id",
            "createRule": "@request.auth.id != '' && owner = @request.auth.id",
            "updateRule": "@request.auth.role = 'admin'"
        })),
    )
    .await;
    let alice_id = alice["id"].as_str().unwrap();
    let post = env
        .ok(
            Some(&alice_token),
            "POST",
            "/api/collections/posts/records",
            Some(json!({ "title": "mine", "owner": alice_id })),
        )
        .await;
    let (status, _) = env
        .call(
            Some(&alice_token),
            "POST",
            "/api/collections/posts/records",
            Some(json!({ "title": "x", "owner": bob_id })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "can't create posts for others"
    );
    let (status, _) = env
        .call(
            None,
            "POST",
            "/api/collections/posts/records",
            Some(json!({ "title": "x", "owner": alice_id })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "guests can't create");
    assert_eq!(
        env.ok(
            Some(&bob_token),
            "GET",
            "/api/collections/posts/records",
            None
        )
        .await["totalItems"],
        0
    );
    assert_eq!(
        env.ok(
            Some(&alice_token),
            "GET",
            "/api/collections/posts/records",
            None
        )
        .await["totalItems"],
        1
    );

    // @request.auth.<field>: only admins may update
    let post_uri = format!(
        "/api/collections/posts/records/{}",
        post["id"].as_str().unwrap()
    );
    let (status, _) = env
        .call(
            Some(&alice_token),
            "PATCH",
            &post_uri,
            Some(json!({ "title": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    env.admin(
        "PATCH",
        &format!("/api/collections/users/records/{alice_id}"),
        Some(json!({ "role": "admin" })),
    )
    .await;
    let updated = env
        .ok(
            Some(&alice_token),
            "PATCH",
            &post_uri,
            Some(json!({ "title": "edited" })),
        )
        .await;
    assert_eq!(updated["title"], "edited");

    env.finish().await;
}

#[tokio::test]
async fn passwords_emails_and_managers() {
    let Some(env) = Env::new().await else { return };
    users_collection(
        &env,
        json!({ "manageRule": "@request.auth.role = 'admin'" }),
    )
    .await;
    let user = env.signup("user@example.com", "user-password").await;
    let admin = env.signup("boss@example.com", "boss-password").await;
    let user_uri = format!(
        "/api/collections/users/records/{}",
        user["id"].as_str().unwrap()
    );
    env.admin(
        "PATCH",
        &format!(
            "/api/collections/users/records/{}",
            admin["id"].as_str().unwrap()
        ),
        Some(json!({ "role": "admin" })),
    )
    .await;

    let user_token = env.token("user@example.com", "user-password").await;

    // changing the password needs the old one, and signs out old sessions
    let (status, res) = env
        .call(
            Some(&user_token),
            "PATCH",
            &user_uri,
            Some(json!({ "password": "new-password", "passwordConfirm": "new-password" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(res["data"]["oldPassword"].is_object());
    env.ok(
        Some(&user_token),
        "PATCH",
        &user_uri,
        Some(json!({ "password": "new-password", "passwordConfirm": "new-password", "oldPassword": "user-password" })),
    )
    .await;
    let (status, _) = env
        .call(
            Some(&user_token),
            "GET",
            "/api/collections/users/records",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let list = env
        .ok(
            Some(&user_token),
            "GET",
            "/api/collections/users/records",
            None,
        )
        .await;
    assert_eq!(
        list["totalItems"], 0,
        "the old token no longer authenticates"
    );
    let (status, _) = env.login("user@example.com", "user-password").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let user_token = env.token("user@example.com", "new-password").await;

    // users can't change their email or verify themselves directly
    let (status, res) = env
        .call(
            Some(&user_token),
            "PATCH",
            &user_uri,
            Some(json!({ "email": "other@example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(res["data"]["email"].is_object());
    let res = env
        .ok(
            Some(&user_token),
            "PATCH",
            &user_uri,
            Some(json!({ "verified": true, "name": "me" })),
        )
        .await;
    assert_eq!(res["verified"], false);
    assert_eq!(res["name"], "me");

    // managers can, without the old password (the manage rule allows them to update via
    // the update rule only if it lets them, so use a superuser-free manager path)
    env.admin(
        "PATCH",
        "/api/collections/users",
        Some(json!({ "updateRule": "id = @request.auth.id || @request.auth.role = 'admin'" })),
    )
    .await;
    let boss_token = env.token("boss@example.com", "boss-password").await;
    let res = env
        .ok(
            Some(&boss_token),
            "PATCH",
            &user_uri,
            Some(json!({ "email": "moved@example.com", "verified": true, "password": "reset-by-boss", "passwordConfirm": "reset-by-boss" })),
        )
        .await;
    assert_eq!(res["verified"], true);
    assert_eq!(res["email"], "moved@example.com", "managers see the email");
    env.token("moved@example.com", "reset-by-boss").await;

    env.finish().await;
}

#[tokio::test]
async fn verification_reset_and_email_change_flows() {
    let Some(env) = Env::new().await else { return };
    users_collection(&env, json!({ "authRule": "verified = true" })).await;
    env.signup("me@example.com", "first-password").await;

    // the auth rule blocks unverified users
    let (status, _) = env.login("me@example.com", "first-password").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // verification
    env.ok(
        None,
        "POST",
        "/api/collections/users/request-verification",
        Some(json!({ "email": "unknown@example.com" })),
    )
    .await;
    env.ok(
        None,
        "POST",
        "/api/collections/users/request-verification",
        Some(json!({ "email": "ME@example.com" })),
    )
    .await;
    let mail = env.last_mail("me@example.com").await;
    assert!(mail.subject.contains("Verify"), "{}", mail.subject);
    let token = token_from_mail(&mail.html);

    // a verification token is not an auth token
    let list = env
        .ok(
            Some(&token),
            "GET",
            "/api/collections/users/records?filter=id!=''",
            None,
        )
        .await;
    assert_eq!(list["totalItems"], 0);
    let (status, _) = env
        .call(
            None,
            "POST",
            "/api/collections/users/confirm-verification",
            Some(json!({ "token": "bad" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    env.ok(
        None,
        "POST",
        "/api/collections/users/confirm-verification",
        Some(json!({ "token": token })),
    )
    .await;
    let session = env.token("me@example.com", "first-password").await;

    // password reset
    let before = env.mail_count();
    env.ok(
        None,
        "POST",
        "/api/collections/users/request-password-reset",
        Some(json!({ "email": "me@example.com" })),
    )
    .await;
    let mail = env.mail_after(before, "me@example.com").await;
    assert!(mail.subject.contains("Reset"), "{}", mail.subject);
    let reset = token_from_mail(&mail.html);
    let (status, res) = env
        .call(
            None,
            "POST",
            "/api/collections/users/confirm-password-reset",
            Some(json!({ "token": reset, "password": "second-password", "passwordConfirm": "different" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(res["data"]["passwordConfirm"].is_object());
    env.ok(
        None,
        "POST",
        "/api/collections/users/confirm-password-reset",
        Some(json!({ "token": reset, "password": "second-password", "passwordConfirm": "second-password" })),
    )
    .await;
    let (status, _) = env
        .call(
            None,
            "POST",
            "/api/collections/users/confirm-password-reset",
            Some(json!({ "token": reset, "password": "third-password", "passwordConfirm": "third-password" })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "reset tokens are single-use"
    );
    let (status, _) = env
        .call(
            Some(&session),
            "POST",
            "/api/collections/users/auth-refresh",
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "old sessions are signed out"
    );
    let (status, _) = env.login("me@example.com", "first-password").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let session = env.token("me@example.com", "second-password").await;

    // email change
    env.signup("taken@example.com", "taken-password").await;
    let (status, res) = env
        .call(
            Some(&session),
            "POST",
            "/api/collections/users/request-email-change",
            Some(json!({ "newEmail": "taken@example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(res["data"]["newEmail"].is_object());
    let (status, _) = env
        .call(
            None,
            "POST",
            "/api/collections/users/request-email-change",
            Some(json!({ "newEmail": "new@example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    env.ok(
        Some(&session),
        "POST",
        "/api/collections/users/request-email-change",
        Some(json!({ "newEmail": "new@example.com" })),
    )
    .await;
    let change = token_from_mail(&env.last_mail("new@example.com").await.html);
    let (status, _) = env
        .call(
            None,
            "POST",
            "/api/collections/users/confirm-email-change",
            Some(json!({ "token": change, "password": "wrong" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    env.ok(
        None,
        "POST",
        "/api/collections/users/confirm-email-change",
        Some(json!({ "token": change, "password": "second-password" })),
    )
    .await;
    let (status, _) = env.login("me@example.com", "second-password").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    env.token("new@example.com", "second-password").await;

    env.finish().await;
}

#[tokio::test]
async fn impersonation_and_settings() {
    let Some(env) = Env::new().await else { return };
    users_collection(&env, json!({})).await;
    let user = env.signup("user@example.com", "user-password").await;
    let id = user["id"].as_str().unwrap();

    let (status, _) = env
        .call(
            None,
            "POST",
            &format!("/api/collections/users/impersonate/{id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let res = env
        .ok(
            Some(&env.admin.clone()),
            "POST",
            &format!("/api/collections/users/impersonate/{id}"),
            Some(json!({ "duration": 60 })),
        )
        .await;
    let token = res["token"].as_str().unwrap().to_string();
    let list = env
        .ok(Some(&token), "GET", "/api/collections/users/records", None)
        .await;
    assert_eq!(list["totalItems"], 1, "the token authenticates as the user");
    let (status, _) = env
        .call(
            Some(&token),
            "POST",
            "/api/collections/users/auth-refresh",
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "impersonation tokens aren't refreshable"
    );

    // settings
    let (status, _) = env.call(None, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let settings = env
        .ok(
            Some(&env.admin.clone()),
            "PATCH",
            "/api/settings",
            Some(
                json!({ "meta": { "appName": "Acme", "appURL": "https://acme.test" },
                         "smtp": { "host": "mail.acme.test", "password": "secret" } }),
            ),
        )
        .await;
    assert_eq!(settings["meta"]["appName"], "Acme");
    assert_eq!(
        settings["smtp"]["password"], "",
        "the SMTP password is never returned"
    );
    let (status, _) = env
        .admin(
            "PATCH",
            "/api/settings",
            Some(json!({ "meta": { "appURL": "not a url" } })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // emails use the settings
    env.ok(
        None,
        "POST",
        "/api/collections/users/request-password-reset",
        Some(json!({ "email": "user@example.com" })),
    )
    .await;
    let mail = env.last_mail("user@example.com").await;
    assert!(mail.subject.contains("Acme"), "{}", mail.subject);
    assert!(
        mail.html
            .contains("https://acme.test/_/#/auth/confirm-password-reset/"),
        "{}",
        mail.html
    );

    // the settings survive a restart and the SMTP password was kept
    let stored: Value = env
        .tdb
        .pool()
        .get()
        .await
        .unwrap()
        .query_one(
            "SELECT value FROM _slonbase.params WHERE key = 'settings'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored["smtp"]["password"], "secret");
    env.admin(
        "PATCH",
        "/api/settings",
        Some(json!({ "smtp": { "port": 2525 } })),
    )
    .await;
    let stored: Value = env
        .tdb
        .pool()
        .get()
        .await
        .unwrap()
        .query_one(
            "SELECT value FROM _slonbase.params WHERE key = 'settings'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored["smtp"]["password"], "secret", "kept when not sent");
    assert_eq!(stored["smtp"]["port"], 2525);

    env.finish().await;
}

/// `@request.auth.*` values (text, bool, select, missing) compared with every field type
/// must never produce SQL that PostgreSQL rejects.
#[tokio::test]
async fn auth_values_in_filters_never_error() {
    let Some(env) = Env::new().await else { return };
    users_collection(&env, json!({ "listRule": "" })).await;
    let user = env.signup("me@example.com", "my-password").await;
    env.admin(
        "PATCH",
        &format!(
            "/api/collections/users/records/{}",
            user["id"].as_str().unwrap()
        ),
        Some(json!({ "role": "admin", "name": "Me", "verified": true })),
    )
    .await;
    let token = env.token("me@example.com", "my-password").await;

    let left = [
        "@request.auth.id",
        "@request.auth.email",
        "@request.auth.verified",
        "@request.auth.role",
        "@request.auth.name",
        "@request.auth.collectionName",
        "@request.auth.missing",
        "@request.auth.emailVisibility",
        "@request.auth.password",
        "@request.auth.created",
    ];
    let right = [
        "id",
        "email",
        "verified",
        "role",
        "name",
        "emailVisibility",
        "'x'",
        "5",
        "true",
        "null",
    ];
    let operators = ["=", "!=", ">", "<=", "~", "!~", "?=", "?~"];
    for l in left {
        for r in right {
            for op in operators {
                let filter = format!("{l} {op} {r}");
                let encoded: String = filter
                    .bytes()
                    .map(|b| {
                        if b.is_ascii_alphanumeric() {
                            (b as char).to_string()
                        } else {
                            format!("%{b:02X}")
                        }
                    })
                    .collect();
                let (status, body) = env
                    .call(
                        Some(&token),
                        "GET",
                        &format!("/api/collections/users/records?filter={encoded}"),
                        None,
                    )
                    .await;
                assert!(
                    status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
                    "{filter:?} -> {status}: {body}"
                );
            }
        }
    }

    // and they evaluate to the caller's values
    let list = env
        .ok(
            Some(&token),
            "GET",
            "/api/collections/users/records?filter=role%3D%40request.auth.role",
            None,
        )
        .await;
    assert_eq!(list["totalItems"], 1);
    let list = env
        .ok(
            Some(&token),
            "GET",
            "/api/collections/users/records?filter=%40request.auth.password%3Dnull",
            None,
        )
        .await;
    assert_eq!(list["totalItems"], 1, "secrets resolve to null");

    env.finish().await;
}

#[tokio::test]
async fn new_device_alerts() {
    let Some(env) = Env::new().await else { return };
    let users = users_collection(&env, json!({})).await;
    assert_eq!(users["authAlert"]["enabled"], true, "{users}");
    assert!(
        users["authAlert"]["emailTemplate"]["body"]
            .as_str()
            .unwrap()
            .contains("{ALERT_INFO}")
    );
    let alice = env.signup("alice@example.com", "alice-password").await;

    let login_from = |agent: &'static str| {
        let env = &env;
        async move {
            let (status, body) = request(
                &env.app,
                "POST",
                "/api/collections/users/auth-with-password",
                &[("user-agent", agent)],
                Some(json!({ "identity": "alice@example.com", "password": "alice-password" })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            // alerts are sent in the background
            tokio::time::sleep(Duration::from_millis(150)).await;
            env.mail_count()
        }
    };
    let alerts = |env: &Env| {
        env.mailer
            .sent
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.subject == "Login from a new location")
            .cloned()
            .collect::<Vec<_>>()
    };

    // the first sign-in and known devices send nothing
    login_from("Laptop/1.0").await;
    login_from("Laptop/1.0").await;
    assert!(alerts(&env).is_empty());

    // a new device does, once
    login_from("Phone/2.0").await;
    let sent = alerts(&env);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, "alice@example.com");
    assert!(sent[0].html.contains("Phone/2.0"), "{}", sent[0].html);
    login_from("Phone/2.0").await;
    login_from("Laptop/1.0").await;
    assert_eq!(alerts(&env).len(), 1);

    // a password change forgets the known devices: the next sign-in is like the first
    let id = alice["id"].as_str().unwrap();
    env.ok(
        Some(&env.admin.clone()),
        "PATCH",
        &format!("/api/collections/users/records/{id}"),
        Some(json!({ "password": "alice-password", "passwordConfirm": "alice-password" })),
    )
    .await;
    login_from("Tablet/3.0").await;
    assert_eq!(alerts(&env).len(), 1);
    login_from("Laptop/1.0").await;
    assert_eq!(alerts(&env).len(), 2);

    // turned off
    env.ok(
        Some(&env.admin.clone()),
        "PATCH",
        "/api/collections/users",
        Some(json!({ "authAlert": { "enabled": false, "emailTemplate": { "subject": "", "body": "" } } })),
    )
    .await;
    let (_, users) = env.admin("GET", "/api/collections/users", None).await;
    assert_eq!(
        users["authAlert"]["emailTemplate"]["subject"], "Login from a new location",
        "a blank template is the default one"
    );
    login_from("Desktop/4.0").await;
    assert_eq!(alerts(&env).len(), 2);

    // deleting the record forgets its devices
    env.ok(
        Some(&env.admin.clone()),
        "DELETE",
        &format!("/api/collections/users/records/{id}"),
        None,
    )
    .await;
    let client = env.app.core().unwrap().pool.get().await.unwrap();
    let left: i64 = client
        .query_one(
            "SELECT count(*) FROM _slonbase.auth_origins WHERE record_id = $1",
            &[&id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(left, 0);

    env.finish().await;
}
