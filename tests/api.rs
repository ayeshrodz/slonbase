//! End-to-end tests of the collections, records and auth APIs against PostgreSQL.
//! See `setup_flow.rs` for how to point them at a database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

struct Client {
    app: Arc<App>,
    token: String,
}

impl Client {
    async fn superuser(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let auth = format!("Bearer {}", self.token);
        request(
            &self.app,
            method,
            uri,
            &[("authorization", auth.as_str())],
            body,
        )
        .await
    }

    async fn guest(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        request(&self.app, method, uri, &[], body).await
    }

    async fn create_collection(&self, body: Value) -> Value {
        let (status, res) = self.superuser("POST", "/api/collections", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{res}");
        res
    }

    async fn create_record(&self, collection: &str, body: Value) -> Value {
        let (status, res) = self
            .superuser(
                "POST",
                &format!("/api/collections/{collection}/records"),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{res}");
        res
    }
}

async fn setup() -> Option<(TestDb, Client, tempfile::TempDir)> {
    let tdb = TestDb::create().await?;
    let (app, token, dir) = ready_app(&tdb).await;
    Some((tdb, Client { app, token }, dir))
}

/// Percent-encodes the values of `key=value&key=value` (each value is everything
/// after its key's first `=`, so filters may contain `=` and `&&`).
fn encode_query(query: &str) -> String {
    let mut parts = Vec::new();
    let mut rest = query;
    while !rest.is_empty() {
        let (key, after) = rest.split_once('=').unwrap_or((rest, ""));
        // the value ends at the next "&key=" that isn't part of "&&"
        let mut end = after.len();
        let bytes = after.as_bytes();
        for i in 0..bytes.len() {
            if bytes[i] == b'&'
                && bytes.get(i + 1) != Some(&b'&')
                && (i == 0 || bytes[i - 1] != b'&')
                && after[i + 1..]
                    .split_once('=')
                    .is_some_and(|(k, _)| k.chars().all(|c| c.is_ascii_alphanumeric()))
            {
                end = i;
                break;
            }
        }
        let value: String = after[..end]
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        parts.push(format!("{key}={value}"));
        rest = after.get(end + 1..).unwrap_or("");
    }
    parts.join("&")
}

fn ids(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn superuser_auth() {
    let Some((tdb, client, _dir)) = setup().await else {
        return;
    };

    let (status, _) = client
        .guest(
            "POST",
            "/api/collections/_superusers/auth-with-password",
            Some(json!({ "identity": ADMIN_EMAIL, "password": "wrong-password" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = client
        .guest(
            "POST",
            "/api/collections/_superusers/auth-with-password",
            Some(json!({ "identity": "ADMIN@example.com", "password": ADMIN_PASSWORD })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["record"]["email"], ADMIN_EMAIL);
    assert_eq!(body["record"]["collectionName"], "_superusers");

    // valid token
    let (status, _) = client.superuser("GET", "/api/collections", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = client
        .superuser("POST", "/api/collections/_superusers/auth-refresh", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["token"].is_string());

    // no, garbage, raw and tampered tokens
    let (status, _) = client.guest("GET", "/api/collections", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for bad in ["garbage", "a.b.c", &format!("{}x", client.token)] {
        let (status, _) = request(
            &client.app,
            "GET",
            "/api/collections",
            &[("authorization", bad)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{bad}");
    }
    // a raw token (without "Bearer ") works like PocketBase
    let (status, _) = request(
        &client.app,
        "GET",
        "/api/collections",
        &[("authorization", client.token.as_str())],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // rotating the token key invalidates issued tokens
    tdb.pool()
        .get()
        .await
        .unwrap()
        .batch_execute(r#"UPDATE slonbase."_superusers" SET "tokenKey" = 'rotated'"#)
        .await
        .unwrap();
    let (status, _) = client.superuser("GET", "/api/collections", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    drop(client);
    tdb.drop_db().await;
}

#[tokio::test]
async fn collections_crud_and_schema_changes() {
    let Some((tdb, client, _dir)) = setup().await else {
        return;
    };

    // validation errors are nested like PocketBase
    let (status, body) = client
        .superuser(
            "POST",
            "/api/collections",
            Some(json!({ "name": "bad name", "fields": [{ "name": "", "type": "text" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["data"]["name"]["message"].is_string(), "{body}");
    assert!(
        body["data"]["fields"]["1"]["name"]["code"].is_string(),
        "{body}"
    );

    let created = client
        .create_collection(json!({
            "name": "posts",
            "fields": [
                { "name": "title", "type": "text", "required": true },
                { "name": "tag", "type": "select", "values": ["a", "b"], "maxSelect": 1 },
                { "name": "created", "type": "autodate", "onCreate": true }
            ],
            "indexes": ["CREATE UNIQUE INDEX idx_posts_title ON posts (title)"],
            "listRule": ""
        }))
        .await;
    assert!(created["id"].as_str().unwrap().starts_with("pbc_"));
    assert_eq!(created["fields"][0]["name"], "id");
    assert_eq!(
        created["indexes"][0],
        "CREATE UNIQUE INDEX `idx_posts_title` ON `posts` (`title`)"
    );
    assert!(created["created"].as_str().unwrap().ends_with('Z'));

    // duplicate names are rejected case-insensitively
    let (status, _) = client
        .superuser("POST", "/api/collections", Some(json!({ "name": "POSTS" })))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, by_name) = client
        .superuser("GET", "/api/collections/posts", None)
        .await;
    let id = by_name["id"].as_str().unwrap().to_string();
    let (status, _) = client
        .superuser("GET", &format!("/api/collections/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    // plus the system collections
    let (_, list) = client.superuser("GET", "/api/collections", None).await;
    assert_eq!(list["totalItems"], 3);

    client
        .create_record("posts", json!({ "title": "first", "tag": "a" }))
        .await;

    // rename the collection and a field, add a field, make the select multiple
    let mut fields = by_name["fields"].as_array().unwrap().clone();
    fields[1]["name"] = json!("headline");
    fields[2]["maxSelect"] = json!(2);
    fields.push(json!({ "name": "views", "type": "number" }));
    let (status, updated) = client
        .superuser(
            "PATCH",
            "/api/collections/posts",
            Some(json!({
                "name": "articles",
                "fields": fields,
                "indexes": ["CREATE UNIQUE INDEX idx_posts_title ON posts (headline)"]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["id"], id.as_str());
    assert_eq!(
        updated["listRule"], "",
        "keys missing from a PATCH are kept"
    );
    assert_eq!(
        updated["indexes"][0],
        "CREATE UNIQUE INDEX `idx_posts_title` ON `articles` (`headline`)"
    );

    // the data survived the changes
    let (status, list) = client
        .guest("GET", "/api/collections/articles/records", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let record = &list["items"][0];
    assert_eq!(record["headline"], "first");
    assert_eq!(record["tag"], json!(["a"]));
    assert_eq!(record["views"], 0);
    assert_eq!(record["collectionName"], "articles");

    // changing a field type is rejected
    let mut fields = updated["fields"].as_array().unwrap().clone();
    assert_eq!(fields[4]["name"], "views");
    fields[4]["type"] = json!("text");
    let (status, _) = client
        .superuser(
            "PATCH",
            "/api/collections/articles",
            Some(json!({ "fields": fields })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = client
        .superuser("DELETE", "/api/collections/articles", None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = client
        .guest("GET", "/api/collections/articles/records", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let exists: bool = tdb
        .pool()
        .get()
        .await
        .unwrap()
        .query_one("SELECT to_regclass('slonbase.articles') IS NOT NULL", &[])
        .await
        .unwrap()
        .get(0);
    assert!(!exists, "the table is dropped");

    drop(client);
    tdb.drop_db().await;
}

#[tokio::test]
async fn records_crud_filter_sort_and_pagination() {
    let Some((tdb, client, _dir)) = setup().await else {
        return;
    };
    client
        .create_collection(json!({
            "name": "posts",
            "fields": [
                { "name": "title", "type": "text", "required": true },
                { "name": "views", "type": "number" },
                { "name": "published", "type": "bool" },
                { "name": "tags", "type": "select", "values": ["a", "b", "c"], "maxSelect": 3 },
                { "name": "meta", "type": "json" },
                { "name": "when", "type": "date" },
                { "name": "created", "type": "autodate", "onCreate": true },
                { "name": "updated", "type": "autodate", "onCreate": true, "onUpdate": true }
            ],
            "indexes": ["CREATE UNIQUE INDEX idx_title ON posts (title)"]
        }))
        .await;

    let first = client
        .create_record(
            "posts",
            json!({ "title": "alpha", "views": 10, "published": true, "tags": ["a", "b"],
                    "meta": { "score": 5, "kind": "x" }, "when": "2024-01-15" }),
        )
        .await;
    assert_eq!(first["id"].as_str().unwrap().len(), 15);
    assert_eq!(first["collectionName"], "posts");
    assert_eq!(first["when"], "2024-01-15 00:00:00.000Z");
    assert_eq!(first["created"], first["updated"]);
    client
        .create_record(
            "posts",
            json!({ "title": "beta", "views": "20", "tags": "c", "meta": { "score": 7 } }),
        )
        .await;
    client
        .create_record(
            "posts",
            json!({ "id": "custom000000001", "title": "gamma", "views": 30, "published": true }),
        )
        .await;

    let list = |query: &str| {
        let uri = format!("/api/collections/posts/records?{}", encode_query(query));
        let client = &client;
        async move {
            let (status, body) = client.superuser("GET", &uri, None).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
            body
        }
    };

    let all = list("").await;
    assert_eq!(all["totalItems"], 3);
    assert_eq!(
        ids(&all),
        ["alpha", "beta", "gamma"],
        "insertion order by default"
    );

    assert_eq!(ids(&list("sort=-views").await), ["gamma", "beta", "alpha"]);
    assert_eq!(ids(&list("filter=views>15").await), ["beta", "gamma"]);
    assert_eq!(ids(&list("filter=views>'15'").await), ["beta", "gamma"]);
    assert_eq!(
        ids(&list("filter=published=true && views<20").await),
        ["alpha"]
    );
    assert_eq!(ids(&list("filter=title~'AL'").await), ["alpha"]);
    assert_eq!(ids(&list("filter=tags?='c'").await), ["beta"]);
    assert_eq!(ids(&list("filter=tags='a'").await), Vec::<String>::new());
    assert_eq!(
        ids(&list("filter=tags=''").await),
        ["gamma"],
        "empty multi matches ''"
    );
    assert_eq!(ids(&list("filter=tags:length=2").await), ["alpha"]);
    assert_eq!(ids(&list("filter=meta.score>6").await), ["beta"]);
    assert_eq!(ids(&list("filter=meta.kind='x'").await), ["alpha"]);
    assert_eq!(ids(&list("filter=when>='2024-01-01'").await), ["alpha"]);
    assert_eq!(ids(&list("filter=when=null").await), ["beta", "gamma"]);
    assert_eq!(ids(&list("filter=id='custom000000001'").await), ["gamma"]);
    assert_eq!(ids(&list("filter=created<=@now").await).len(), 3);

    let page = list("perPage=2&page=2&sort=title").await;
    assert_eq!(ids(&page), ["gamma"]);
    assert_eq!(page["totalPages"], 2);
    assert_eq!(page["perPage"], 2);
    let skipped = list("skipTotal=1").await;
    assert_eq!(skipped["totalItems"], -1);

    let picked = list("fields=id,title").await;
    let keys: Vec<&String> = picked["items"][0].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["id", "title"]);

    for bad in [
        "filter=nope=1",
        "filter=title=",
        "sort=nope",
        "filter=title='x'; DROP",
    ] {
        let (status, _) = client
            .superuser(
                "GET",
                &format!("/api/collections/posts/records?{}", encode_query(bad)),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    // view, update with modifiers, unique violations, delete
    let id = first["id"].as_str().unwrap();
    let uri = format!("/api/collections/posts/records/{id}");
    let (status, viewed) = client.superuser("GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(viewed, first);

    let (status, updated) = client
        .superuser(
            "PATCH",
            &uri,
            Some(json!({ "views+": 5, "tags-": "a", "+tags": "c" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["views"], 15);
    assert_eq!(updated["tags"], json!(["c", "b"]));
    assert_eq!(updated["title"], "alpha");
    assert_eq!(updated["created"], first["created"]);

    let (status, body) = client
        .superuser("PATCH", &uri, Some(json!({ "title": "beta" })))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["data"]["title"]["message"], "Value must be unique.");

    let (status, body) = client
        .superuser(
            "POST",
            "/api/collections/posts/records",
            Some(json!({ "views": "x", "tags": ["z"] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "Failed to create record.");
    for field in ["title", "views", "tags"] {
        assert!(body["data"][field].is_object(), "{field}: {body}");
    }

    let (status, _) = client.superuser("DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = client.superuser("GET", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    drop(client);
    tdb.drop_db().await;
}

#[tokio::test]
async fn api_rules() {
    let Some((tdb, client, _dir)) = setup().await else {
        return;
    };
    client
        .create_collection(json!({
            "name": "posts",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "status", "type": "select", "values": ["draft", "published"] },
                { "name": "secret", "type": "text", "hidden": true }
            ],
            "listRule": "status = 'published'",
            "viewRule": "status = 'published'",
            "createRule": "@request.body.title != 'blocked' && status = 'draft'",
            "updateRule": null,
            "deleteRule": "status = 'draft'"
        }))
        .await;
    let published = client
        .create_record(
            "posts",
            json!({ "title": "public", "status": "published", "secret": "s1" }),
        )
        .await;
    let draft = client
        .create_record(
            "posts",
            json!({ "title": "private", "status": "draft", "secret": "s2" }),
        )
        .await;
    assert_eq!(published["secret"], "s1", "superusers see hidden fields");

    // list and view only show what the rule allows, without hidden fields
    let (status, list) = client
        .guest("GET", "/api/collections/posts/records", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&list), ["public"]);
    assert!(list["items"][0].get("secret").is_none());
    let (status, _) = client
        .guest(
            "GET",
            &format!(
                "/api/collections/posts/records/{}",
                draft["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // guests can't filter by hidden fields, the rule still applies to a client filter
    let (status, _) = client
        .guest(
            "GET",
            &format!(
                "/api/collections/posts/records?{}",
                encode_query("filter=secret='s1'")
            ),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, list) = client
        .guest(
            "GET",
            &format!(
                "/api/collections/posts/records?{}",
                encode_query("filter=status='draft'")
            ),
            None,
        )
        .await;
    assert_eq!(list["totalItems"], 0);

    // create rule: checked with the body and against the new record
    let (status, created) = client
        .guest(
            "POST",
            "/api/collections/posts/records",
            Some(json!({ "title": "mine", "status": "draft" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert!(created.get("secret").is_none());
    for body in [
        json!({ "title": "blocked", "status": "draft" }),
        json!({ "title": "sneaky", "status": "published" }),
    ] {
        let (status, res) = client
            .guest("POST", "/api/collections/posts/records", Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    }
    let (_, all) = client
        .superuser("GET", "/api/collections/posts/records", None)
        .await;
    assert_eq!(all["totalItems"], 3, "rejected creates are rolled back");

    // null rule: superusers only
    let (status, body) = client
        .guest(
            "PATCH",
            &format!(
                "/api/collections/posts/records/{}",
                published["id"].as_str().unwrap()
            ),
            Some(json!({ "title": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["message"], "Only superusers can perform this action.");

    // delete rule
    let (status, _) = client
        .guest(
            "DELETE",
            &format!(
                "/api/collections/posts/records/{}",
                published["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = client
        .guest(
            "DELETE",
            &format!(
                "/api/collections/posts/records/{}",
                draft["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // rules can't be saved if they don't compile
    let (status, body) = client
        .superuser(
            "PATCH",
            "/api/collections/posts",
            Some(json!({ "listRule": "missing = 1" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["data"]["listRule"].is_object());

    drop(client);
    tdb.drop_db().await;
}

#[tokio::test]
async fn relations() {
    let Some((tdb, client, _dir)) = setup().await else {
        return;
    };
    let authors = client
        .create_collection(json!({
            "name": "authors",
            "fields": [{ "name": "name", "type": "text" }],
            "listRule": "", "viewRule": "", "deleteRule": ""
        }))
        .await;
    let authors_id = authors["id"].as_str().unwrap();
    client
        .create_collection(json!({
            "name": "posts",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "author", "type": "relation", "collectionId": authors_id, "required": true },
                { "name": "editors", "type": "relation", "collectionId": authors_id, "maxSelect": 5 }
            ],
            "listRule": ""
        }))
        .await;
    client
        .create_collection(json!({
            "name": "comments",
            "fields": [
                { "name": "body", "type": "text" },
                { "name": "author", "type": "relation", "collectionId": authors_id, "cascadeDelete": true }
            ],
            "listRule": ""
        }))
        .await;

    let ann = client
        .create_record("authors", json!({ "name": "ann" }))
        .await;
    let bob = client
        .create_record("authors", json!({ "name": "bob" }))
        .await;
    let (ann_id, bob_id) = (ann["id"].as_str().unwrap(), bob["id"].as_str().unwrap());

    let (status, body) = client
        .superuser(
            "POST",
            "/api/collections/posts/records",
            Some(json!({ "title": "x", "author": "doesnotexist12" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["data"]["author"].is_object());

    let post = client
        .create_record(
            "posts",
            json!({ "title": "p1", "author": ann_id, "editors": [ann_id, bob_id] }),
        )
        .await;
    client
        .create_record(
            "posts",
            json!({ "title": "p2", "author": bob_id, "editors": [bob_id] }),
        )
        .await;
    client
        .create_record("comments", json!({ "body": "c1", "author": bob_id }))
        .await;

    // filter by relation id
    let (_, list) = client
        .guest(
            "GET",
            &format!(
                "/api/collections/posts/records?{}",
                encode_query(&format!("filter=editors?='{bob_id}'"))
            ),
            None,
        )
        .await;
    assert_eq!(list["totalItems"], 2);

    // a required reference blocks the delete
    let (status, _) = client
        .guest(
            "DELETE",
            &format!("/api/collections/authors/records/{ann_id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // after moving p2's required author away from bob, deleting bob removes him from
    // optional lists and cascades to his comments
    let (_, p2) = client
        .guest(
            "GET",
            &format!(
                "/api/collections/posts/records?{}",
                encode_query("filter=title='p2'")
            ),
            None,
        )
        .await;
    let p2_id = p2["items"][0]["id"].as_str().unwrap();
    client
        .superuser(
            "PATCH",
            &format!("/api/collections/posts/records/{p2_id}"),
            Some(json!({ "author": ann_id })),
        )
        .await;
    let (status, body) = client
        .guest(
            "DELETE",
            &format!("/api/collections/authors/records/{bob_id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (_, viewed) = client
        .superuser(
            "GET",
            &format!(
                "/api/collections/posts/records/{}",
                post["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(viewed["editors"], json!([ann_id]));
    let (_, comments) = client
        .guest("GET", "/api/collections/comments/records", None)
        .await;
    assert_eq!(comments["totalItems"], 0, "cascade delete");

    // a referenced collection can't be deleted
    let (status, _) = client
        .superuser("DELETE", "/api/collections/authors", None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    drop(client);
    tdb.drop_db().await;
}

/// Every combination of operand types and operators must produce SQL that PostgreSQL
/// accepts: invalid filters may be rejected with 400, but never fail with 500.
#[tokio::test]
async fn filter_type_matrix_never_errors_in_postgres() {
    let Some((tdb, client, _dir)) = setup().await else {
        return;
    };
    client
        .create_collection(json!({
            "name": "things",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "views", "type": "number" },
                { "name": "flag", "type": "bool" },
                { "name": "when", "type": "date" },
                { "name": "made", "type": "autodate", "onCreate": true },
                { "name": "one", "type": "select", "values": ["a", "b"] },
                { "name": "many", "type": "select", "values": ["a", "b"], "maxSelect": 2 },
                { "name": "meta", "type": "json" },
                { "name": "mail", "type": "email" }
            ],
            "listRule": ""
        }))
        .await;
    client
        .create_record("things", json!({ "title": "t", "views": 3, "flag": true, "when": "2024-05-05",
                                         "one": "a", "many": ["a", "b"], "meta": { "n": 2, "s": "x" } }))
        .await;
    client
        .create_record("things", json!({ "title": "", "meta": [1, 2] }))
        .await;

    let operands = [
        "title",
        "views",
        "flag",
        "when",
        "made",
        "one",
        "many",
        "many:length",
        "meta",
        "meta.n",
        "meta.s",
        "mail",
        "id",
        "title:lower",
        "title:length",
        "'text'",
        "''",
        "5",
        "-1.5",
        "'5'",
        "true",
        "false",
        "null",
        "'2024-01-01'",
        "'2024-01-01 10:00:00.000Z'",
        "@now",
        "@todayStart",
        "@year",
        "@request.query.q",
        "@request.method",
        "@request.auth.id",
        "@request.headers.x_test",
    ];
    let operators = [
        "=", "!=", ">", ">=", "<", "<=", "~", "!~", "?=", "?!=", "?>", "?~",
    ];

    let mut checked = 0;
    for left in operands {
        for right in operands {
            for op in operators {
                let filter = format!("{left} {op} {right}");
                let uri = format!(
                    "/api/collections/things/records?q=7&{}",
                    encode_query(&format!("filter={filter}"))
                );
                let (status, body) = client.guest("GET", &uri, None).await;
                assert!(
                    status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
                    "{filter:?} -> {status}: {body}"
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, operands.len() * operands.len() * operators.len());

    drop(client);
    tdb.drop_db().await;
}

#[tokio::test]
async fn superusers_are_a_system_collection() {
    let Some((tdb, client, _dir)) = setup().await else {
        return;
    };

    let (_, c) = client
        .superuser("GET", "/api/collections/_superusers", None)
        .await;
    assert_eq!(c["id"], "pbc_3142635823");
    assert_eq!(c["system"], true);
    assert_eq!(c["type"], "auth");
    let (_, external) = client
        .superuser("GET", "/api/collections/_externalAuths", None)
        .await;
    assert_eq!(external["system"], true);

    // system collections can't be renamed or deleted
    let (status, _) = client
        .superuser(
            "PATCH",
            "/api/collections/_superusers",
            Some(json!({ "name": "admins" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = client
        .superuser("DELETE", "/api/collections/_externalAuths", None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // but their options can change
    let (status, res) = client
        .superuser(
            "PATCH",
            "/api/collections/_superusers",
            Some(json!({ "authToken": { "duration": 3600 } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // superusers are managed with the records API, by superusers only
    let (_, list) = client
        .superuser("GET", "/api/collections/_superusers/records", None)
        .await;
    assert_eq!(list["totalItems"], 1);
    assert_eq!(list["items"][0]["email"], ADMIN_EMAIL);
    assert!(list["items"][0].get("password").is_none());
    let first_id = list["items"][0]["id"].as_str().unwrap().to_string();
    let (status, _) = client
        .guest("GET", "/api/collections/_superusers/records", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // passwords need at least 10 characters
    let (status, _) = client
        .superuser(
            "POST",
            "/api/collections/_superusers/records",
            Some(json!({ "email": "second@example.com", "password": "short123", "passwordConfirm": "short123" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let created = client
        .create_record(
            "_superusers",
            json!({ "email": "second@example.com", "password": "second-password", "passwordConfirm": "second-password" }),
        )
        .await;
    let (status, login) = client
        .guest(
            "POST",
            "/api/collections/_superusers/auth-with-password",
            Some(json!({ "identity": "second@example.com", "password": "second-password" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    assert_eq!(login["record"]["collectionName"], "_superusers");
    let second = Client {
        app: client.app.clone(),
        token: login["token"].as_str().unwrap().to_string(),
    };
    let (status, _) = second.superuser("GET", "/api/collections", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, refreshed) = second
        .superuser("POST", "/api/collections/_superusers/auth-refresh", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{refreshed}");

    // the last superuser can't be deleted
    let (status, _) = second
        .superuser(
            "DELETE",
            &format!("/api/collections/_superusers/records/{first_id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, res) = second
        .superuser(
            "DELETE",
            &format!(
                "/api/collections/_superusers/records/{}",
                created["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    // the deleted superuser's token stopped working
    let (status, _) = client.superuser("GET", "/api/collections", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    drop((client, second));
    tdb.drop_db().await;
}

/// Installations from before the system collections kept superusers and linked accounts
/// in private tables; they're moved into the collections.
#[tokio::test]
async fn legacy_tables_move_into_system_collections() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let pool = tdb.pool();
    slonbase::migrations::run(&pool).await.unwrap();
    let hash = bcrypt::hash("legacy-password", 4).unwrap();
    {
        let client = pool.get().await.unwrap();
        client
            .execute(
                "INSERT INTO _slonbase.superusers (id, email, password_hash, token_key)
                 VALUES ('legacyadmin0001', 'old@example.com', $1, 'k')",
                &[&hash],
            )
            .await
            .unwrap();
        client
            .batch_execute(
                // a stand-in for the linked record's collection (the legacy table's foreign key)
                "INSERT INTO _slonbase.collections (id, name, data)
                     VALUES ('pbc_users00001', 'people', '{}');
                 INSERT INTO _slonbase.external_auths (id, collection_id, record_id, provider, provider_id)
                     VALUES ('link00000000001', 'pbc_users00001', 'rec', 'github', '42');",
            )
            .await
            .unwrap();
    }

    slonbase::collections::system::ensure(&pool).await.unwrap();
    let client = pool.get().await.unwrap();
    let row = client
        .query_one(
            r#"SELECT email, password, length("tokenKey"), verified FROM slonbase."_superusers""#,
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "old@example.com");
    assert_eq!(row.get::<_, String>(1), hash);
    assert_eq!(row.get::<_, i32>(2), 50);
    assert!(row.get::<_, bool>(3));
    let link = client
        .query_one(
            r#"SELECT "collectionRef", "recordRef", provider, "providerId" FROM slonbase."_externalAuths""#,
            &[],
        )
        .await
        .unwrap();
    assert_eq!(link.get::<_, String>(3), "42");
    for table in ["_slonbase.superusers", "_slonbase.external_auths"] {
        let exists: bool = client
            .query_one("SELECT to_regclass($1) IS NOT NULL", &[&table])
            .await
            .unwrap()
            .get(0);
        assert!(!exists, "{table} was dropped");
    }
    // idempotent
    slonbase::collections::system::ensure(&pool).await.unwrap();
    client
        .batch_execute("DELETE FROM _slonbase.collections WHERE id = 'pbc_users00001'")
        .await
        .unwrap();
    drop(client);

    // and the migrated superuser can sign in
    let (_dir, config) = temp_config();
    let app = new_app(&config, Some(tdb.url.clone()));
    app.bootstrap().await.unwrap();
    let (status, body) = request(
        &app,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        &[],
        Some(json!({ "identity": "old@example.com", "password": "legacy-password" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    drop(app);
    tdb.drop_db().await;
}
