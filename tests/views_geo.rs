//! Geo Point fields and view collections. See `setup_flow.rs` for how to point the tests at
//! a database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

async fn call(
    app: &Arc<App>,
    token: Option<&str>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = token.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.iter().map(|a| ("authorization", a.as_str())).collect();
    request(app, method, uri, &headers, body).await
}

fn q(filter: &str) -> String {
    form_urlencoded::byte_serialize(filter.as_bytes()).collect()
}

#[tokio::test]
async fn geo_points() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let admin = Some(token.as_str());
    let (status, res) = call(
        &app,
        admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "places", "listRule": "", "viewRule": "", "createRule": "",
            "fields": [
                { "name": "name", "type": "text" },
                { "name": "loc", "type": "geoPoint" },
                { "name": "home", "type": "geoPoint", "required": true },
            ],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["fields"][2]["type"], "geoPoint");

    // Sofia, Plovdiv, London
    for (name, lon, lat) in [
        ("sofia", 23.3219, 42.6977),
        ("plovdiv", 24.7453, 42.1354),
        ("london", -0.1276, 51.5072),
    ] {
        let (status, res) = call(
            &app,
            None,
            "POST",
            "/api/collections/places/records",
            Some(json!({
                "name": name, "loc": { "lon": lon, "lat": lat }, "home": { "lon": "1", "lat": "2" },
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{res}");
        assert_eq!(res["loc"], json!({ "lon": lon, "lat": lat }));
        assert_eq!(
            res["home"],
            json!({ "lon": 1, "lat": 2 }),
            "numeric strings are parsed"
        );
    }
    // defaults, ranges and required
    let (status, res) = call(
        &app,
        None,
        "POST",
        "/api/collections/places/records",
        Some(json!({ "name": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(res["data"]["home"]["code"], "validation_required", "{res}");
    let (status, res) = call(
        &app,
        None,
        "POST",
        "/api/collections/places/records",
        Some(json!({ "name": "x", "home": { "lon": 200, "lat": 0 } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        res["data"]["home"]["message"]
            .as_str()
            .unwrap()
            .contains("longitude"),
        "{res}"
    );

    // filters: subfields and geoDistance (km)
    let (_, list) = call(
        &app,
        None,
        "GET",
        &format!(
            "/api/collections/places/records?sort=name&filter={}",
            q("geoDistance(loc.lon, loc.lat, 23.3219, 42.6977) < 200")
        ),
        None,
    )
    .await;
    let names: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["plovdiv", "sofia"], "{list}");
    let (_, list) = call(
        &app,
        None,
        "GET",
        &format!(
            "/api/collections/places/records?filter={}",
            q("loc.lat > 50")
        ),
        None,
    )
    .await;
    assert_eq!(list["items"][0]["name"], "london");
    let (status, _) = call(
        &app,
        None,
        "GET",
        &format!(
            "/api/collections/places/records?filter={}",
            q("geoDistance(loc.lon) < 1")
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn view_collections() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app_configured(&tdb, slonbase::Extensions::default(), |o| {
        o.sql_console = Some(true)
    })
    .await;
    let admin = Some(token.as_str());
    call(
        &app,
        admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "orders", "listRule": "", "viewRule": "", "createRule": "",
            "fields": [
                { "name": "customer", "type": "text" },
                { "name": "total", "type": "number" },
                { "name": "paid", "type": "bool" },
            ],
        })),
    )
    .await;
    for (customer, total, paid) in [("ann", 10, true), ("ann", 15, false), ("bob", 7, true)] {
        call(
            &app,
            None,
            "POST",
            "/api/collections/orders/records",
            Some(json!({ "customer": customer, "total": total, "paid": paid })),
        )
        .await;
    }

    let query = "SELECT customer AS id, customer, sum(total) AS spent, count(*) AS orders, bool_and(paid) AS all_paid, now() AS last FROM orders GROUP BY customer";
    let (status, view) = call(
        &app,
        admin,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "customers", "type": "view", "listRule": "", "viewRule": "spent > 0",
            "createRule": "", "viewQuery": query,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let types: Vec<(String, String)> = view["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["name"].as_str().unwrap().to_string(),
                f["type"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        types,
        vec![
            ("id".into(), "text".into()),
            ("customer".into(), "text".into()),
            ("spent".into(), "number".into()),
            ("orders".into(), "number".into()),
            ("all_paid".into(), "bool".into()),
            ("last".into(), "date".into()),
        ]
    );
    assert_eq!(view["createRule"], Value::Null, "views are read-only");
    assert_eq!(view["viewQuery"], query);

    // read like any collection: filters, sorts, rules
    let (status, list) = call(
        &app,
        None,
        "GET",
        "/api/collections/customers/records?sort=-spent",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["totalItems"], 2);
    assert_eq!(list["items"][0]["id"], "ann");
    assert_eq!(list["items"][0]["spent"], 25);
    assert_eq!(list["items"][0]["all_paid"], false);
    assert_eq!(list["items"][1]["orders"], 1);
    let (_, one) = call(
        &app,
        None,
        "GET",
        "/api/collections/customers/records/bob",
        None,
    )
    .await;
    assert_eq!(one["spent"], 7);
    let (_, filtered) = call(
        &app,
        None,
        "GET",
        &format!(
            "/api/collections/customers/records?filter={}",
            q("all_paid = true")
        ),
        None,
    )
    .await;
    assert_eq!(filtered["items"][0]["id"], "bob");
    // new orders show up at once
    call(
        &app,
        None,
        "POST",
        "/api/collections/orders/records",
        Some(json!({ "customer": "cy", "total": 3, "paid": true })),
    )
    .await;
    let (_, list) = call(
        &app,
        None,
        "GET",
        "/api/collections/customers/records",
        None,
    )
    .await;
    assert_eq!(list["totalItems"], 3);

    // read-only
    let (status, res) = call(
        &app,
        admin,
        "POST",
        "/api/collections/customers/records",
        Some(json!({ "id": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(res["message"], "View collections are read-only.");
    let (status, _) = call(
        &app,
        admin,
        "DELETE",
        "/api/collections/customers/records/ann",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // changing the query keeps field options of the same names
    let (status, res) = call(
        &app,
        admin,
        "PATCH",
        "/api/collections/customers",
        Some(json!({
            "fields": view["fields"].as_array().unwrap().iter().map(|f| {
                let mut f = f.clone();
                if f["name"] == "customer" { f["presentable"] = json!(true); }
                f
            }).collect::<Vec<_>>(),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, res) = call(&app, admin, "PATCH", "/api/collections/customers", Some(json!({
        "viewQuery": "SELECT customer AS id, customer, sum(total) AS spent FROM slonbase.orders GROUP BY customer",
    }))).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["fields"].as_array().unwrap().len(), 3);
    assert_eq!(res["fields"][1]["presentable"], true);
    assert_eq!(res["fields"][1]["id"], view["fields"][1]["id"]);

    // invalid queries
    for (query, needle) in [
        ("SELECT 1 AS x", "id column"),
        ("DELETE FROM slonbase.orders", "SELECT"),
        ("SELECT id FROM slonbase.nope", "does not exist"),
        ("SELECT id, id FROM slonbase.orders", "more than once"),
    ] {
        let (status, res) = call(
            &app,
            admin,
            "POST",
            "/api/collections",
            Some(json!({ "name": "bad", "type": "view", "viewQuery": query })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {res}");
        assert!(
            res["data"]["viewQuery"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains(needle),
            "{query}: {res}"
        );
    }

    // a table a view uses can't lose the column
    let (_, orders) = call(&app, admin, "GET", "/api/collections/orders", None).await;
    let fields: Vec<Value> = orders["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["name"] != "total")
        .cloned()
        .collect();
    let (status, res) = call(
        &app,
        admin,
        "PATCH",
        "/api/collections/orders",
        Some(json!({ "fields": fields })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["message"].as_str().unwrap().contains("view"), "{res}");

    // backups recreate views after their tables
    let (status, _) = call(
        &app,
        admin,
        "POST",
        "/api/backups",
        Some(json!({ "name": "v.zip" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, res) = call(&app, admin, "POST", "/api/backups/v.zip/restore", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let (_, list) = call(
        &app,
        None,
        "GET",
        "/api/collections/customers/records",
        None,
    )
    .await;
    assert_eq!(list["totalItems"], 3, "{list}");

    let (status, _) = call(&app, admin, "DELETE", "/api/collections/customers", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn views_need_the_sql_console() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, res) = call(
        &app,
        Some(&token),
        "POST",
        "/api/collections",
        Some(json!({
            "name": "v", "type": "view", "viewQuery": "SELECT id FROM slonbase._superusers",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");
    drop(app);
    tdb.drop_db().await;
}
