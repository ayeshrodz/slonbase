//! The figures the dashboard shows about requests and records (the Overview, Health and the
//! command center): what a request is, which requests the dashboard's own pages make, and that
//! every place counts the same way. See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

const HOST: &str = "dash.example.com";

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// The headers of a request made by a page of the dashboard.
fn from_dashboard<'a>(auth: &'a str, referer: &'a str) -> [(&'a str, &'a str); 3] {
    [
        ("authorization", auth),
        ("host", HOST),
        ("referer", referer),
    ]
}

/// The status as the dashboard reads it (a read the figures themselves leave out), once the
/// entries written so far are in the database: the figure is the same twice in a row.
async fn settled_status(app: &Arc<App>, auth: &str) -> Value {
    let page = format!("https://{HOST}/_/#/health");
    let mut last = None;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(700)).await;
        let (status, body) = request(
            app,
            "GET",
            "/api/system/status",
            &from_dashboard(auth, &page),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let requests = body["logs"]["requests24h"].as_i64().unwrap();
        if last == Some(requests) {
            return body;
        }
        last = Some(requests);
    }
    panic!("the request count did not settle");
}

#[tokio::test]
async fn the_dashboards_own_requests_are_not_counted() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = bearer(&token);
    let before = settled_status(&app, &auth).await["logs"]["requests24h"]
        .as_i64()
        .unwrap();

    // the application's: counted
    for _ in 0..3 {
        let (status, _) = request(&app, "GET", "/api/collections/nope/records", &[], None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    // the dashboard's own pages: the main installation's and a project's, with the host the
    // browser used (a proxy's forwarded host counts too)
    for referer in [
        format!("https://{HOST}/_/#/data"),
        format!("https://{HOST}/_/"),
        format!("https://{HOST}/p/demo/_/#/logs"),
    ] {
        let (status, _) = request(
            &app,
            "GET",
            "/api/collections",
            &from_dashboard(&auth, &referer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = request(
        &app,
        "GET",
        "/api/collections",
        &[
            ("authorization", auth.as_str()),
            ("host", "backend:8090"),
            ("x-forwarded-host", HOST),
            ("referer", &format!("https://{HOST}/_/#/data")),
        ],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // not the dashboard's: another site's page, another path, a look-alike path, no referer
    for referer in [
        "https://other.example.com/_/#/data".to_string(),
        format!("https://{HOST}/api/docs"),
        format!("https://{HOST}/_x/"),
        format!("https://{HOST}.evil.example/_/"),
        String::new(),
    ] {
        let headers = if referer.is_empty() {
            vec![("authorization", auth.as_str()), ("host", HOST)]
        } else {
            vec![
                ("authorization", auth.as_str()),
                ("host", HOST),
                ("referer", referer.as_str()),
            ]
        };
        let (status, _) = request(&app, "GET", "/api/collections", &headers, None).await;
        assert_eq!(status, StatusCode::OK);
    }

    let status = settled_status(&app, &auth).await;
    let counted = status["logs"]["requests24h"].as_i64().unwrap() - before;
    assert_eq!(
        counted,
        3 + 5,
        "3 requests of the application and 5 that only look like the dashboard's: {status}"
    );

    // the entries say which they are, and the logs keep all of them
    let (_, list) = request(
        &app,
        "GET",
        "/api/logs?perPage=100&filter=data.dashboard%20%3D%20true",
        &[("authorization", auth.as_str())],
        None,
    )
    .await;
    let marked = list["items"].as_array().unwrap();
    assert!(
        marked.len() >= 4,
        "the four requests of the dashboard are marked: {list}"
    );
    assert!(marked.iter().all(|e| e["data"]["dashboard"] == true));
    let (_, list) = request(
        &app,
        "GET",
        "/api/logs?perPage=100&filter=data.url%20%3D%20%27%2Fapi%2Fcollections%27%20%26%26%20data.dashboard%20!%3D%20true",
        &[("authorization", auth.as_str())],
        None,
    )
    .await;
    assert_eq!(
        list["items"].as_array().unwrap().len(),
        5,
        "the others carry no mark: {list}"
    );
    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn the_dashboards_files_and_redirect_are_not_requests() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = bearer(&token);
    let before = settled_status(&app, &auth).await["logs"]["requests24h"]
        .as_i64()
        .unwrap();

    // a visit: the page and its files (answered, then answered "not modified" as the browser's
    // copy is current), the redirect from the address's root, the health check
    let (status, headers, _) = raw_request(&app, "GET", "/_/", &[], vec![]).await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").expect("the page has an ETag").clone();
    for _ in 0..5 {
        let (status, _, _) = raw_request(
            &app,
            "GET",
            "/_/",
            &[("if-none-match", etag.to_str().unwrap())],
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
    }
    for _ in 0..3 {
        let (status, _, _) = raw_request(&app, "GET", "/", &[], vec![]).await;
        assert!(status.is_redirection(), "{status}");
        raw_request(&app, "GET", "/api/health", &[], vec![]).await;
    }
    // a failure is logged: it is one
    let (status, _, _) = raw_request(&app, "GET", "/_/nope/missing.js", &[], vec![]).await;
    let failed = status.as_u16() >= 400;

    let status = settled_status(&app, &auth).await;
    let counted = status["logs"]["requests24h"].as_i64().unwrap() - before;
    assert_eq!(counted, i64::from(failed), "{status}");
    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn the_hours_add_up_to_the_day_and_the_settings_that_hide_requests_are_reported() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = bearer(&token);
    for _ in 0..4 {
        request(&app, "GET", "/api/collections/nope/records", &[], None).await;
    }
    let status = settled_status(&app, &auth).await;
    let hours: Vec<i64> = status["logs"]["requestsPerHour"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_i64().unwrap())
        .collect();
    assert_eq!(hours.len(), 24, "{status}");
    assert_eq!(
        hours.iter().sum::<i64>(),
        status["logs"]["requests24h"].as_i64().unwrap(),
        "the 24 hours are the day: {status}"
    );
    assert!(
        *hours.last().unwrap() >= 4,
        "the latest hour is the last one: {hours:?}"
    );
    // everything is logged: nothing to say
    assert_eq!(status["logging"]["partial"], "");
    assert_eq!(status["logging"]["dropped"], 0);
    let ids = |status: &Value| -> Vec<String> {
        status["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(!ids(&status).contains(&"logging-partial".to_string()));

    // paths left out, and a level above info: the figures are what is kept, and say so
    let dashboard = format!("https://{HOST}/_/#/settings/logs");
    let (code, body) = request(
        &app,
        "PATCH",
        "/api/settings",
        &from_dashboard(&auth, &dashboard),
        Some(json!({ "logs": { "excludePaths": ["/api/files/"], "minLevel": 4 } })),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let (_, status) = request(
        &app,
        "GET",
        "/api/system/status",
        &from_dashboard(&auth, &dashboard),
        None,
    )
    .await;
    let partial = status["logging"]["partial"].as_str().unwrap();
    assert!(
        partial.contains("minimum level") && partial.contains("leaves out"),
        "{partial}"
    );
    assert!(ids(&status).contains(&"logging-partial".to_string()));

    // logging off is its own state, not a partial one
    let (code, _) = request(
        &app,
        "PATCH",
        "/api/settings",
        &from_dashboard(&auth, &dashboard),
        Some(json!({ "logs": { "maxDays": 0 } })),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let (_, status) = request(
        &app,
        "GET",
        "/api/system/status",
        &from_dashboard(&auth, &dashboard),
        None,
    )
    .await;
    assert_eq!(status["logging"]["enabled"], false);
    assert_eq!(status["logging"]["partial"], "");
    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn records_are_counted_the_same_everywhere() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = bearer(&token);
    let headers = [("authorization", auth.as_str())];
    for (name, records) in [("a", 2), ("b", 0), ("c", 5)] {
        let (status, body) = request(
            &app,
            "POST",
            "/api/collections",
            &headers,
            Some(json!({
                "name": name, "type": "base", "createRule": "",
                "fields": [{ "name": "t", "type": "text" }],
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        for _ in 0..records {
            let (status, _) = request(
                &app,
                "POST",
                &format!("/api/collections/{name}/records"),
                &[],
                Some(json!({ "t": "x" })),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
    }
    let (status, body) = request(
        &app,
        "POST",
        "/api/collections",
        &headers,
        Some(json!({
            "name": "v", "type": "view", "viewQuery": "SELECT id, t FROM a",
        })),
    )
    .await;
    // (a view needs the SQL console on some builds: it is only left out of the counts)
    let view = status == StatusCode::OK;
    let _ = body;

    // everyone's request is refused but a superuser's
    let (status, _) = request(&app, "GET", "/api/collections/meta/counts", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, counts) =
        request(&app, "GET", "/api/collections/meta/counts", &headers, None).await;
    assert_eq!(status, StatusCode::OK, "{counts}");
    let (_, collections) =
        request(&app, "GET", "/api/collections?perPage=100", &headers, None).await;
    let id_of = |name: &str| {
        collections["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(counts["counts"][id_of("a")], 2, "{counts}");
    assert_eq!(counts["counts"][id_of("b")], 0, "{counts}");
    assert_eq!(counts["counts"][id_of("c")], 5, "{counts}");
    assert_eq!(counts["estimated"], json!([]));
    if view {
        assert!(
            counts["counts"].get(id_of("v")).is_none(),
            "views have no table"
        );
    }
    // the system collections are there (the dashboard leaves them out of the total)
    assert!(counts["counts"][id_of("_superusers")].as_i64().unwrap() >= 1);

    // the command center's total is the user collections', not the system ones'
    let (status, overview) = request(&app, "GET", "/api/projects/overview", &headers, None).await;
    assert_eq!(status, StatusCode::OK, "{overview}");
    let main = &overview["items"][0];
    assert_eq!(main["records"], 7, "2 + 0 + 5, no superusers: {main}");
    assert_eq!(main["collections"], 3 + i64::from(view), "{main}");
    drop(app);
    tdb.drop_db().await;
}
