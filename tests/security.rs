//! Findings of the 1.1 security review, each kept as a test (see docs/security-review.md).
//! See `setup_flow.rs` for how to point them at a database.

mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

async fn as_admin(
    app: &std::sync::Arc<slonbase::app::App>,
    admin: &str,
    method: &str,
    uri: &str,
    body: Value,
) -> Value {
    let auth = format!("Bearer {admin}");
    let (status, res) = request(app, method, uri, &[("authorization", &auth)], Some(body)).await;
    assert!(status.is_success(), "{method} {uri} -> {status}: {res}");
    res
}

/// How many records of `collection` a client's `filter` matches.
async fn matches(
    app: &std::sync::Arc<slonbase::app::App>,
    token: Option<&str>,
    collection: &str,
    filter: &str,
) -> i64 {
    let auth = token.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.iter().map(|a| ("authorization", a.as_str())).collect();
    let uri = format!(
        "/api/collections/{collection}/records?filter={}",
        enc(filter)
    );
    let (status, res) = request(app, "GET", &uri, &headers, None).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {res}");
    res["totalItems"].as_i64().unwrap()
}

/// A client's filter could read other collections' records through relation paths and
/// `@collection` lookups, whatever their API rules, one yes-or-no answer at a time, and the
/// emails of auth records that hide them. It now sees what the client could list itself.
#[tokio::test]
async fn filters_only_see_what_the_client_may_list() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app(&tdb).await;

    // superusers only
    let vault = as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections",
        json!({
            "name": "vault", "fields": [{ "name": "value", "type": "text" }]
        }),
    )
    .await;
    // each member sees only their own record
    let members = as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections",
        json!({
            "name": "members", "type": "auth",
            "listRule": "id = @request.auth.id", "viewRule": "id = @request.auth.id"
        }),
    )
    .await;
    // public, and listing everyone (with their emails hidden)
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections",
        json!({
            "name": "people", "type": "auth", "listRule": "", "viewRule": ""
        }),
    )
    .await;
    as_admin(&app, &admin, "POST", "/api/collections", json!({
        "name": "notes", "listRule": "", "viewRule": "",
        "fields": [
            { "name": "title", "type": "text" },
            { "name": "ref", "type": "relation", "collectionId": vault["id"], "maxSelect": 1 },
            { "name": "owner", "type": "relation", "collectionId": members["id"], "maxSelect": 1 }
        ]
    })).await;
    let secret = as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections/vault/records",
        json!({ "value": "s3cr3t" }),
    )
    .await;
    let member = as_admin(&app, &admin, "POST", "/api/collections/members/records", json!({
        "email": "ceo@hidden.example", "password": "password-123", "passwordConfirm": "password-123"
    })).await;
    as_admin(&app, &admin, "POST", "/api/collections/people/records", json!({
        "email": "alice@hidden.example", "password": "password-123", "passwordConfirm": "password-123"
    })).await;
    as_admin(
        &app,
        &admin,
        "POST",
        "/api/collections/notes/records",
        json!({
            "title": "hello", "ref": secret["id"], "owner": member["id"]
        }),
    )
    .await;

    // a guest learns nothing
    for (collection, filter) in [
        ("notes", "ref.value ~ 's3cr3t%'"),
        ("notes", "@collection.vault.value ~ 's3cr3t%'"),
        ("notes", "owner.email ~ 'ceo@%'"),
        ("notes", "@collection.members.email ~ 'ceo%'"),
        ("people", "email ~ 'alice%'"),
    ] {
        assert_eq!(
            matches(&app, None, collection, filter).await,
            0,
            "{collection}: {filter}"
        );
    }
    // and can't sort by hidden emails to learn their order
    let (status, _) = request(
        &app,
        "GET",
        "/api/collections/people/records?sort=-email",
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // what the caller may see still works: their own record, public fields, and everything
    // for superusers
    let (_, signed_in) = request(
        &app,
        "POST",
        "/api/collections/members/auth-with-password",
        &[],
        Some(json!({ "identity": "ceo@hidden.example", "password": "password-123" })),
    )
    .await;
    let member_token = signed_in["token"].as_str().unwrap();
    assert_eq!(
        matches(&app, Some(member_token), "notes", "owner.email ~ 'ceo@%'").await,
        1
    );
    assert_eq!(
        matches(
            &app,
            Some(member_token),
            "notes",
            "@collection.members.email ~ 'ceo%'"
        )
        .await,
        1
    );
    assert_eq!(
        matches(&app, Some(member_token), "notes", "ref.value ~ 's3cr3t%'").await,
        0
    );
    assert_eq!(matches(&app, None, "notes", "title = 'hello'").await, 1);
    assert_eq!(
        matches(&app, Some(&admin), "notes", "ref.value ~ 's3cr3t%'").await,
        1
    );
    assert_eq!(
        matches(&app, Some(&admin), "people", "email ~ 'alice%'").await,
        1
    );
    // rules written by superusers still reach every collection
    as_admin(
        &app,
        &admin,
        "PATCH",
        "/api/collections/notes",
        json!({ "listRule": "ref.value = 's3cr3t'" }),
    )
    .await;
    assert_eq!(matches(&app, None, "notes", "title = 'hello'").await, 1);

    drop(app);
    tdb.drop_db().await;
}

/// Every response says not to sniff its type or be framed by other sites; the setup page,
/// whose URL carries the setup token, allows only its own script; the health check doesn't
/// tell anyone how the database connection failed.
#[tokio::test]
async fn security_headers_and_quiet_health() {
    let (_dir, config_path) = temp_config();
    let app = new_app(&config_path, None);
    app.bootstrap().await.unwrap();
    for uri in ["/api/health", "/_/", "/_/setup", "/api/collections"] {
        let (_, headers, _) = raw_request(&app, "GET", uri, &[], vec![]).await;
        assert_eq!(headers["x-content-type-options"], "nosniff", "{uri}");
        assert!(headers.contains_key("x-frame-options"), "{uri}");
        assert!(headers.contains_key("referrer-policy"), "{uri}");
    }
    let (_, headers, _) = raw_request(&app, "GET", "/_/setup", &[], vec![]).await;
    let policy = headers["content-security-policy"].to_str().unwrap();
    assert!(policy.contains("script-src 'sha256-"), "{policy}");
    assert!(!policy.contains("script-src 'unsafe-inline'"), "{policy}");

    // a database that can't be reached: the answer says so, without the details
    let (_dir, config_path) = temp_config();
    let app = new_app(
        &config_path,
        Some("postgres://nobody:secret@127.0.0.1:1/internal_db?connect_timeout=1".into()),
    );
    tokio::spawn({
        let app = app.clone();
        async move { app.bootstrap().await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let (status, res) = request(&app, "GET", "/api/health", &[], None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{res}");
    let message = res["message"].as_str().unwrap();
    assert!(
        !message.contains("127.0.0.1") && !message.contains("refused"),
        "{message}"
    );
}
