//! Cron jobs made in the dashboard (`/api/crons`): making, listing, changing, running and
//! deleting them, their checks, who may do what, that the scheduler runs them once a minute,
//! and that `/api/settings` never shows them.
//! See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

async fn call(
    app: &Arc<App>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    request(app, method, uri, &[("authorization", auth.as_str())], body).await
}

async fn eventually(mut check: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..150 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    false
}

async fn records(app: &Arc<App>, token: &str) -> i64 {
    let (status, res) = call(app, token, "GET", "/api/collections/events/records", None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    res["totalItems"].as_i64().unwrap()
}

async fn add(app: &Arc<App>, token: &str, title: &str) {
    let (status, res) = call(
        app,
        token,
        "POST",
        "/api/collections/events/records",
        Some(json!({ "title": title })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
}

fn cleanup(name: &str, filter: &str) -> Value {
    json!({
        "name": name,
        "expression": "0 3 * * *",
        "type": "cleanup",
        "config": { "collection": "events", "filter": filter },
    })
}

#[tokio::test]
async fn jobs_made_in_the_dashboard() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({
            "name": "events",
            "fields": [{ "name": "title", "type": "text" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    for title in ["old a", "keep", "old b"] {
        add(&app, &token, title).await;
    }

    // what can't be saved says why, field by field
    for (body, field) in [
        (cleanup("", "title ~ 'old'"), "name"),
        (
            json!({ "name": "x", "expression": "nope", "type": "cleanup",
                    "config": { "collection": "events", "filter": "title ~ 'old'" } }),
            "expression",
        ),
        (
            json!({ "name": "x", "expression": "0 3 * * *", "type": "shell", "config": {} }),
            "type",
        ),
        (
            json!({ "name": "x", "expression": "0 3 * * *", "type": "cleanup",
                    "config": { "collection": "missing", "filter": "a = 1" } }),
            "config",
        ),
        (
            json!({ "name": "x", "expression": "0 3 * * *", "type": "cleanup",
                    "config": { "collection": "_superusers", "filter": "id != ''" } }),
            "config",
        ),
        // nothing to go by: it would delete every record
        (
            json!({ "name": "x", "expression": "0 3 * * *", "type": "cleanup",
                    "config": { "collection": "events" } }),
            "config",
        ),
        (cleanup("x", "nonexistent_field = 1"), "config"),
        // the age needs a created field, which this collection doesn't have
        (
            json!({ "name": "x", "expression": "0 3 * * *", "type": "cleanup",
                    "config": { "collection": "events", "olderThanDays": 7 } }),
            "config",
        ),
        (
            json!({ "name": "x", "expression": "0 3 * * *", "type": "cleanup",
                    "config": { "collection": "events", "olderThanDays": 0 } }),
            "config",
        ),
    ] {
        let (status, res) = call(&app, &token, "POST", "/api/crons", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {res}");
        assert!(res["data"][field].is_object(), "{field}: {res}");
    }
    assert_eq!(records(&app, &token).await, 3);

    // the preview says when it runs and how many records match, and saves nothing
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/cron-preview",
        Some(cleanup("preview", "title ~ 'old'")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["matching"], 2, "{res}");
    assert_eq!(res["next"].as_array().unwrap().len(), 5);
    assert_eq!(res["timezone"], "UTC");
    let (_, listed) = call(&app, &token, "GET", "/api/crons", None).await;
    assert!(listed.as_array().unwrap().is_empty(), "{listed}");

    // make one
    let (status, job) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(cleanup("Old events", "title ~ 'old'")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{job}");
    let id = job["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("user:"), "{id}");
    assert_eq!(job["enabled"], true);
    assert_eq!(job["config"]["maxRows"], 10_000, "defaults are filled in");
    // names are unique
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(cleanup("old EVENTS", "title ~ 'x'")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"]["name"].is_object());

    // it is listed, with its next run, and never in the settings
    let (_, listed) = call(&app, &token, "GET", "/api/crons", None).await;
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], id.as_str());
    assert_eq!(listed[0]["kind"], "user");
    assert_eq!(listed[0]["type"], "cleanup");
    assert_eq!(listed[0]["description"], "every day at 03:00");
    assert!(listed[0]["next"].is_string());
    let (_, settings) = call(&app, &token, "GET", "/api/settings", None).await;
    assert!(settings.get("cronJobs").is_none(), "{settings}");

    // run it now: the records go, and the history says how many
    let (status, _) = call(&app, &token, "POST", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        eventually(async || records(&app, &token).await == 1).await,
        "the old events are deleted"
    );
    let query = format!("/api/jobs/runs?kind=cron&job={id}");
    let mut run = Value::Null;
    assert!(
        eventually(async || {
            let (_, res) = call(&app, &token, "GET", &query, None).await;
            run = res["items"][0].clone();
            run["status"] == "ok"
        })
        .await,
        "{run}"
    );
    assert_eq!(run["trigger"], "manual");
    assert_eq!(run["meta"]["deleted"], 2, "{run}");

    // change it, then switch it off: no next run
    let (status, job) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "name": "Old events (nightly)", "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{job}");
    assert_eq!(job["name"], "Old events (nightly)");
    assert!(job["next"].is_null(), "{job}");
    let (status, _) = call(
        &app,
        &token,
        "PATCH",
        "/api/crons/user:nope",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // delete it
    let (status, _) = call(&app, &token, "DELETE", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, listed) = call(&app, &token, "GET", "/api/crons", None).await;
    assert!(listed.as_array().unwrap().is_empty());
    let (status, _) = call(&app, &token, "DELETE", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, &token, "POST", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    drop(app);
    tdb.drop_db().await;
}

#[tokio::test]
async fn the_scheduler_runs_a_due_job_once_a_minute() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let core = app.core().unwrap();
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "events", "fields": [{ "name": "title", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let mut body = cleanup("Every minute", "title ~ 'old'");
    body["expression"] = "* * * * *".into();
    let (status, job) = call(&app, &token, "POST", "/api/crons", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{job}");
    let id = job["id"].as_str().unwrap().to_string();
    let mut off = cleanup("Off", "title ~ 'old'");
    off["expression"] = "* * * * *".into();
    off["enabled"] = false.into();
    let (status, _) = call(&app, &token, "POST", "/api/crons", Some(off)).await;
    assert_eq!(status, StatusCode::OK);

    // two ticks in one minute run it once (a tick that straddles the minute may run it twice)
    let minute = || chrono::Utc::now().format("%H:%M").to_string();
    let before = minute();
    slonbase::hooks::run_scheduled_crons(&core).await;
    slonbase::hooks::run_scheduled_crons(&core).await;
    let same_minute = before == minute();
    let query = format!("/api/jobs/runs?kind=cron&job={id}");
    assert!(
        eventually(async || {
            let (_, res) = call(&app, &token, "GET", &query, None).await;
            res["total"].as_i64().unwrap_or(0) >= 1 && res["items"][0]["status"] != "running"
        })
        .await
    );
    let (_, res) = call(&app, &token, "GET", &query, None).await;
    if same_minute {
        assert_eq!(res["total"], 1, "{res}");
    }
    assert_eq!(res["items"][0]["trigger"], "schedule");
    // the disabled job never ran
    let (_, all) = call(&app, &token, "GET", "/api/jobs/runs?kind=cron", None).await;
    assert!(
        all["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["job"] == id.as_str()),
        "{all}"
    );

    drop((app, core));
    tdb.drop_db().await;
}

#[tokio::test]
async fn who_may_make_jobs() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "events", "fields": [{ "name": "title", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, job) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(cleanup("Old events", "title ~ 'old'")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{job}");
    let id = job["id"].as_str().unwrap().to_string();

    // a key of each role stands for a person of that role
    let mut keys = Vec::new();
    for role in ["viewer", "developer", "admin"] {
        let (status, key) = call(
            &app,
            &token,
            "POST",
            "/api/team/keys",
            Some(json!({ "name": role, "role": role })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{key}");
        keys.push((role, key["key"].as_str().unwrap().to_string()));
    }
    for (role, key) in &keys {
        // everyone looks, developers and up run, only admins and owners change
        let (status, _) = call(&app, key, "GET", "/api/crons", None).await;
        assert_eq!(status, StatusCode::OK, "{role} lists");
        let (status, _) = call(&app, key, "POST", &format!("/api/crons/{id}"), None).await;
        let may_run = *role != "viewer";
        assert_eq!(
            status == StatusCode::NO_CONTENT,
            may_run,
            "{role} runs: {status}"
        );
        let make = ["admin"].contains(role);
        let (status, res) = call(
            &app,
            key,
            "POST",
            "/api/crons",
            Some(cleanup(&format!("by {role}"), "title ~ 'z'")),
        )
        .await;
        assert_eq!(status == StatusCode::OK, make, "{role} makes: {res}");
        let (status, _) = call(
            &app,
            key,
            "PATCH",
            &format!("/api/crons/{id}"),
            Some(json!({ "name": format!("renamed by {role}") })),
        )
        .await;
        assert_eq!(status == StatusCode::OK, make, "{role} changes: {status}");
        let (status, _) = call(
            &app,
            key,
            "POST",
            "/api/cron-preview",
            Some(cleanup("p", "a = 1")),
        )
        .await;
        assert_eq!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::OK,
            make,
            "{role} previews: {status}"
        );
        if !make {
            assert_eq!(status, StatusCode::FORBIDDEN, "{role} previews");
            let (status, _) = call(&app, key, "DELETE", &format!("/api/crons/{id}"), None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{role} deletes");
        }
    }

    drop(app);
    tdb.drop_db().await;
}

fn url_job(name: &str, url: &str) -> Value {
    json!({
        "name": name,
        "expression": "*/5 * * * *",
        "type": "http",
        "config": {
            "url": url,
            "method": "POST",
            "headers": { "Authorization": "Bearer s3cret" },
            "body": "{\"ping\":true}",
        },
    })
}

#[tokio::test]
async fn jobs_that_call_a_url() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;

    // this server, private networks and the metadata address can't be called, nor odd URLs
    for url in [
        "http://127.0.0.1:8090/api/health",
        "http://localhost/",
        "http://10.1.2.3/hook",
        "http://169.254.169.254/latest/meta-data/",
        "http://[::1]/",
        "file:///etc/passwd",
        "https://user:pass@93.184.216.34/",
        "not a url",
    ] {
        let (status, res) = call(
            &app,
            &token,
            "POST",
            "/api/crons",
            Some(url_job("ping", url)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url}: {res}");
        assert!(res["data"]["config"]["url"].is_object(), "{url}: {res}");
    }
    let mut odd = url_job("ping", "http://93.184.216.34/hook");
    odd["config"]["method"] = json!("TRACE");
    odd["config"]["timeoutSeconds"] = json!(600);
    odd["config"]["headers"] = json!({ "Host": "x", "bad name": "y" });
    let (status, res) = call(&app, &token, "POST", "/api/crons", Some(odd)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    for field in ["method", "timeoutSeconds", "headers"] {
        assert!(res["data"]["config"][field].is_object(), "{field}: {res}");
    }

    // a public address is fine; the header's value is a secret: shown masked, kept on edit
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(url_job("ping", "http://93.184.216.34/hook")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let id = res["id"].as_str().unwrap().to_string();
    assert_eq!(res["type"], "http");
    assert_eq!(res["config"]["headers"]["Authorization"], "••••••••");
    assert!(!res.to_string().contains("s3cret"), "{res}");

    let (_, list) = call(&app, &token, "GET", "/api/crons", None).await;
    assert!(!list.to_string().contains("s3cret"), "{list}");
    let (_, settings) = call(&app, &token, "GET", "/api/settings", None).await;
    assert!(!settings.to_string().contains("s3cret"), "{settings}");

    // sending the mask back changes nothing; a new value replaces it
    let core = app.core().unwrap();
    let saved = |core: &slonbase::core::Core| {
        core.settings().cron_jobs[0].config["headers"]["Authorization"].clone()
    };
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "config": {
            "url": "http://93.184.216.34/hook",
            "headers": { "Authorization": "••••••••" },
        } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(saved(&core), "Bearer s3cret");

    // a saved header only goes to the address it was saved with: pointing the job somewhere
    // else with the mask is refused, with a typed value it is fine
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "config": {
            "url": "http://93.184.216.34/other",
            "headers": { "Authorization": "••••••••" },
        } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"]["config"]["headers"].is_object(), "{res}");
    assert_eq!(saved(&core), "Bearer s3cret");
    assert_eq!(
        core.settings().cron_jobs[0].config["url"],
        "http://93.184.216.34/hook"
    );
    let (status, _) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "config": {
            "url": "http://93.184.216.34/other",
            "headers": { "Authorization": "Bearer new" },
        } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved(&core), "Bearer new");

    // running it now to a private address that got in some other way is refused at run time
    // and recorded as failed (the guard is on the request, not only on saving)
    let mut def = core.settings().cron_jobs[0].clone();
    def.config["url"] = json!("http://127.0.0.1:1/");
    let result = slonbase::cron_jobs::execute(&core, &def).await;
    assert!(result.unwrap_err().contains("not a public address"));
}

async fn last_run(app: &Arc<App>, token: &str, id: &str) -> Value {
    let query = format!("/api/jobs/runs?kind=cron&job={id}");
    let mut run = Value::Null;
    assert!(
        eventually(async || {
            let (_, res) = call(app, token, "GET", &query, None).await;
            run = res["items"][0].clone();
            run["status"].is_string() && run["status"] != "running"
        })
        .await,
        "the run is recorded"
    );
    run
}

#[tokio::test]
async fn jobs_that_send_email() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let mailer = Arc::new(slonbase::mail::MemoryMailer::default());
    app.core().unwrap().set_mailer(mailer.clone());
    let job = |to: Value, subject: &str| {
        json!({
            "name": "Daily note",
            "expression": "0 8 * * *",
            "type": "mail",
            "config": { "to": to, "subject": subject, "body": "Hello <b>team</b>\nsecond line" },
        })
    };

    // email has to be set up first
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(job(json!("ops@example.com"), "Hi")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"]["config"]["to"].is_object(), "{res}");
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "smtp": { "enabled": true, "host": "mail.acme.test", "port": 2525 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    for (to, subject, field) in [
        (json!([]), "Hi", "to"),
        (json!("not-an-address"), "Hi", "to"),
        (json!("a@b.co\nBcc: x@y.co"), "Hi", "to"),
        (json!("ops@example.com"), "", "subject"),
        (
            json!("ops@example.com"),
            "Hi\nBcc: evil@example.com",
            "subject",
        ),
        (
            json!(
                (0..11)
                    .map(|i| format!("u{i}@example.com"))
                    .collect::<Vec<_>>()
            ),
            "Hi",
            "to",
        ),
    ] {
        let (status, res) = call(
            &app,
            &token,
            "POST",
            "/api/crons",
            Some(job(to.clone(), subject)),
        )
        .await;
        // (a newline between addresses is a separator, so that one stays valid but harmless)
        if to == json!("a@b.co\nBcc: x@y.co") {
            assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
            continue;
        }
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {res}");
        assert!(res["data"]["config"][field].is_object(), "{field}: {res}");
    }

    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(job(json!("ops@example.com, dev@example.com"), "Morning")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let id = res["id"].as_str().unwrap().to_string();
    assert_eq!(
        res["config"]["to"],
        json!(["ops@example.com", "dev@example.com"])
    );

    let (status, _) = call(&app, &token, "POST", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let run = last_run(&app, &token, &id).await;
    assert_eq!(run["status"], "ok", "{run}");
    assert_eq!(run["meta"]["sent"], 2, "{run}");
    let sent = mailer.sent.lock().unwrap().clone();
    let tos: Vec<_> = sent.iter().map(|m| m.to.as_str()).collect();
    assert_eq!(tos, ["ops@example.com", "dev@example.com"]);
    assert_eq!(sent[0].subject, "Morning");
    // the text is text: markup is shown as typed, lines are kept
    assert_eq!(sent[0].html, "Hello &lt;b&gt;team&lt;/b&gt;<br>second line");
}

#[tokio::test]
async fn jobs_that_run_sql() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    // without the console there are no SQL jobs
    let (app, token, _dir) = ready_app(&tdb).await;
    let sql = |name: &str, statement: &str| {
        json!({ "name": name, "expression": "0 0 29 2 *", "type": "sql",
                "config": { "statement": statement } })
    };
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(sql("n", "SELECT 1")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"]["config"]["statement"].is_object(), "{res}");
    drop(app);

    let tdb2 = TestDb::create().await.unwrap();
    let (app, token, _dir) =
        ready_app_configured(&tdb2, Default::default(), |o| o.sql_console = Some(true)).await;
    // a read-only console: reads only, and what it refuses at save time is refused here too
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(sql("w", "DELETE FROM x")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    let (status, res) = call(&app, &token, "POST", "/api/crons", Some(sql("e", " "))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(sql("Count", "SELECT count(*) AS n FROM _slonbase.job_runs")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let id = res["id"].as_str().unwrap().to_string();

    // owners only: an admin (a key of that role) can neither make nor change one, may delete it
    let (_, key) = call(
        &app,
        &token,
        "POST",
        "/api/team/keys",
        Some(json!({ "name": "admin", "role": "admin" })),
    )
    .await;
    let admin = key["key"].as_str().unwrap().to_string();
    let (status, res) = call(
        &app,
        &admin,
        "POST",
        "/api/crons",
        Some(sql("Mine", "SELECT 1")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");
    let (status, _) = call(
        &app,
        &admin,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "config": { "statement": "SELECT 2" } })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &app,
        &admin,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "type": "http", "config": { "url": "http://93.184.216.34/" } })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "turning it into another type is a change too"
    );

    // it runs, and the history keeps what it did, not what it read
    let (status, _) = call(&app, &token, "POST", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let run = last_run(&app, &token, &id).await;
    assert_eq!(run["status"], "ok", "{run}");
    assert_eq!(run["meta"]["rows"], 1, "{run}");
    assert_eq!(run["meta"]["readOnly"], true, "{run}");

    let (status, _) = call(&app, &admin, "DELETE", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn failed_runs_are_tried_again_with_a_wait() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let job = |retries: Value, delay: Value| {
        json!({
            "name": "Always fails", "expression": "0 0 29 2 *", "type": "sql",
            "config": { "statement": "SELECT 1/0" },
            "retries": retries, "retryDelaySeconds": delay,
        })
    };

    // the bounds
    for (retries, delay, field) in [
        (json!(6), json!(30), "retries"),
        (json!(-1), json!(30), "retries"),
        (json!(2), json!(0), "retryDelaySeconds"),
        (json!(2), json!(301), "retryDelaySeconds"),
    ] {
        let (status, res) = call(
            &app,
            &token,
            "POST",
            "/api/crons",
            Some(job(retries, delay)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {res}");
        assert!(res["data"][field].is_object(), "{field}: {res}");
    }

    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(job(json!(1), json!(1))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(
        (res["retries"].as_i64(), res["retryDelaySeconds"].as_i64()),
        (Some(1), Some(1))
    );
    let id = res["id"].as_str().unwrap().to_string();

    // run now: it fails, waits a second, fails again, and the history says it tried twice
    let started = std::time::Instant::now();
    let (status, _) = call(&app, &token, "POST", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let run = last_run(&app, &token, &id).await;
    assert_eq!(run["status"], "error", "{run}");
    assert!(
        run["detail"].as_str().unwrap().contains("after 2 attempts"),
        "{run}"
    );
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "it waited before the retry"
    );

    // no retries: one attempt, and the plain message
    let (_, res) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "retries": 0 })),
    )
    .await;
    assert_eq!(res["retries"], 0);
    let (status, _) = call(&app, &token, "POST", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        eventually(async || {
            let (_, res) = call(
                &app,
                &token,
                "GET",
                &format!("/api/jobs/runs?kind=cron&job={id}"),
                None,
            )
            .await;
            res["items"][0]["status"] == "error"
                && !res["items"][0]["detail"]
                    .as_str()
                    .unwrap_or("")
                    .contains("attempts")
                && res["total"] == 2
        })
        .await,
        "a second run, with one attempt"
    );
}

#[tokio::test]
async fn a_failing_job_tells_its_people_once_and_again_when_it_works() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let core = app.core().unwrap();
    let mailer = Arc::new(slonbase::mail::MemoryMailer::default());
    core.set_mailer(mailer.clone());
    let job = |statement: &str, notify: Value| {
        json!({
            // (a schedule the test's own clock never meets: the runs below are the only ones)
            "name": "Nightly check", "expression": "0 0 29 2 *", "type": "sql",
            "config": { "statement": statement }, "notify": notify,
        })
    };

    // email has to be on, the addresses plausible, the webhook public
    let notify = json!({ "emails": ["ops@example.com"] });
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(job("SELECT 1/0", notify.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"]["notify"]["emails"].is_object(), "{res}");
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "smtp": { "enabled": true, "host": "mail.acme.test", "port": 2525 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    for (bad, field) in [
        (json!({ "emails": ["nope"] }), "emails"),
        (json!({ "emails": ["a@b.co\nBcc: x@y.co"] }), "emails"),
        (
            json!({ "webhook": "http://127.0.0.1:9000/hook" }),
            "webhook",
        ),
        (json!({ "webhook": "http://169.254.169.254/" }), "webhook"),
        (json!({ "webhook": "file:///etc/passwd" }), "webhook"),
    ] {
        let (status, res) = call(
            &app,
            &token,
            "POST",
            "/api/crons",
            Some(job("SELECT 1/0", bad.clone())),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {res}");
        assert!(res["data"]["notify"][field].is_object(), "{bad}: {res}");
    }

    // a webhook is a secret: shown hidden, and kept when the mask comes back
    let notify = json!({ "emails": ["ops@example.com", "dev@example.com"], "webhook": "http://93.184.216.34/hooks/T0K3N" });
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(job("SELECT 1/0", notify)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["notify"]["webhook"], "••••••••");
    assert!(!res.to_string().contains("T0K3N"), "{res}");
    let id = res["id"].as_str().unwrap().to_string();
    let (_, list) = call(&app, &token, "GET", "/api/crons", None).await;
    assert!(!list.to_string().contains("T0K3N"), "{list}");
    let (status, _) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "notify": { "emails": ["ops@example.com"], "webhook": "••••••••" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        core.settings().cron_jobs[0].notify.webhook,
        "http://93.184.216.34/hooks/T0K3N"
    );
    // no webhook from here on: the runs below must not call a real address
    let (status, _) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "notify": { "emails": ["ops@example.com"], "webhook": "" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(core.settings().cron_jobs[0].notify.webhook, "");
    // queues a run and waits until it and the notices it queued are through
    let (client, conn) = tokio_postgres::connect(&tdb.url, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(conn);
    let client = &client;
    let run = |def: slonbase::cron_jobs::JobDef, trigger: &'static str| {
        let core = core.clone();
        async move {
            slonbase::cron_jobs::enqueue(&core, &def, trigger, "", None)
                .await
                .unwrap();
            let settled = eventually(async || {
                let pending = "SELECT count(*) FROM _slonbase.jobs WHERE state IN ('queued', 'running')";
                client.query_one(pending, &[]).await.unwrap().get::<_, i64>(0) == 0
            })
            .await;
            assert!(settled, "the queue didn't settle");
        }
    };
    let mails = || mailer.sent.lock().unwrap().clone();

    // the first failure tells; the next ones, and a run started by hand, stay quiet
    run(core.settings().cron_jobs[0].clone(), "schedule").await;
    assert_eq!(mails().len(), 1, "{:?}", mails());
    assert_eq!(mails()[0].subject, "Cron job failed: Nightly check");
    assert!(
        mails()[0].html.contains("division by zero"),
        "{}",
        mails()[0].html
    );
    run(core.settings().cron_jobs[0].clone(), "schedule").await;
    run(core.settings().cron_jobs[0].clone(), "manual").await;
    assert_eq!(
        mails().len(),
        1,
        "a job that keeps failing sends one message: {:?}",
        mails()
    );

    // it works again: one message, then quiet
    let (status, _) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "config": { "statement": "SELECT 1" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    run(core.settings().cron_jobs[0].clone(), "schedule").await;
    assert_eq!(mails().len(), 2);
    assert_eq!(mails()[1].subject, "Cron job recovered: Nightly check");
    run(core.settings().cron_jobs[0].clone(), "schedule").await;
    assert_eq!(mails().len(), 2);

    // and a new streak tells again
    let (_, _) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "config": { "statement": "SELECT 1/0" } })),
    )
    .await;
    run(core.settings().cron_jobs[0].clone(), "schedule").await;
    assert_eq!(mails().len(), 3);
    assert!(
        mails().iter().all(|m| m.to == "ops@example.com"),
        "{:?}",
        mails()
    );
}

#[tokio::test]
async fn jobs_are_switched_off_and_copied_from_the_list() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "events", "fields": [{ "name": "title", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");

    // a copy keeps the secrets of its original (the form only has them masked)
    let (status, original) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(url_job("Ping", "http://93.184.216.34/hook")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{original}");
    let from = original["id"].as_str().unwrap().to_string();
    let copy = |name: &str, url: &str, header: &str| {
        let mut body = url_job(name, url);
        body["config"]["headers"] = json!({ "Authorization": header });
        body["copyOf"] = json!(from);
        body
    };
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(copy("Ping (copy)", "http://93.184.216.34/hook", "••••••••")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_ne!(res["id"], original["id"]);
    let core = app.core().unwrap();
    let jobs = core.settings().cron_jobs.clone();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[1].config["headers"]["Authorization"], "Bearer s3cret");
    assert!(!res.to_string().contains("s3cret"), "{res}");
    // the preview of the form does the same
    let (status, _) = call(
        &app,
        &token,
        "POST",
        "/api/cron-preview",
        Some(copy("Ping (3)", "http://93.184.216.34/hook", "••••••••")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // ...but the secret doesn't follow the copy to another address, and an unknown original
    // is not found
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(copy("Elsewhere", "http://93.184.216.35/hook", "••••••••")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"]["config"]["headers"].is_object(), "{res}");
    let (status, _) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(copy(
            "Elsewhere",
            "http://93.184.216.35/hook",
            "Bearer mine",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut lost = copy("Lost", "http://93.184.216.34/hook", "x");
    lost["copyOf"] = json!("user:nope");
    let (status, _) = call(&app, &token, "POST", "/api/crons", Some(lost)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // switching off needs nothing else to be right; switching on checks the job again
    let (status, cleanup_job) = call(
        &app,
        &token,
        "POST",
        "/api/crons",
        Some(cleanup("Old events", "title ~ 'old'")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleanup_job}");
    let id = cleanup_job["id"].as_str().unwrap().to_string();
    let (status, res) = call(&app, &token, "GET", "/api/collections", None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let events = res["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "events")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, res) = call(
        &app,
        &token,
        "DELETE",
        &format!("/api/collections/{events}"),
        None,
    )
    .await;
    assert!(status.is_success(), "{status} {res}");
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["enabled"], false);
    let (status, _) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/crons/{id}"),
        Some(json!({ "enabled": true })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "its collection is gone");
}

#[tokio::test]
async fn paused_jobs_dont_run_on_their_schedule_but_can_be_run_by_hand() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let core = app.core().unwrap();
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "events", "fields": [{ "name": "title", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    for title in ["old a", "old b"] {
        add(&app, &token, title).await;
    }
    let mut body = cleanup("Every minute", "title ~ 'old'");
    body["expression"] = "* * * * *".into();
    let (status, job) = call(&app, &token, "POST", "/api/crons", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{job}");
    let id = job["id"].as_str().unwrap().to_string();

    // paused (a setting, so it is shared by every instance)
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "schedule": { "pauseJobs": true } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["schedule"]["pauseJobs"], true, "{res}");
    assert_eq!(
        res["schedule"]["timezone"], "UTC",
        "the time zone is untouched"
    );
    slonbase::hooks::run_scheduled_crons(&core).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(records(&app, &token).await, 2, "nothing ran");
    let (_, runs) = call(&app, &token, "GET", "/api/jobs/runs?kind=cron", None).await;
    assert_eq!(runs["total"], 0, "{runs}");

    // "Run now" still works while paused
    let (status, _) = call(&app, &token, "POST", &format!("/api/crons/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        eventually(async || records(&app, &token).await == 0).await,
        "run by hand"
    );
    for title in ["old c"] {
        add(&app, &token, title).await;
    }

    // resumed within the same minute: what is due runs, because pausing claimed nothing
    let (status, _) = call(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "schedule": { "pauseJobs": false } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    slonbase::hooks::run_scheduled_crons(&core).await;
    assert!(
        eventually(async || records(&app, &token).await == 0).await,
        "it runs again once resumed"
    );

    // the time zone setting alone doesn't turn the pause off, and the default is not paused
    let (_, settings) = call(&app, &token, "GET", "/api/settings", None).await;
    assert_eq!(settings["schedule"]["pauseJobs"], false, "{settings}");
}

async fn key_for(app: &Arc<App>, token: &str, role: &str) -> String {
    let (status, key) = call(
        app,
        token,
        "POST",
        "/api/team/keys",
        Some(json!({ "name": role, "role": role })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{key}");
    key["key"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn what_each_role_can_read_and_run() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "smtp": { "enabled": true, "host": "mail.acme.test", "port": 2525 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let mut ids = Vec::new();
    for body in [
        json!({ "name": "Query", "expression": "0 3 * * *", "type": "sql",
                "config": { "statement": "SELECT 1 AS secret_marker" } }),
        json!({ "name": "Mail", "expression": "0 8 * * *", "type": "mail",
                "config": { "to": "ceo@example.com", "subject": "Quarterly numbers", "body": "internal figures" },
                "notify": { "emails": ["oncall@example.com"] } }),
    ] {
        let (status, res) = call(&app, &token, "POST", "/api/crons", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{res}");
        ids.push(res["id"].as_str().unwrap().to_string());
    }
    let (viewer, developer, admin) = (
        key_for(&app, &token, "viewer").await,
        key_for(&app, &token, "developer").await,
        key_for(&app, &token, "admin").await,
    );

    // below admin: that a job exists, when it runs and how it went, not what it does
    for (role, key) in [("viewer", &viewer), ("developer", &developer)] {
        let (status, list) = call(&app, key, "GET", "/api/crons", None).await;
        assert_eq!(status, StatusCode::OK);
        let text = list.to_string();
        for secret in [
            "secret_marker",
            "ceo@example.com",
            "Quarterly numbers",
            "internal figures",
            "oncall@example.com",
        ] {
            assert!(!text.contains(secret), "{role} sees {secret}: {text}");
        }
        assert_eq!(
            list.as_array().unwrap().len(),
            2,
            "{role} still sees the jobs"
        );
        assert!(list[0]["name"].is_string());
    }
    // an admin edits jobs, so sees their settings, except an SQL statement (owners'): the
    // editor can't show what it can't edit
    let (_, list) = call(&app, &admin, "GET", "/api/crons", None).await;
    let text = list.to_string();
    assert!(
        text.contains("internal figures"),
        "admins see a mail job: {text}"
    );
    assert!(
        !text.contains("secret_marker"),
        "admins don't see SQL: {text}"
    );
    let (_, list) = call(&app, &token, "GET", "/api/crons", None).await;
    assert!(
        list.to_string().contains("secret_marker"),
        "owners see it all"
    );

    // only an owner runs a saved SQL statement by hand
    for (role, key) in [("developer", &developer), ("admin", &admin)] {
        let (status, res) = call(&app, key, "POST", &format!("/api/crons/{}", ids[0]), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{role}: {res}");
    }
    let (status, _) = call(
        &app,
        &developer,
        "POST",
        &format!("/api/crons/{}", ids[1]),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "developers still run the others"
    );
    let (status, _) = call(
        &app,
        &token,
        "POST",
        &format!("/api/crons/{}", ids[0]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn jobs_edited_at_the_same_time_are_all_kept() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "events", "fields": [{ "name": "title", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let makes = (0..12).map(|i| {
        let app = app.clone();
        let token = token.clone();
        async move {
            call(
                &app,
                &token,
                "POST",
                "/api/crons",
                Some(cleanup(&format!("Job {i}"), "title ~ 'x'")),
            )
            .await
        }
    });
    for (status, res) in futures_util::future::join_all(makes).await {
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    let (_, list) = call(&app, &token, "GET", "/api/crons", None).await;
    assert_eq!(
        list.as_array().unwrap().len(),
        12,
        "no job was lost: {list}"
    );
}

#[tokio::test]
async fn settings_saved_at_the_same_time_are_all_kept() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/collections",
        Some(json!({ "name": "events", "fields": [{ "name": "title", "type": "text" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    // jobs, API keys and a settings change, all at once: each is applied on top of the others
    let mut calls = Vec::new();
    for i in 0..4 {
        let (a1, t1) = (app.clone(), token.clone());
        calls.push(tokio::spawn(async move {
            call(
                &a1,
                &t1,
                "POST",
                "/api/crons",
                Some(cleanup(&format!("Job {i}"), "title ~ 'x'")),
            )
            .await
        }));
        let (a2, t2) = (app.clone(), token.clone());
        calls.push(tokio::spawn(async move {
            call(
                &a2,
                &t2,
                "POST",
                "/api/team/keys",
                Some(json!({ "name": format!("key {i}"), "role": "viewer" })),
            )
            .await
        }));
    }
    let (app2, token2) = (app.clone(), token.clone());
    calls.push(tokio::spawn(async move {
        call(
            &app2,
            &token2,
            "PATCH",
            "/api/settings",
            Some(json!({ "meta": { "appName": "Shop" } })),
        )
        .await
    }));
    for handle in calls {
        let (status, res) = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    let (_, jobs) = call(&app, &token, "GET", "/api/crons", None).await;
    assert_eq!(jobs.as_array().unwrap().len(), 4, "{jobs}");
    let (_, settings) = call(&app, &token, "GET", "/api/settings", None).await;
    assert_eq!(settings["meta"]["appName"], "Shop");
    // and what every instance would load from the database is the same
    let core = app.core().unwrap();
    let client = core.pool.get().await.unwrap();
    let stored = slonbase::settings::Settings::load(&client, None)
        .await
        .unwrap();
    assert_eq!((stored.cron_jobs.len(), stored.api_keys.len()), (4, 4));
    assert_eq!(stored.meta.app_name, "Shop");
}

#[tokio::test]
async fn history_never_shows_the_content_of_a_called_urls_answer() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let core = app.core().unwrap();
    // a run recorded by an earlier version, with the start of the answer in its result
    core.pool
        .get()
        .await
        .unwrap()
        .execute(
            "INSERT INTO _slonbase.job_runs (kind, job, trigger, status, finished, meta)
             VALUES ('cron', 'user:old', 'schedule', 'ok', now(),
                     '{\"status\":200,\"response\":\"{\\\"token\\\":\\\"leaked\\\"}\"}'::jsonb)",
            &[],
        )
        .await
        .unwrap();
    for key in [token.clone(), key_for(&app, &token, "viewer").await] {
        let (status, runs) = call(&app, &key, "GET", "/api/jobs/runs?kind=cron", None).await;
        assert_eq!(status, StatusCode::OK, "{runs}");
        assert_eq!(runs["items"][0]["meta"]["status"], 200, "{runs}");
        assert!(!runs.to_string().contains("leaked"), "{runs}");
    }
}

#[tokio::test]
async fn names_and_mail_schedules_are_safe() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        Some(json!({ "smtp": { "enabled": true, "host": "mail.acme.test", "port": 2525 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    // a name ends up in an email's subject: no line breaks or other control characters
    for name in ["Nightly\nBcc: evil@example.com", "Tab\there", "Bell\u{7}"] {
        let mut body = url_job(name, "http://93.184.216.34/");
        body["notify"] = json!({ "emails": ["ops@example.com"] });
        let (status, res) = call(&app, &token, "POST", "/api/crons", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name:?}: {res}");
        assert!(res["data"]["name"].is_object(), "{name:?}: {res}");
    }
    // an email job can't run more than every 5 minutes (a typo mustn't mail people every minute)
    let mail = |expression: &str| {
        json!({ "name": "Mail", "expression": expression, "type": "mail",
                "config": { "to": "ops@example.com", "subject": "Hi", "body": "x" } })
    };
    for expression in ["* * * * *", "*/2 * * * *", "0,3 * * * *", "0-10 * * * *"] {
        let (status, res) = call(&app, &token, "POST", "/api/crons", Some(mail(expression))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{expression}: {res}");
        assert!(res["data"]["expression"].is_object(), "{expression}: {res}");
    }
    for expression in ["*/5 * * * *", "0 * * * *", "0 8 * * 1", "7,40 * * * *"] {
        let (status, res) = call(&app, &token, "POST", "/api/crons", Some(mail(expression))).await;
        assert_eq!(status, StatusCode::OK, "{expression}: {res}");
        let id = res["id"].as_str().unwrap().to_string();
        let (status, _) = call(&app, &token, "DELETE", &format!("/api/crons/{id}"), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
}
