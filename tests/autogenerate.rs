//! `autogeneratePattern` takes any regular expression, and `field:autogenerate` appends a
//! generated value, through the API. Needs PostgreSQL.

mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;

#[tokio::test]
async fn regular_expression_patterns() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = format!("Bearer {token}");
    let admin = |method: &'static str, uri: &str, body: Option<Value>| {
        let app = app.clone();
        let auth = auth.clone();
        let uri = uri.to_string();
        async move {
            request(
                &app,
                method,
                &uri,
                &[("authorization", auth.as_str())],
                body,
            )
            .await
        }
    };

    // a pattern the field can't hold is reported with PocketBase's codes
    let (status, res) = admin(
        "POST",
        "/api/collections",
        Some(json!({
            "name": "invoices",
            "fields": [
                { "name": "number", "type": "text", "max": 6, "autogeneratePattern": "INV-[0-9]{6}" },
                { "name": "ref", "type": "text", "autogeneratePattern": "(ab" },
            ],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(
        res["data"]["fields"]["1"]["autogeneratePattern"]["code"],
        "validation_invalid_autogenerate_pattern_value",
        "{res}"
    );
    assert_eq!(
        res["data"]["fields"]["2"]["autogeneratePattern"]["code"],
        "validation_invalid_autogenerate_pattern",
        "{res}"
    );

    // custom ids too, as PocketBase allows
    let (status, res) = admin(
        "POST",
        "/api/collections",
        Some(json!({
            "name": "invoices", "createRule": "", "updateRule": "", "viewRule": "",
            "fields": [
                { "name": "id", "type": "text", "primaryKey": true, "required": true,
                  "min": 8, "max": 8, "pattern": "^inv[0-9]{5}$", "autogeneratePattern": "inv[0-9]{5}" },
                { "name": "number", "type": "text", "autogeneratePattern": "INV-(19|20)[0-9]{2}-[A-Z]{3}" },
                { "name": "slug", "type": "text", "autogeneratePattern": "[a-z]{4}" },
            ],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    let number = regex::Regex::new("^INV-(19|20)[0-9]{2}-[A-Z]{3}$").unwrap();
    let (status, created) = request(
        &app,
        "POST",
        "/api/collections/invoices/records",
        &[],
        Some(json!({ "slug:autogenerate": "post-" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap();
    assert!(
        regex::Regex::new("^inv[0-9]{5}$").unwrap().is_match(id),
        "{created}"
    );
    assert!(
        number.is_match(created["number"].as_str().unwrap()),
        "{created}"
    );
    let slug = created["slug"].as_str().unwrap();
    assert!(
        slug.len() == 9
            && slug.starts_with("post-")
            && slug[5..].bytes().all(|b| b.is_ascii_lowercase()),
        "{created}"
    );

    // on update the modifier generates again; a plain value is kept as given
    let (status, updated) = request(
        &app,
        "PATCH",
        &format!("/api/collections/invoices/records/{id}"),
        &[],
        Some(json!({ "number:autogenerate": "", "slug": "fixed" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert!(
        number.is_match(updated["number"].as_str().unwrap()),
        "{updated}"
    );
    assert_eq!(updated["slug"], "fixed");
}
