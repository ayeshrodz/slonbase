//! Relation paths in filters and rules, and `expand`, against PostgreSQL.
//! See `setup_flow.rs` for how to point them at a database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

struct Env {
    tdb: TestDb,
    app: Arc<App>,
    admin: String,
    _dir: tempfile::TempDir,
    ids: std::collections::HashMap<String, String>,
}

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

impl Env {
    async fn call(
        &self,
        admin: bool,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let auth = format!("Bearer {}", self.admin);
        let headers: Vec<(&str, &str)> = if admin {
            vec![("authorization", auth.as_str())]
        } else {
            vec![]
        };
        request(&self.app, method, uri, &headers, body).await
    }

    async fn ok(&self, admin: bool, method: &str, uri: &str, body: Option<Value>) -> Value {
        let (status, res) = self.call(admin, method, uri, body).await;
        assert!(status.is_success(), "{method} {uri} -> {status}: {res}");
        res
    }

    async fn collection(&self, body: Value) -> String {
        self.ok(true, "POST", "/api/collections", Some(body)).await["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn record(&mut self, collection: &str, key: &str, body: Value) -> String {
        let id = self
            .ok(
                true,
                "POST",
                &format!("/api/collections/{collection}/records"),
                Some(body),
            )
            .await["id"]
            .as_str()
            .unwrap()
            .to_string();
        self.ids.insert(key.to_string(), id.clone());
        id
    }

    fn id(&self, key: &str) -> &str {
        &self.ids[key]
    }

    /// Titles of the posts matching a filter (as a guest).
    async fn titles(&self, filter: &str) -> Vec<String> {
        let (status, body) = self
            .call(
                false,
                "GET",
                &format!(
                    "/api/collections/posts/records?sort=title&filter={}",
                    enc(filter)
                ),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{filter}: {body}");
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["title"].as_str().unwrap().to_string())
            .collect()
    }
}

/// orgs <- authors (org, tags) <- posts (author, editors) <- comments (post), plus a
/// private collection referenced by posts.
async fn setup() -> Option<Env> {
    let tdb = TestDb::create().await?;
    let (app, admin, dir) = ready_app(&tdb).await;
    let mut env = Env {
        tdb,
        app,
        admin,
        _dir: dir,
        ids: Default::default(),
    };

    let orgs = env
        .collection(json!({ "name": "orgs", "fields": [{ "name": "name", "type": "text" }], "listRule": "", "viewRule": "" }))
        .await;
    let authors = env
        .collection(json!({
            "name": "authors",
            "fields": [
                { "name": "name", "type": "text" },
                { "name": "org", "type": "relation", "collectionId": orgs },
                { "name": "tags", "type": "select", "values": ["x", "y"], "maxSelect": 2 },
                { "name": "secret", "type": "text", "hidden": true }
            ],
            "listRule": "", "viewRule": ""
        }))
        .await;
    let private = env
        .collection(json!({ "name": "private", "fields": [{ "name": "note", "type": "text" }] }))
        .await;
    let posts = env
        .collection(json!({
            "name": "posts",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "author", "type": "relation", "collectionId": authors },
                { "name": "editors", "type": "relation", "collectionId": authors, "maxSelect": 5 },
                { "name": "extra", "type": "relation", "collectionId": private }
            ],
            "listRule": "", "viewRule": "", "createRule": "", "updateRule": ""
        }))
        .await;
    env.collection(json!({
        "name": "comments",
        "fields": [
            { "name": "body", "type": "text" },
            { "name": "post", "type": "relation", "collectionId": posts }
        ],
        "listRule": "", "viewRule": ""
    }))
    .await;

    let acme = env.record("orgs", "acme", json!({ "name": "acme" })).await;
    let other = env
        .record("orgs", "other", json!({ "name": "other" }))
        .await;
    let ann = env
        .record(
            "authors",
            "ann",
            json!({ "name": "ann", "org": acme, "tags": ["x"], "secret": "s" }),
        )
        .await;
    let bob = env
        .record(
            "authors",
            "bob",
            json!({ "name": "bob", "org": other, "tags": ["x", "y"] }),
        )
        .await;
    let note = env
        .record("private", "note", json!({ "note": "hidden" }))
        .await;
    let p1 = env
        .record(
            "posts",
            "p1",
            json!({ "title": "p1", "author": ann, "editors": [ann, bob], "extra": note }),
        )
        .await;
    env.record(
        "posts",
        "p2",
        json!({ "title": "p2", "author": bob, "editors": [bob] }),
    )
    .await;
    env.record("posts", "p3", json!({ "title": "p3" })).await;
    env.record("comments", "c1", json!({ "body": "first", "post": p1 }))
        .await;
    env.record("comments", "c2", json!({ "body": "second", "post": p1 }))
        .await;
    Some(env)
}

#[tokio::test]
async fn relation_paths_in_filters_and_rules() {
    let Some(env) = setup().await else { return };

    assert_eq!(env.titles("author.name = 'ann'").await, ["p1"]);
    assert_eq!(
        env.titles("author.org.name = 'other'").await,
        ["p2"],
        "two hops"
    );
    assert_eq!(env.titles("author.org.name ~ 'ac'").await, ["p1"]);
    assert_eq!(env.titles("author = null").await, ["p3"]);
    assert_eq!(
        env.titles("author.name = null").await,
        ["p3"],
        "a missing relation is empty"
    );
    assert_eq!(env.titles("author.name != null").await, ["p1", "p2"]);

    // multi-valued relations: plain operators match all, ? any
    assert_eq!(env.titles("editors.name ?= 'ann'").await, ["p1"]);
    assert_eq!(env.titles("editors.name ?= 'bob'").await, ["p1", "p2"]);
    assert_eq!(env.titles("editors.name = 'bob'").await, ["p2"]);
    assert_eq!(env.titles("editors.name = null").await, ["p3"]);
    assert_eq!(
        env.titles("editors.org.name ?= 'acme'").await,
        ["p1"],
        "through a multi relation"
    );
    // a multi-valued field of the related record
    assert_eq!(env.titles("author.tags ?= 'y'").await, ["p2"]);

    // hidden fields of related records stay hidden from guests
    let (status, _) = env
        .call(
            false,
            "GET",
            &format!(
                "/api/collections/posts/records?filter={}",
                enc("author.secret = 's'")
            ),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = env
        .call(
            true,
            "GET",
            &format!(
                "/api/collections/posts/records?filter={}",
                enc("author.secret = 's'")
            ),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["totalItems"], 1);

    // invalid paths
    for bad in [
        "author.nope = 1",
        "title.x = 1",
        "author.org.name.x = 1",
        "editors.name:lower = 'a'",
    ] {
        let (status, _) = env
            .call(
                false,
                "GET",
                &format!("/api/collections/posts/records?filter={}", enc(bad)),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    // rules can use relation paths
    env.ok(
        true,
        "PATCH",
        "/api/collections/posts",
        Some(json!({ "listRule": "author.org.name = 'acme'" })),
    )
    .await;
    assert_eq!(env.titles("").await, ["p1"]);
    let (status, _) = env
        .call(
            true,
            "PATCH",
            "/api/collections/posts",
            Some(json!({ "listRule": "author.missing = 1" })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "rules are validated through relations"
    );

    env.tdb.drop_db().await;
}

#[tokio::test]
async fn expand_relations() {
    let Some(env) = setup().await else { return };
    let p1 = env.id("p1").to_string();

    let post = env
        .ok(false, "GET", &format!("/api/collections/posts/records/{p1}?expand=author.org,editors,comments_via_post,extra"), None)
        .await;
    let expand = &post["expand"];
    assert_eq!(expand["author"]["name"], "ann");
    assert_eq!(expand["author"]["collectionName"], "authors");
    assert!(
        expand["author"].get("secret").is_none(),
        "hidden fields stay hidden"
    );
    assert_eq!(expand["author"]["expand"]["org"]["name"], "acme", "nested");
    let editors: Vec<&str> = expand["editors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(editors, ["ann", "bob"], "in the stored order");
    let comments: Vec<&str> = expand["comments_via_post"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap())
        .collect();
    assert_eq!(comments, ["first", "second"], "back-relation");
    assert!(
        expand.get("extra").is_none(),
        "the private collection's rule hides it from guests"
    );

    let as_admin = env
        .ok(
            true,
            "GET",
            &format!("/api/collections/posts/records/{p1}?expand=extra,author"),
            None,
        )
        .await;
    assert_eq!(as_admin["expand"]["extra"]["note"], "hidden");
    assert_eq!(
        as_admin["expand"]["author"]["secret"], "s",
        "superusers see hidden fields"
    );

    // in lists, with fields, and unknown paths are ignored
    let list = env
        .ok(false, "GET", "/api/collections/posts/records?sort=title&expand=author,nope,x_via_y&fields=title,expand", None)
        .await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(
        items[0].as_object().unwrap().keys().collect::<Vec<_>>(),
        ["title", "expand"]
    );
    assert_eq!(items[1]["expand"]["author"]["name"], "bob");
    assert!(items[2].get("expand").is_none(), "nothing to expand");

    // create and update responses
    let created = env
        .ok(
            false,
            "POST",
            "/api/collections/posts/records?expand=author",
            Some(json!({ "title": "p4", "author": env.id("bob") })),
        )
        .await;
    assert_eq!(created["expand"]["author"]["name"], "bob");
    let updated = env
        .ok(
            false,
            "PATCH",
            &format!(
                "/api/collections/posts/records/{}?expand=editors",
                created["id"].as_str().unwrap()
            ),
            Some(json!({ "editors": [env.id("ann")] })),
        )
        .await;
    assert_eq!(updated["expand"]["editors"][0]["name"], "ann");

    env.tdb.drop_db().await;
}

/// Relation path operands compared with every kind of value never fail in PostgreSQL.
#[tokio::test]
async fn relation_path_matrix_never_errors() {
    let Some(env) = setup().await else { return };
    let left = [
        "author.name",
        "author.org",
        "author.org.name",
        "author.tags",
        "editors.name",
        "editors.org.name",
        "editors.tags",
        "author.created",
        "author.id",
        "editors.id",
    ];
    let right = [
        "'ann'",
        "''",
        "5",
        "true",
        "null",
        "@now",
        "title",
        "author",
        "editors",
        "'2024-01-01'",
    ];
    let operators = ["=", "!=", ">", "<=", "~", "!~", "?=", "?!=", "?~"];
    for l in left {
        for r in right {
            for op in operators {
                let filter = format!("{l} {op} {r}");
                let (status, body) = env
                    .call(
                        false,
                        "GET",
                        &format!("/api/collections/posts/records?filter={}", enc(&filter)),
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
    env.tdb.drop_db().await;
}

#[tokio::test]
async fn request_auth_relation_paths() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app(&tdb).await;
    let admin_auth = format!("Bearer {admin}");
    let as_admin = |method: &'static str, uri: String, body: Value| {
        let (app, auth) = (app.clone(), admin_auth.clone());
        async move {
            let (status, res) = request(
                &app,
                method,
                &uri,
                &[("authorization", auth.as_str())],
                Some(body),
            )
            .await;
            assert!(status.is_success(), "{method} {uri} -> {status}: {res}");
            res
        }
    };
    let id = |v: Value| v["id"].as_str().unwrap().to_string();

    // users belong to a team (single) and groups (multiple); teams have an owner
    let users = id(as_admin(
        "POST",
        "/api/collections".into(),
        json!({
            "name": "members", "type": "auth", "fields": [{ "name": "name", "type": "text" }],
        }),
    )
    .await);
    let groups = id(as_admin(
        "POST",
        "/api/collections".into(),
        json!({
            "name": "groups", "fields": [{ "name": "name", "type": "text" }],
        }),
    )
    .await);
    let teams = id(as_admin(
        "POST",
        "/api/collections".into(),
        json!({
            "name": "teams", "fields": [
                { "name": "name", "type": "text" },
                { "name": "owner", "type": "relation", "collectionId": users, "maxSelect": 1 },
                { "name": "settings", "type": "json" },
            ],
        }),
    )
    .await);
    let mut fields =
        as_admin("GET", "/api/collections/members".into(), Value::Null).await["fields"]
            .as_array()
            .unwrap()
            .clone();
    fields.extend([
        json!({ "name": "team", "type": "relation", "collectionId": teams, "maxSelect": 1 }),
        json!({ "name": "groups", "type": "relation", "collectionId": groups, "maxSelect": 5 }),
    ]);
    as_admin(
        "PATCH",
        "/api/collections/members".into(),
        json!({ "fields": fields }),
    )
    .await;
    let docs = json!({
        "name": "docs",
        "fields": [
            { "name": "title", "type": "text" },
            { "name": "team", "type": "relation", "collectionId": teams, "maxSelect": 1 },
        ],
        // team owners see their team's docs; members of the "editors" group see all of them;
        // and anyone whose team allows it (a JSON path through a relation)
        "listRule": "(team = @request.auth.team && @request.auth.team.owner = @request.auth.id) \
                     || @request.auth.groups.name ?= 'editors' \
                     || (team = @request.auth.team && @request.auth.team.settings.open = true)",
    });
    as_admin("POST", "/api/collections".into(), docs).await;

    // a path the auth collections don't have is rejected when the rule is saved
    let (status, res) = request(
        &app,
        "PATCH",
        "/api/collections/docs",
        &[("authorization", admin_auth.as_str())],
        Some(json!({ "viewRule": "@request.auth.team.nope = 1" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(
        res["data"]["viewRule"]["message"]
            .as_str()
            .unwrap()
            .contains("nope"),
        "{res}"
    );

    let editors = id(as_admin(
        "POST",
        "/api/collections/groups/records".into(),
        json!({ "name": "editors" }),
    )
    .await);
    let others = id(as_admin(
        "POST",
        "/api/collections/groups/records".into(),
        json!({ "name": "others" }),
    )
    .await);
    let mut members = Vec::new();
    for name in ["owner", "member", "editor", "loner"] {
        let groups = if name == "editor" {
            json!([others, editors])
        } else {
            json!([others])
        };
        members.push(id(as_admin(
            "POST",
            "/api/collections/members/records".into(),
            json!({
                "email": format!("{name}@example.com"), "password": "password-123",
                "passwordConfirm": "password-123", "name": name, "groups": groups,
            }),
        )
        .await));
    }
    let red = id(as_admin(
        "POST",
        "/api/collections/teams/records".into(),
        json!({
            "name": "red", "owner": members[0], "settings": { "open": false },
        }),
    )
    .await);
    let blue = id(as_admin(
        "POST",
        "/api/collections/teams/records".into(),
        json!({
            "name": "blue", "owner": members[3], "settings": { "open": true },
        }),
    )
    .await);
    for m in &members[..3] {
        as_admin(
            "PATCH",
            format!("/api/collections/members/records/{m}"),
            json!({ "team": red }),
        )
        .await;
    }
    as_admin(
        "PATCH",
        format!("/api/collections/members/records/{}", members[3]),
        json!({ "team": blue }),
    )
    .await;
    for (title, team) in [("red doc", &red), ("blue doc", &blue)] {
        as_admin(
            "POST",
            "/api/collections/docs/records".into(),
            json!({ "title": title, "team": team }),
        )
        .await;
    }

    let titles = |token: Option<String>| {
        let app = app.clone();
        async move {
            let auth = token.map(|t| format!("Bearer {t}"));
            let headers: Vec<(&str, &str)> =
                auth.iter().map(|a| ("authorization", a.as_str())).collect();
            let (status, res) = request(
                &app,
                "GET",
                "/api/collections/docs/records?sort=title",
                &headers,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{res}");
            res["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["title"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    let token = |email: &str| {
        let (app, email) = (app.clone(), email.to_string());
        async move {
            let (status, res) = request(
                &app,
                "POST",
                "/api/collections/members/auth-with-password",
                &[],
                Some(json!({ "identity": email, "password": "password-123" })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{res}");
            Some(res["token"].as_str().unwrap().to_string())
        }
    };
    assert_eq!(
        titles(token("owner@example.com").await).await,
        ["red doc"],
        "the team's owner"
    );
    assert!(
        titles(token("member@example.com").await).await.is_empty(),
        "a member who doesn't own the team"
    );
    assert_eq!(
        titles(token("editor@example.com").await).await,
        ["blue doc", "red doc"],
        "a group member"
    );
    assert_eq!(
        titles(token("loner@example.com").await).await,
        ["blue doc"],
        "an open team (and its owner)"
    );
    assert!(titles(None).await.is_empty(), "guests: the paths are null");
    // a superuser (another auth collection, without these fields) gets everything anyway
    let _ = tdb;
}
