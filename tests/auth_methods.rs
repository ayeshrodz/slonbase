//! End-to-end tests of OTP, MFA and OAuth2 sign-in against PostgreSQL, with a mock
//! OAuth2 provider. See `setup_flow.rs` for how to point them at a database.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use common::*;
use slonbase::app::App;
use slonbase::mail::MemoryMailer;
use slonbase::server;

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

    async fn ok(&self, token: Option<&str>, method: &str, uri: &str, body: Option<Value>) -> Value {
        let (status, res) = self.call(token, method, uri, body).await;
        assert!(status.is_success(), "{method} {uri} -> {status}: {res}");
        res
    }

    async fn admin(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.call(Some(&self.admin.clone()), method, uri, body)
            .await
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

    async fn login(&self, body: Value) -> (StatusCode, Value) {
        self.call(
            None,
            "POST",
            "/api/collections/users/auth-with-password",
            Some(body),
        )
        .await
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

    async fn count(&self, sql: &str) -> i64 {
        let client = self.tdb.pool().get().await.unwrap();
        client.query_one(sql, &[]).await.unwrap().get(0)
    }

    async fn finish(self) {
        drop(self.app);
        self.tdb.drop_db().await;
    }
}

async fn users_collection(env: &Env, extra: Value) -> Value {
    let mut body = json!({
        "name": "users",
        "type": "auth",
        "fields": [
            { "name": "name", "type": "text" },
            { "name": "role", "type": "text" },
            { "name": "avatar", "type": "file", "mimeTypes": ["image/png"] }
        ],
        "listRule": "id = @request.auth.id",
        "viewRule": "id = @request.auth.id",
        "createRule": "",
        "updateRule": "id = @request.auth.id",
        "deleteRule": "id = @request.auth.id"
    });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    let (status, res) = env.admin("POST", "/api/collections", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    res
}

/// The digits of the OTP in an email.
fn otp_from_mail(html: &str) -> String {
    let start = html.find("<strong>").expect("OTP in the email") + "<strong>".len();
    html[start..start + html[start..].find('<').unwrap()].to_string()
}

async fn request_otp(env: &Env, email: &str) -> (String, String) {
    let skip = env.mail_count();
    let res = env
        .ok(
            None,
            "POST",
            "/api/collections/users/request-otp",
            Some(json!({ "email": email })),
        )
        .await;
    let otp_id = res["otpId"].as_str().unwrap().to_string();
    let mail = env.mail_after(skip, email).await;
    (otp_id, otp_from_mail(&mail.html))
}

#[tokio::test]
async fn otp_and_mfa() {
    let Some(env) = Env::new().await else {
        return;
    };
    users_collection(&env, json!({})).await;

    // disabled by default
    let (status, _) = env
        .call(
            None,
            "POST",
            "/api/collections/users/request-otp",
            Some(json!({ "email": "a@example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let methods = env
        .ok(None, "GET", "/api/collections/users/auth-methods", None)
        .await;
    assert_eq!(methods["otp"], json!({ "enabled": false, "duration": 0 }));

    // MFA needs two methods
    let (status, res) = env
        .admin(
            "PATCH",
            "/api/collections/users",
            Some(json!({ "mfa": { "enabled": true } })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res.pointer("/data/mfa/enabled").is_some(), "{res}");
    let (status, res) = env
        .admin(
            "PATCH",
            "/api/collections/users",
            Some(json!({ "otp": { "enabled": true, "length": 2 } })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res.pointer("/data/otp/length").is_some(), "{res}");
    env.ok(
        Some(&env.admin.clone()),
        "PATCH",
        "/api/collections/users",
        Some(json!({ "otp": { "enabled": true, "length": 6, "duration": 300 } })),
    )
    .await;
    let methods = env
        .ok(None, "GET", "/api/collections/users/auth-methods", None)
        .await;
    assert_eq!(methods["otp"], json!({ "enabled": true, "duration": 300 }));

    let user = env.signup("a@example.com", "password123").await;
    assert_eq!(user["verified"], false);

    // unknown emails get an id too, but no email
    let skip = env.mail_count();
    let res = env
        .ok(
            None,
            "POST",
            "/api/collections/users/request-otp",
            Some(json!({ "email": "nobody@example.com" })),
        )
        .await;
    assert_eq!(res["otpId"].as_str().unwrap().len(), 15);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(env.mail_count(), skip);
    let (status, _) = env
        .call(
            None,
            "POST",
            "/api/collections/users/auth-with-otp",
            Some(json!({ "otpId": res["otpId"], "password": "123456" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // sign in with an OTP
    let (otp_id, code) = request_otp(&env, "a@example.com").await;
    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_digit()));
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let (status, res) = env
        .call(
            None,
            "POST",
            "/api/collections/users/auth-with-otp",
            Some(json!({ "otpId": otp_id, "password": wrong })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["message"], "Invalid or expired OTP.");
    let res = env
        .ok(
            None,
            "POST",
            "/api/collections/users/auth-with-otp",
            Some(json!({ "otpId": otp_id, "password": code })),
        )
        .await;
    assert_eq!(res["record"]["email"], "a@example.com");
    assert_eq!(res["record"]["verified"], true, "the OTP proves the email");
    assert!(res["token"].is_string());
    // single use
    let (status, _) = env
        .call(
            None,
            "POST",
            "/api/collections/users/auth-with-otp",
            Some(json!({ "otpId": otp_id, "password": code })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // too many wrong attempts invalidate an OTP
    let (otp_id, code) = request_otp(&env, "a@example.com").await;
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for _ in 0..5 {
        env.call(
            None,
            "POST",
            "/api/collections/users/auth-with-otp",
            Some(json!({ "otpId": otp_id, "password": wrong })),
        )
        .await;
    }
    let (status, _) = env
        .call(
            None,
            "POST",
            "/api/collections/users/auth-with-otp",
            Some(json!({ "otpId": otp_id, "password": code })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // MFA: the first method answers 401 with an mfaId, the second one signs in
    env.ok(
        Some(&env.admin.clone()),
        "PATCH",
        "/api/collections/users",
        Some(json!({ "mfa": { "enabled": true, "duration": 120 } })),
    )
    .await;
    let methods = env
        .ok(None, "GET", "/api/collections/users/auth-methods", None)
        .await;
    assert_eq!(methods["mfa"], json!({ "enabled": true, "duration": 120 }));

    let (status, res) = env
        .login(json!({ "identity": "a@example.com", "password": "password123" }))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{res}");
    let mfa_id = res["mfaId"].as_str().unwrap().to_string();
    assert!(res.get("token").is_none());

    // the same method again isn't a second factor
    let (status, res) = env
        .login(json!({ "identity": "a@example.com", "password": "password123", "mfaId": mfa_id }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        res["message"],
        "A different authentication method is required."
    );

    // another record can't use it
    let other = env.signup("b@example.com", "password123").await;
    let (_, code_b) = request_otp(&env, "b@example.com").await;
    let otp_b = env
        .count(&format!(
            "SELECT count(*) FROM _slonbase.otps WHERE record_id = '{}'",
            other["id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(otp_b, 1);
    let (otp_id_b, code_b2) = request_otp(&env, "b@example.com").await;
    let _ = code_b;
    let (status, res) = env
        .call(
            None,
            "POST",
            &format!("/api/collections/users/auth-with-otp?mfaId={mfa_id}"),
            Some(json!({ "otpId": otp_id_b, "password": code_b2 })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["message"], "Invalid MFA session.");

    // the OTP completes the sign-in (mfaId in the query string works too)
    let (otp_id, code) = request_otp(&env, "a@example.com").await;
    let res = env
        .ok(
            None,
            "POST",
            &format!("/api/collections/users/auth-with-otp?mfaId={mfa_id}"),
            Some(json!({ "otpId": otp_id, "password": code })),
        )
        .await;
    let token = res["token"].as_str().unwrap().to_string();
    let me = env
        .ok(
            Some(&token),
            "GET",
            &format!(
                "/api/collections/users/records/{}",
                user["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(me["email"], "a@example.com");
    // the MFA session is single use
    let (status, res) = env
        .login(json!({ "identity": "a@example.com", "password": "password123", "mfaId": mfa_id }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["message"], "Invalid or expired MFA session.");

    // the MFA rule limits which records need it
    env.ok(
        Some(&env.admin.clone()),
        "PATCH",
        "/api/collections/users",
        Some(json!({ "mfa": { "enabled": true, "rule": "role = 'admin'" } })),
    )
    .await;
    let (status, res) = env
        .login(json!({ "identity": "a@example.com", "password": "password123" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let client = env.tdb.pool().get().await.unwrap();
    client
        .execute(
            "UPDATE slonbase.users SET role = 'admin' WHERE email = 'a@example.com'",
            &[],
        )
        .await
        .unwrap();
    let (status, _) = env
        .login(json!({ "identity": "a@example.com", "password": "password123" }))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, res) = env
        .admin(
            "PATCH",
            "/api/collections/users",
            Some(json!({ "mfa": { "enabled": true, "rule": "nope ~" } })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");

    // deleting a record removes its OTPs and MFA sessions
    let pending = |id: &str| {
        format!(
            "SELECT (SELECT count(*) FROM _slonbase.otps WHERE record_id = '{id}')
                  + (SELECT count(*) FROM _slonbase.mfas WHERE record_id = '{id}')"
        )
    };
    let id = user["id"].as_str().unwrap();
    assert!(env.count(&pending(id)).await > 0);
    env.ok(
        Some(&env.admin.clone()),
        "DELETE",
        &format!("/api/collections/users/records/{id}"),
        None,
    )
    .await;
    assert_eq!(env.count(&pending(id)).await, 0);

    drop(client);
    env.finish().await;
}

// ---------------------------------------------------------------------------
// OAuth2

/// A mock OpenID Connect provider. Codes are `code-{user}`; each user's info is set by
/// the test.
#[derive(Clone, Default)]
struct Provider {
    users: Arc<Mutex<HashMap<String, Value>>>,
    /// The token requests received.
    requests: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

impl Provider {
    async fn start() -> (Self, String) {
        let provider = Self::default();
        let router = Router::new()
            .route("/token", post(token))
            .route("/userinfo", get(userinfo))
            .route(
                "/avatar.png",
                get(|| async {
                    let mut png = Vec::new();
                    image_png(&mut png);
                    ([("content-type", "image/png")], png)
                }),
            )
            .with_state(provider.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (provider, url)
    }

    fn set_user(&self, name: &str, info: Value) {
        self.users.lock().unwrap().insert(name.to_string(), info);
    }
}

/// A 1x1 PNG.
fn image_png(out: &mut Vec<u8>) {
    out.extend_from_slice(&[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]);
}

async fn token(
    State(provider): State<Provider>,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    provider.requests.lock().unwrap().push(form.clone());
    let user = form
        .get("code")
        .and_then(|c| c.strip_prefix("code-"))
        .unwrap_or_default();
    let valid = form.get("client_secret").map(String::as_str) == Some("s3cret")
        && form.get("grant_type").map(String::as_str) == Some("authorization_code")
        && provider.users.lock().unwrap().contains_key(user);
    if !valid {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "access_token": format!("token-{user}"),
            "refresh_token": "refresh",
            "token_type": "Bearer",
            "expires_in": 3600,
        })),
    )
}

async fn userinfo(
    State(provider): State<Provider>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    let user = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer token-"))
        .unwrap_or_default()
        .to_string();
    match provider.users.lock().unwrap().get(&user) {
        Some(info) => (StatusCode::OK, Json(info.clone())),
        None => (StatusCode::UNAUTHORIZED, Json(json!({}))),
    }
}

async fn oauth2(env: &Env, token: Option<&str>, user: &str, extra: Value) -> (StatusCode, Value) {
    let mut body = json!({
        "provider": "oidc",
        "code": format!("code-{user}"),
        "codeVerifier": "verifier",
        "redirectURL": "http://localhost/callback",
    });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    env.call(
        token,
        "POST",
        "/api/collections/users/auth-with-oauth2",
        Some(body),
    )
    .await
}

#[tokio::test]
async fn oauth2_sign_in() {
    let Some(env) = Env::new().await else {
        return;
    };
    let (provider, url) = Provider::start().await;
    let oauth2_options = json!({
        "enabled": true,
        "providers": [{
            "name": "oidc",
            "displayName": "Test ID",
            "clientId": "client-1",
            "clientSecret": "s3cret",
            "authURL": format!("{url}/authorize"),
            "tokenURL": format!("{url}/token"),
            "userInfoURL": format!("{url}/userinfo"),
        }],
        "mappedFields": { "name": "name", "avatarURL": "avatar" },
    });

    // invalid options
    let (status, res) = env
        .admin(
            "POST",
            "/api/collections",
            Some(json!({
                "name": "bad", "type": "auth",
                "oauth2": { "enabled": true, "providers": [
                    { "name": "nope", "clientId": "x", "clientSecret": "y" },
                    { "name": "oidc2", "clientId": "x", "clientSecret": "y" },
                ], "mappedFields": { "name": "missing" } },
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    for key in [
        "/data/oauth2/providers/0/name",
        "/data/oauth2/providers/1/authURL",
        "/data/oauth2/mappedFields/name",
    ] {
        assert!(res.pointer(key).is_some(), "{key}: {res}");
    }

    let created = users_collection(&env, json!({ "oauth2": oauth2_options })).await;
    // client secrets are never returned, and kept when an update leaves them empty
    assert_eq!(created["oauth2"]["providers"][0]["clientSecret"], "");
    assert_eq!(created["oauth2"]["providers"][0]["clientId"], "client-1");
    let mut options = created["oauth2"].clone();
    options["providers"][0]["displayName"] = json!("Test Identity");
    let (status, updated) = env
        .admin(
            "PATCH",
            "/api/collections/users",
            Some(json!({ "oauth2": options })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["oauth2"]["providers"][0]["clientSecret"], "");
    let listed = env
        .ok(Some(&env.admin.clone()), "GET", "/api/collections", None)
        .await;
    assert!(!listed.to_string().contains("s3cret"));

    // auth methods
    let methods = env
        .ok(None, "GET", "/api/collections/users/auth-methods", None)
        .await;
    assert_eq!(methods["oauth2"]["enabled"], true);
    let entry = &methods["oauth2"]["providers"][0];
    assert_eq!(entry["name"], "oidc");
    assert_eq!(entry["displayName"], "Test Identity");
    let auth_url = entry["authURL"].as_str().unwrap();
    assert!(auth_url.starts_with(&format!("{url}/authorize?client_id=client-1&")));
    assert!(auth_url.ends_with("&redirect_uri="));
    assert!(auth_url.contains("scope=openid+email+profile"));
    assert_eq!(entry["codeChallengeMethod"], "S256");
    assert_eq!(entry["state"].as_str().unwrap().len(), 30);
    assert!(!methods.to_string().contains("s3cret"));

    // a new account signs up, with mapped fields and a downloaded avatar
    provider.set_user(
        "ann",
        json!({ "sub": "p-ann", "email": "ann@example.com", "email_verified": true,
                "name": "Ann", "picture": format!("{url}/avatar.png") }),
    );
    let (status, res) = oauth2(
        &env,
        None,
        "ann",
        json!({ "createData": { "role": "member" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["meta"]["isNew"], true);
    assert_eq!(res["meta"]["id"], "p-ann");
    assert_eq!(res["meta"]["accessToken"], "token-ann");
    assert_eq!(res["meta"]["refreshToken"], "refresh");
    assert!(res["meta"]["expiry"].as_str().unwrap().ends_with('Z'));
    assert_eq!(res["meta"]["rawUser"]["name"], "Ann");
    let ann = res["record"].clone();
    assert_eq!(ann["email"], "ann@example.com");
    assert_eq!(ann["verified"], true);
    assert_eq!(ann["name"], "Ann");
    assert_eq!(ann["role"], "member");
    assert!(ann["avatar"].as_str().unwrap().ends_with(".png"), "{ann}");
    let token = res["token"].as_str().unwrap().to_string();
    env.ok(
        Some(&token),
        "GET",
        &format!(
            "/api/collections/users/records/{}",
            ann["id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    let request = provider.requests.lock().unwrap().last().unwrap().clone();
    assert_eq!(request["client_id"], "client-1");
    assert_eq!(request["code_verifier"], "verifier");
    assert_eq!(request["redirect_uri"], "http://localhost/callback");

    // signing in again finds the linked record
    provider.set_user(
        "ann",
        json!({ "sub": "p-ann", "email": "ann.new@example.com", "email_verified": true }),
    );
    let (status, res) = oauth2(&env, None, "ann", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["meta"]["isNew"], false);
    assert_eq!(res["record"]["id"], ann["id"]);
    assert_eq!(res["record"]["email"], "ann@example.com");

    // a bad code or a provider error
    let (status, res) = oauth2(&env, None, "nobody", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["message"], "Failed to fetch OAuth2 token.");
    let (status, res) = oauth2(&env, None, "ann", json!({ "provider": "github" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(res["data"]["provider"].is_object(), "{res}");
    let (status, _) = oauth2(&env, None, "ann", json!({ "redirectURL": "" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // an unverified record with the provider's email: someone may have signed up with an
    // address they don't own, so its password stops working
    let squatter = env.signup("bob@example.com", "password123").await;
    provider.set_user(
        "bob",
        json!({ "sub": "p-bob", "email": "bob@example.com", "email_verified": true }),
    );
    let (status, res) = oauth2(&env, None, "bob", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["meta"]["isNew"], false);
    assert_eq!(res["record"]["id"], squatter["id"]);
    assert_eq!(res["record"]["verified"], true);
    let (status, _) = env
        .login(json!({ "identity": "bob@example.com", "password": "password123" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // a verified record with the provider's email keeps its password
    let carol = env.signup("carol@example.com", "password123").await;
    let client = env.tdb.pool().get().await.unwrap();
    client
        .execute(
            "UPDATE slonbase.users SET verified = TRUE WHERE email = 'carol@example.com'",
            &[],
        )
        .await
        .unwrap();
    provider.set_user(
        "carol",
        json!({ "sub": "p-carol", "email": "carol@example.com", "email_verified": true }),
    );
    let (_, res) = oauth2(&env, None, "carol", json!({})).await;
    assert_eq!(res["record"]["id"], carol["id"]);
    let (status, _) = env
        .login(json!({ "identity": "carol@example.com", "password": "password123" }))
        .await;
    assert_eq!(status, StatusCode::OK);

    // unverified provider emails are ignored: no link by email, and the sign-up needs one
    provider.set_user(
        "eve",
        json!({ "sub": "p-eve", "email": "carol@example.com", "email_verified": false }),
    );
    let (status, res) = oauth2(&env, None, "eve", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"]["email"].is_object(), "{res}");
    let (status, res) = oauth2(
        &env,
        None,
        "eve",
        json!({ "createData": { "email": "eve@example.com" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_ne!(res["record"]["id"], carol["id"]);
    assert_eq!(res["record"]["email"], "eve@example.com");
    assert_eq!(res["record"]["verified"], false);

    // a signed-in record links another provider account to itself
    let dave = env.signup("dave@example.com", "password123").await;
    let (_, login) = env
        .login(json!({ "identity": "dave@example.com", "password": "password123" }))
        .await;
    let dave_token = login["token"].as_str().unwrap().to_string();
    provider.set_user(
        "dave",
        json!({ "sub": "p-dave", "email": "dave.other@example.com", "email_verified": true }),
    );
    let (status, res) = oauth2(&env, Some(&dave_token), "dave", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["record"]["id"], dave["id"]);
    assert_eq!(res["record"]["email"], "dave@example.com");

    // linked accounts: the record itself and superusers see and remove them
    let dave_id = dave["id"].as_str().unwrap();
    let links_uri = format!("/api/collections/users/records/{dave_id}/external-auths");
    let links = env.ok(Some(&dave_token), "GET", &links_uri, None).await;
    assert_eq!(links.as_array().unwrap().len(), 1);
    assert_eq!(links[0]["provider"], "oidc");
    assert_eq!(links[0]["providerId"], "p-dave");
    assert_eq!(links[0]["recordId"], dave_id);
    let (status, _) = env.call(Some(&token), "GET", &links_uri, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = env.call(None, "GET", &links_uri, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = env
        .call(
            Some(&dave_token),
            "DELETE",
            &format!("{links_uri}/oidc"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = env
        .call(
            Some(&dave_token),
            "DELETE",
            &format!("{links_uri}/oidc"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let links = env
        .ok(Some(&env.admin.clone()), "GET", &links_uri, None)
        .await;
    assert_eq!(links, json!([]));

    // deleting a record removes its links
    let ann_id = ann["id"].as_str().unwrap();
    env.ok(
        Some(&env.admin.clone()),
        "DELETE",
        &format!("/api/collections/users/records/{ann_id}"),
        None,
    )
    .await;
    assert_eq!(
        env.count(&format!(
            r#"SELECT count(*) FROM slonbase."_externalAuths" WHERE "recordRef" = '{ann_id}'"#
        ))
        .await,
        0
    );

    // sign-up follows the create rule
    env.ok(
        Some(&env.admin.clone()),
        "PATCH",
        "/api/collections/users",
        Some(json!({ "createRule": null })),
    )
    .await;
    provider.set_user(
        "frank",
        json!({ "sub": "p-frank", "email": "frank@example.com", "email_verified": true }),
    );
    let (status, _) = oauth2(&env, None, "frank", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // MFA applies to OAuth2 too
    env.ok(
        Some(&env.admin.clone()),
        "PATCH",
        "/api/collections/users",
        Some(json!({ "mfa": { "enabled": true } })),
    )
    .await;
    let (status, res) = oauth2(&env, None, "bob", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{res}");
    let mfa_id = res["mfaId"].as_str().unwrap();
    let (status, res) = env
        .login(
            json!({ "identity": "carol@example.com", "password": "password123", "mfaId": mfa_id }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "another record: {res}");

    // disabled
    let mut options = updated["oauth2"].clone();
    options["enabled"] = json!(false);
    env.ok(
        Some(&env.admin.clone()),
        "PATCH",
        "/api/collections/users",
        Some(json!({ "oauth2": options, "mfa": { "enabled": false } })),
    )
    .await;
    let (status, _) = oauth2(&env, None, "bob", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    drop(client);
    env.finish().await;
}

/// Reads SSE events from `/api/realtime`.
struct Stream {
    body: Body,
    buffer: String,
}

impl Stream {
    async fn open(app: &Arc<App>) -> (Self, String) {
        let res = server::router(app.clone())
            .oneshot(Request::get("/api/realtime").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let mut stream = Self {
            body: res.into_body(),
            buffer: String::new(),
        };
        let (_, data) = stream.next(Duration::from_secs(5)).await.unwrap();
        (stream, data["clientId"].as_str().unwrap().to_string())
    }

    async fn next(&mut self, timeout: Duration) -> Option<(String, Value)> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                let (mut event, mut data) = (String::new(), String::new());
                for line in block.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        event = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim());
                    }
                }
                if event.is_empty() {
                    continue;
                }
                return Some((event, serde_json::from_str(&data).unwrap_or(Value::Null)));
            }
            let frame = tokio::time::timeout_at(deadline, self.body.frame())
                .await
                .ok()??
                .ok()?;
            if let Ok(bytes) = frame.into_data() {
                self.buffer.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
    }
}

#[tokio::test]
async fn oauth2_redirect_reaches_the_realtime_client() {
    let Some(env) = Env::new().await else {
        return;
    };
    let (mut stream, client_id) = Stream::open(&env.app).await;
    env.ok(
        None,
        "POST",
        "/api/realtime",
        Some(json!({ "clientId": client_id, "subscriptions": ["@oauth2"] })),
    )
    .await;

    let (status, _, body) = raw_request(
        &env.app,
        "GET",
        &format!("/api/oauth2-redirect?state={client_id}&code=the-code"),
        &[],
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("close this window"));
    let (event, data) = stream.next(Duration::from_secs(5)).await.expect("@oauth2");
    assert_eq!(event, "@oauth2");
    assert_eq!(
        data,
        json!({ "state": client_id, "code": "the-code", "error": "" })
    );

    // form_post
    let (status, _, _) = raw_request(
        &env.app,
        "POST",
        "/api/oauth2-redirect",
        &[("content-type", "application/x-www-form-urlencoded")],
        format!("state={client_id}&error=access_denied").into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, data) = stream.next(Duration::from_secs(5)).await.expect("@oauth2");
    assert_eq!(data["error"], "access_denied");

    // a missing state
    let (status, _, _) =
        raw_request(&env.app, "GET", "/api/oauth2-redirect?code=x", &[], vec![]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // clients that aren't subscribed get nothing
    env.ok(
        None,
        "POST",
        "/api/realtime",
        Some(json!({ "clientId": client_id, "subscriptions": [] })),
    )
    .await;
    raw_request(
        &env.app,
        "GET",
        &format!("/api/oauth2-redirect?state={client_id}&code=x"),
        &[],
        vec![],
    )
    .await;
    assert!(stream.next(Duration::from_millis(700)).await.is_none());

    drop(stream);
    env.finish().await;
}
