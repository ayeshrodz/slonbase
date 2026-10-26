//! Prometheus metrics (`--metrics-bind`). See `setup_flow.rs` for how to point the tests at a
//! database.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

use common::*;

#[tokio::test]
async fn counts_requests_by_route() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, admin, _dir) = ready_app(&tdb).await;
    let auth = format!("Bearer {admin}");
    let (status, _) = request(
        &app,
        "POST",
        "/api/collections",
        &[("authorization", auth.as_str())],
        Some(json!({ "name": "notes", "listRule": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for _ in 0..3 {
        request(&app, "GET", "/api/collections/notes/records", &[], None).await;
    }
    request(&app, "GET", "/api/collections/nope/records", &[], None).await;

    let res = slonbase::metrics::router(app.clone())
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    let text =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let value = |prefix: &str| -> f64 {
        text.lines()
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix} in\n{text}"))
            .rsplit(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    // by route pattern, not by URL (the metrics are shared by this binary's tests: at least)
    let records = r#"slonbase_http_requests_total{project="",method="GET",route="/api/collections/{collection}/records",status="200"}"#;
    assert!(value(records) >= 3.0, "{text}");
    let missing = r#"slonbase_http_requests_total{project="",method="GET",route="/api/collections/{collection}/records",status="404"}"#;
    assert!(value(missing) >= 1.0);
    assert!(
        !text.contains("/api/collections/notes"),
        "URLs aren't labels"
    );
    let bucket = r#"slonbase_http_request_duration_seconds_bucket{project="",method="GET",route="/api/collections/{collection}/records",le="+Inf"}"#;
    assert!(value(bucket) >= 4.0);
    assert!(value(r#"slonbase_db_connections{state="max"}"#) > 0.0);
    assert_eq!(value("slonbase_instances"), 1.0);
    assert!(text.contains(&format!(
        "slonbase_info{{version=\"{}\"}} 1",
        env!("CARGO_PKG_VERSION")
    )));
    drop(app);
    tdb.drop_db().await;
}
