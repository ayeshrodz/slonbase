//! Automations (jobs of several steps that pass data on) and the email templates they send.
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

async fn ok(app: &Arc<App>, token: &str, method: &str, uri: &str, body: Value) -> Value {
    let (status, res) = call(app, token, method, uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{method} {uri}: {res}");
    res
}

async fn eventually(mut check: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..300 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Runs an automation now and returns its run once it has ended. `seen` is how many runs it
/// had before.
async fn run(app: &Arc<App>, token: &str, id: &str, seen: usize) -> Value {
    let (status, res) = call(app, token, "POST", &format!("/api/automations/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let query = format!("/api/jobs/runs?kind=cron&job={id}");
    let mut last = Value::Null;
    assert!(
        eventually(async || {
            let (_, res) = call(app, token, "GET", &query, None).await;
            last = res["items"][0].clone();
            res["total"].as_u64() == Some(seen as u64 + 1)
                && last["status"].is_string()
                && last["status"] != "running"
        })
        .await,
        "the run ended: {last}"
    );
    last
}

async fn key_for(app: &Arc<App>, token: &str, role: &str) -> String {
    let key = ok(
        app,
        token,
        "POST",
        "/api/team/keys",
        json!({ "name": role, "role": role }),
    )
    .await;
    key["key"].as_str().unwrap().to_string()
}

async fn mail_on(app: &Arc<App>, token: &str) -> Arc<slonbase::mail::MemoryMailer> {
    let mailer = Arc::new(slonbase::mail::MemoryMailer::default());
    app.core().unwrap().set_mailer(mailer.clone());
    ok(
        app,
        token,
        "PATCH",
        "/api/settings",
        json!({ "smtp": { "enabled": true, "host": "mail.acme.test", "port": 2525 } }),
    )
    .await;
    mailer
}

/// An `invoices` collection with three invoices, two of them overdue.
async fn with_invoices(app: &Arc<App>, token: &str) {
    ok(
        app,
        token,
        "POST",
        "/api/collections",
        json!({ "name": "invoices", "fields": [
            { "name": "customer", "type": "text" },
            { "name": "email", "type": "email" },
            { "name": "amount", "type": "number" },
            { "name": "overdue", "type": "bool" },
            { "name": "reminded", "type": "bool" },
        ] }),
    )
    .await;
    for (customer, email, amount, overdue) in [
        ("Ann <Ltd>", "ann@example.com", 120.5, true),
        ("Bob", "bob@example.com", 80.0, true),
        ("Cy", "cy@example.com", 10.0, false),
    ] {
        ok(
            app,
            token,
            "POST",
            "/api/collections/invoices/records",
            json!({ "customer": customer, "email": email, "amount": amount, "overdue": overdue }),
        )
        .await;
    }
}

const NEVER: &str = "0 0 29 2 *";

#[tokio::test]
async fn templates_are_made_previewed_and_kept_while_in_use() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let mailer = mail_on(&app, &token).await;

    // checked when saved
    for (body, field) in [
        (json!({ "name": "", "subject": "s", "body": "b" }), "name"),
        (
            json!({ "name": "x", "subject": "Hi {{ name", "body": "b" }),
            "subject",
        ),
        (
            json!({ "name": "x", "subject": "a\nb", "body": "b" }),
            "subject",
        ),
        (
            json!({ "name": "x", "subject": "s", "body": "{% if a %}" }),
            "body",
        ),
        (
            json!({ "name": "x", "subject": "s", "body": "b", "sample": [1] }),
            "sample",
        ),
    ] {
        let (status, res) = call(&app, &token, "POST", "/api/templates", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
        assert!(res["data"][field].is_object(), "{field}: {res}");
    }
    let template = ok(
        &app,
        &token,
        "POST",
        "/api/templates",
        json!({
            "name": "Reminder", "description": "Sent for overdue invoices",
            "subject": "Invoice overdue, {{ item.customer }}",
            "body": "<p>Dear {{ item.customer }}, {{ item.amount }} is due.</p>",
            "sample": { "item": { "customer": "Sample", "amount": 5 } },
        }),
    )
    .await;
    let id = template["id"].as_str().unwrap().to_string();
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/templates",
        Some(json!({ "name": "reminder", "subject": "s", "body": "b" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a name is used once: {res}"
    );

    // a preview renders what is typed, with the data given: values are escaped
    let preview = ok(
        &app,
        &token,
        "POST",
        "/api/templates/preview",
        json!({
            "subject": "Hi {{ name }}", "body": "<p>{{ name }}</p>",
            "data": { "name": "<i>Ann</i>" },
        }),
    )
    .await;
    assert_eq!(preview["subject"], "Hi <i>Ann</i>");
    let html = preview["html"].as_str().unwrap();
    assert!(
        html.contains("&lt;i&gt;Ann") && !html.contains("<i>Ann"),
        "{html}"
    );
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/templates/preview",
        Some(json!({ "subject": "s", "body": "{{ a +" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");

    // a body that would build a huge text outside the output is refused at once, even behind
    // a comment that holds a quote
    let started = std::time::Instant::now();
    let big = "x".repeat(90_000);
    let body = format!(
        "{{# ' #}}{{% set kept %}}{{% for i in range(10000) %}}{big}{{% endfor %}}{{% endset %}}"
    );
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/templates/preview",
        Some(json!({ "subject": "s", "body": body })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(started.elapsed() < Duration::from_secs(5));

    // changed, listed, and sent as a test with its sample
    let changed = ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/templates/{id}"),
        json!({ "description": "For overdue invoices" }),
    )
    .await;
    assert_eq!(changed["description"], "For overdue invoices");
    assert_eq!(changed["subject"], "Invoice overdue, {{ item.customer }}");
    let (_, list) = call(&app, &token, "GET", "/api/templates", None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let (status, res) = call(
        &app,
        &token,
        "POST",
        &format!("/api/templates/{id}/test"),
        Some(json!({ "email": "me@example.com" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let sent = mailer.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, "me@example.com");
    assert_eq!(sent[0].subject, "Invoice overdue, Sample");

    // who may do what: everyone reads, developers preview, admins change
    let viewer = key_for(&app, &token, "viewer").await;
    let developer = key_for(&app, &token, "developer").await;
    let (status, _) = call(&app, &viewer, "GET", "/api/templates", None).await;
    assert_eq!(status, StatusCode::OK);
    let preview = json!({ "subject": "s", "body": "b" });
    let (status, _) = call(
        &app,
        &viewer,
        "POST",
        "/api/templates/preview",
        Some(preview.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &app,
        &developer,
        "POST",
        "/api/templates/preview",
        Some(preview),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for key in [&viewer, &developer] {
        let body = json!({ "name": "n", "subject": "s", "body": "b" });
        let (status, _) = call(&app, key, "POST", "/api/templates", Some(body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = call(&app, key, "DELETE", &format!("/api/templates/{id}"), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    // a template an automation sends stays
    with_invoices(&app, &token).await;
    let automation = ok(
        &app,
        &token,
        "POST",
        "/api/automations",
        json!({ "name": "Reminders", "expression": NEVER, "type": "steps", "config": { "steps": [
            { "name": "send", "type": "email",
              "config": { "template": id, "to": "ops@example.com" } },
        ] } }),
    )
    .await;
    let (status, res) = call(
        &app,
        &token,
        "DELETE",
        &format!("/api/templates/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(
        res["message"].as_str().unwrap().contains("Reminders"),
        "{res}"
    );
    let automation_id = automation["id"].as_str().unwrap();
    let (status, _) = call(
        &app,
        &token,
        "DELETE",
        &format!("/api/automations/{automation_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &app,
        &token,
        "DELETE",
        &format!("/api/templates/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn steps_pass_data_on_from_a_query_to_emails_and_writes() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let mailer = mail_on(&app, &token).await;
    with_invoices(&app, &token).await;
    let template = ok(
        &app,
        &token,
        "POST",
        "/api/templates",
        json!({
            "name": "Reminder",
            "subject": "Invoice overdue, {{ item.customer }}",
            "body": "<p>Dear {{ item.customer }}, {{ item.amount }} is due ({{ total }} open).</p>",
        }),
    )
    .await;
    let automation = ok(
        &app,
        &token,
        "POST",
        "/api/automations",
        json!({ "name": "Overdue reminders", "expression": NEVER, "type": "steps", "config": { "steps": [
            { "name": "overdue", "type": "query", "config": {
                "collection": "invoices", "filter": "overdue = {:yes} && reminded = false",
                "params": { "yes": true }, "sort": "customer" } },
            { "name": "any", "type": "condition",
              "config": { "expression": "steps.overdue.count > 0" } },
            { "name": "remind", "type": "email", "forEach": "steps.overdue.rows", "config": {
                "template": template["id"], "to": "{{ item.email }}",
                "data": { "total": "{{ steps.overdue.count }}" } } },
            { "name": "mark", "type": "write", "forEach": "steps.overdue.rows", "config": {
                "action": "update", "collection": "invoices", "id": "{{ item.id }}",
                "data": { "reminded": true } } },
        ] } }),
    )
    .await;
    let id = automation["id"].as_str().unwrap().to_string();
    assert_eq!(automation["type"], "steps");
    assert_eq!(automation["config"]["steps"][0]["config"]["limit"], 100);

    let first = run(&app, &token, &id, 0).await;
    assert_eq!(first["status"], "ok", "{first}");
    // each customer got their own email, their data escaped in the body
    let sent = mailer.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0].to, "ann@example.com");
    assert_eq!(sent[0].subject, "Invoice overdue, Ann <Ltd>");
    assert!(
        sent[0]
            .html
            .contains("Dear Ann &lt;Ltd&gt;, 120.5 is due (2 open)."),
        "{}",
        sent[0].html
    );
    assert_eq!(sent[1].to, "bob@example.com");
    // and their invoices are marked
    let (_, marked) = call(
        &app,
        &token,
        "GET",
        "/api/collections/invoices/records?filter=reminded%3Dtrue",
        None,
    )
    .await;
    assert_eq!(marked["totalItems"], 2, "{marked}");
    // the history says what happened in numbers, with nothing of what was read
    assert_eq!(
        first["meta"]["steps"],
        json!([
            { "name": "overdue", "type": "query", "count": 2 },
            { "name": "any", "type": "condition", "passed": true },
            { "name": "remind", "type": "email", "count": 2 },
            { "name": "mark", "type": "write", "count": 2 },
        ])
    );
    assert!(!first.to_string().contains("example.com"), "{first}");

    // nothing overdue is left: the condition ends the next run before anything is sent
    let second = run(&app, &token, &id, 1).await;
    assert_eq!(second["status"], "ok", "{second}");
    assert_eq!(second["meta"]["stoppedAt"], "any");
    assert_eq!(second["meta"]["steps"].as_array().unwrap().len(), 2);
    assert_eq!(mailer.sent.lock().unwrap().len(), 2);

    // what a step did wrong names the step
    ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        json!({ "config": { "steps": [
            { "name": "gone", "type": "write", "config": {
                "action": "update", "collection": "invoices", "id": "nope", "data": { "reminded": true } } },
        ] } }),
    )
    .await;
    let third = run(&app, &token, &id, 2).await;
    assert_eq!(third["status"], "error", "{third}");
    assert!(
        third["detail"]
            .as_str()
            .unwrap()
            .starts_with("Step gone: No record"),
        "{third}"
    );
}

#[tokio::test]
async fn a_retry_continues_at_the_step_that_failed() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let mailer = mail_on(&app, &token).await;
    ok(
        &app,
        &token,
        "POST",
        "/api/collections",
        json!({ "name": "gate", "fields": [{ "name": "title", "type": "text" }] }),
    )
    .await;
    // an email, then a statement that fails until the gate has a row
    let automation = ok(
        &app,
        &token,
        "POST",
        "/api/automations",
        json!({
            "name": "Two steps", "expression": NEVER, "type": "steps",
            "retries": 3, "retryDelaySeconds": 1,
            "config": { "steps": [
                { "name": "tell", "type": "mail", "forEach": "['a@example.com', 'b@example.com']",
                  "config": { "to": "{{ item }}", "subject": "Started", "body": "Run of {{ automation.name }}" } },
                { "name": "gate", "type": "sql",
                  "config": { "statement": "SELECT 1/count(*) AS n FROM slonbase.gate" } },
                { "name": "done", "type": "mail",
                  "config": { "to": "ops@example.com", "subject": "Gate has {{ steps.gate.rows[0].n }}", "body": "ok" } },
            ] },
        }),
    )
    .await;
    let id = automation["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &app,
        &token,
        "POST",
        &format!("/api/automations/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // the first attempt sent the two emails and failed at the statement
    assert!(
        eventually(async || mailer.sent.lock().unwrap().len() == 2).await,
        "{:?}",
        mailer.sent.lock().unwrap()
    );
    let query = format!("/api/jobs/runs?kind=cron&job={id}");
    assert!(
        eventually(async || {
            let (_, res) = call(&app, &token, "GET", &query, None).await;
            res["items"][0]["detail"]
                .as_str()
                .is_some_and(|d| d.contains("Attempt 1 failed"))
        })
        .await
    );
    ok(
        &app,
        &token,
        "POST",
        "/api/collections/gate/records",
        json!({ "title": "open" }),
    )
    .await;
    let mut last = Value::Null;
    assert!(
        eventually(async || {
            let (_, res) = call(&app, &token, "GET", &query, None).await;
            last = res["items"][0].clone();
            last["status"] == "ok"
        })
        .await,
        "{last}"
    );
    // the retry went on from the statement: the first emails were not sent again, and the last
    // step had the statement's rows
    let sent = mailer.sent.lock().unwrap().clone();
    let subjects: Vec<&str> = sent.iter().map(|m| m.subject.as_str()).collect();
    assert_eq!(subjects, ["Started", "Started", "Gate has 1"], "{sent:?}");
    assert!(last["meta"]["attempts"].as_u64().unwrap() >= 2, "{last}");
    assert_eq!(last["meta"]["steps"].as_array().unwrap().len(), 3, "{last}");
    // one run in the history, and nothing kept of it once it ended
    let (_, res) = call(&app, &token, "GET", &query, None).await;
    assert_eq!(res["total"], 1, "{res}");
    let kept: Option<Value> = tdb
        .pool()
        .get()
        .await
        .unwrap()
        .query_one(
            "SELECT state FROM _slonbase.job_runs ORDER BY id DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(kept, None);
}

#[tokio::test]
async fn steps_are_checked_and_follow_the_rules_of_what_they_do() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    mail_on(&app, &token).await;
    with_invoices(&app, &token).await;
    let automation = |steps: Value| json!({ "name": "Checked", "expression": NEVER, "type": "steps", "config": { "steps": steps } });
    let query = |name: &str| json!({ "name": name, "type": "query", "config": { "collection": "invoices" } });

    for (steps, field) in [
        (json!([]), "steps"),
        (
            json!([{ "name": "Bad Name", "type": "query", "config": { "collection": "invoices" } }]),
            "steps.0.name",
        ),
        (json!([query("a"), query("a")]), "steps.1.name"),
        (
            json!([{ "name": "a", "type": "shell", "config": {} }]),
            "steps.0.type",
        ),
        (
            json!([{ "name": "a", "type": "query", "forEach": "x", "config": { "collection": "invoices" } }]),
            "steps.0.forEach",
        ),
        (
            json!([{ "name": "a", "type": "query", "config": { "collection": "_superusers" } }]),
            "steps.0.config.collection",
        ),
        (
            json!([{ "name": "a", "type": "query", "config": { "collection": "invoices", "filter": "nope = 1" } }]),
            "steps.0.config.filter",
        ),
        // data never becomes part of a filter or a statement
        (
            json!([{ "name": "a", "type": "query", "config": { "collection": "invoices", "filter": "customer = '{{ x }}'" } }]),
            "steps.0.config.filter",
        ),
        (
            json!([{ "name": "a", "type": "sql", "config": { "statement": "SELECT '{{ x }}'" } }]),
            "steps.0.config.statement",
        ),
        (
            json!([{ "name": "a", "type": "condition", "config": { "expression": "1 +" } }]),
            "steps.0.config.expression",
        ),
        (
            json!([{ "name": "a", "type": "email", "config": { "template": "nope", "to": "a@b.co" } }]),
            "steps.0.config.template",
        ),
        (
            json!([{ "name": "a", "type": "write", "config": { "action": "drop", "collection": "invoices" } }]),
            "steps.0.config.action",
        ),
        (
            json!([{ "name": "a", "type": "write", "config": { "action": "update", "collection": "invoices", "data": {} } }]),
            "steps.0.config.id",
        ),
        (
            json!([{ "name": "a", "type": "http", "config": { "url": "http://127.0.0.1/x" } }]),
            "steps.0.config.url",
        ),
        // data may fill in a path, never choose the server
        (
            json!([{ "name": "a", "type": "http", "config": { "url": "https://{{ item.host }}/x" } }]),
            "steps.0.config.url",
        ),
        (
            json!([{ "name": "a", "type": "http", "config": { "url": "{{ item.url }}" } }]),
            "steps.0.config.url",
        ),
        // a credential is only sent to an https:// address, also when the path is built
        (
            json!([{ "name": "a", "type": "http", "config": {
                "url": "http://93.184.216.34/orders/{{ item.id }}", "headers": { "Authorization": "Bearer x" } } }]),
            "steps.0.config.headers",
        ),
        (
            json!([{ "name": "a", "type": "mail", "config": { "to": "{{ item.email", "subject": "s", "body": "b" } }]),
            "steps.0.config",
        ),
    ] {
        let (status, res) = call(
            &app,
            &token,
            "POST",
            "/api/automations",
            Some(automation(steps.clone())),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{steps}: {res}");
        let pointer = format!("/data/config/{}", field.replace('.', "/"));
        assert!(res.pointer(&pointer).is_some(), "{field}: {res}");
    }
    let built = automation(json!([
        { "name": "a", "type": "http", "config": { "url": "http://93.184.216.34/orders/{{ steps.x.id }}" } },
    ]));
    let (status, res) = call(&app, &token, "POST", "/api/cron-preview", Some(built)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a path may be built from data: {res}"
    );

    // more than 20 steps
    let many: Vec<Value> = (0..21).map(|n| query(&format!("s{n}"))).collect();
    let (status, _) = call(
        &app,
        &token,
        "POST",
        "/api/automations",
        Some(automation(json!(many))),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // what sends email runs at most every 5 minutes, as a step too
    let mut often = automation(json!([
        { "name": "m", "type": "mail", "config": { "to": "a@example.com", "subject": "s", "body": "b" } },
    ]));
    often["expression"] = json!("* * * * *");
    let (status, res) = call(&app, &token, "POST", "/api/automations", Some(often)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"]["expression"].is_object(), "{res}");

    // a statement among the steps is an owner's to save, to see and to run
    let admin = key_for(&app, &token, "admin").await;
    let with_sql = automation(json!([
        { "name": "q", "type": "sql", "config": { "statement": "SELECT 1 AS one" } },
        { "name": "c", "type": "condition", "config": { "expression": "steps.q.rows[0].one | int == 1" } },
    ]));
    let (status, res) = call(
        &app,
        &admin,
        "POST",
        "/api/automations",
        Some(with_sql.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");
    let made = ok(&app, &token, "POST", "/api/automations", with_sql).await;
    let id = made["id"].as_str().unwrap().to_string();
    let (_, list) = call(&app, &admin, "GET", "/api/automations", None).await;
    let seen = list
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(seen["config"], json!({}), "an admin doesn't see it: {seen}");
    let (status, _) = call(
        &app,
        &admin,
        "POST",
        &format!("/api/automations/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &app,
        &admin,
        "PATCH",
        &format!("/api/automations/{id}"),
        Some(json!({ "name": "Mine" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let ran = run(&app, &token, &id, 0).await;
    assert_eq!(ran["status"], "ok", "{ran}");
    assert_eq!(ran["meta"]["steps"][1]["passed"], true, "{ran}");
}

#[tokio::test]
async fn records_reach_steps_without_their_secrets() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let mailer = mail_on(&app, &token).await;
    ok(
        &app,
        &token,
        "POST",
        "/api/collections",
        json!({ "name": "members", "type": "auth" }),
    )
    .await;
    ok(
        &app,
        &token,
        "POST",
        "/api/collections/members/records",
        json!({ "email": "m@example.com", "password": "correct-horse-1", "passwordConfirm": "correct-horse-1" }),
    )
    .await;
    let automation = ok(
        &app,
        &token,
        "POST",
        "/api/automations",
        json!({ "name": "Dump", "expression": NEVER, "type": "steps", "config": { "steps": [
            { "name": "all", "type": "query", "config": { "collection": "members" } },
            { "name": "tell", "type": "mail", "config": {
                "to": "ops@example.com", "subject": "Members",
                "body": "{{ steps.all.rows | tojson }}" } },
        ] } }),
    )
    .await;
    let ran = run(&app, &token, automation["id"].as_str().unwrap(), 0).await;
    assert_eq!(ran["status"], "ok", "{ran}");
    let sent = mailer.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    let body = &sent[0].html;
    assert!(body.contains("m@example.com"), "{body}");
    assert!(
        !body.contains("tokenKey") && !body.contains("password"),
        "{body}"
    );
}

#[tokio::test]
async fn a_job_of_one_type_becomes_steps_and_keeps_its_secrets() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    // a job as 2.6 made it, through the address 2.6 had
    let job = ok(
        &app,
        &token,
        "POST",
        "/api/crons",
        json!({ "name": "Ping", "expression": NEVER, "type": "http", "config": {
            "url": "https://93.184.216.34/hook", "headers": { "Authorization": "Bearer T0K3N" } } }),
    )
    .await;
    let id = job["id"].as_str().unwrap().to_string();
    // it is in the automations as it is
    let (_, list) = call(&app, &token, "GET", "/api/automations", None).await;
    let listed = list
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(listed["type"], "http");
    assert!(!list.to_string().contains("T0K3N"));

    // saved as steps (what the dashboard does when it is edited): the header's value stays
    let changed = ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        json!({ "type": "steps", "config": { "steps": [
            { "name": "ping", "type": "http", "config": {
                "url": "https://93.184.216.34/hook", "headers": { "Authorization": "••••••••" } } },
            { "name": "ok", "type": "condition", "config": { "expression": "steps.ping.status == 200" } },
        ] } }),
    )
    .await;
    assert_eq!(
        changed["config"]["steps"][0]["config"]["headers"]["Authorization"],
        "••••••••"
    );
    let core = app.core().unwrap();
    let stored = core.settings().cron_jobs[0].config.clone();
    assert_eq!(
        stored["steps"][0]["config"]["headers"]["Authorization"],
        "Bearer T0K3N"
    );

    // renamed in the editor (sent with the name it was saved under): the value stays
    let renamed = ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        json!({ "config": { "steps": [
            { "name": "webhook", "was": "ping", "type": "http", "config": {
                "url": "https://93.184.216.34/hook", "headers": { "Authorization": "••••••••" } } },
        ] } }),
    )
    .await;
    assert_eq!(renamed["config"]["steps"][0]["name"], "webhook");
    assert!(renamed["config"]["steps"][0].get("was").is_none());
    let stored = core.settings().cron_jobs[0].config.clone();
    assert_eq!(
        stored["steps"][0]["config"]["headers"]["Authorization"],
        "Bearer T0K3N"
    );
    assert!(stored["steps"][0].get("was").is_none());
    // a hidden value with nothing saved behind it is refused, never saved empty
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        Some(json!({ "config": { "steps": [
            { "name": "other", "type": "http", "config": {
                "url": "https://93.184.216.34/hook", "headers": { "Authorization": "••••••••" } } },
        ] } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert_eq!(
        core.settings().cron_jobs[0].config["steps"][0]["config"]["headers"]["Authorization"],
        "Bearer T0K3N"
    );

    // the same step, a header that was never saved, sent hidden: refused too
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        Some(json!({ "config": { "steps": [
            { "name": "webhook", "type": "http", "config": {
                "url": "https://93.184.216.34/hook",
                "headers": { "Authorization": "••••••••", "X-New": "••••••••" } } },
        ] } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");

    // a saved value never follows a step to another address
    let (status, res) = call(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        Some(json!({ "config": { "steps": [
            { "name": "webhook", "type": "http", "config": {
                "url": "https://93.184.216.35/other", "headers": { "Authorization": "••••••••" } } },
        ] } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(!res.to_string().contains("T0K3N"));
}

#[tokio::test]
async fn an_automation_changed_while_a_retry_waits_is_not_continued() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) =
        ready_app_configured(&tdb, Default::default(), |o| o.sql_console = Some(true)).await;
    let mailer = mail_on(&app, &token).await;
    let steps = |subject: &str| {
        json!({ "steps": [
            { "name": "tell", "type": "mail",
              "config": { "to": "a@example.com", "subject": subject, "body": "x" } },
            { "name": "fails", "type": "sql", "config": { "statement": "SELECT 1/0" } },
        ] })
    };
    let automation = ok(
        &app,
        &token,
        "POST",
        "/api/automations",
        json!({
            "name": "Changing", "expression": NEVER, "type": "steps",
            "retries": 2, "retryDelaySeconds": 2, "config": steps("First"),
        }),
    )
    .await;
    let id = automation["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &app,
        &token,
        "POST",
        &format!("/api/automations/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let query = format!("/api/jobs/runs?kind=cron&job={id}");
    assert!(
        eventually(async || {
            let (_, res) = call(&app, &token, "GET", &query, None).await;
            res["items"][0]["detail"]
                .as_str()
                .is_some_and(|d| d.contains("Attempt 1 failed"))
        })
        .await
    );
    // changed before the retry: its steps are no longer the ones the run was in the middle of
    ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        json!({ "config": steps("Second") }),
    )
    .await;
    let mut last = Value::Null;
    assert!(
        eventually(async || {
            let (_, res) = call(&app, &token, "GET", &query, None).await;
            last = res["items"][0].clone();
            last["status"] == "error"
        })
        .await,
        "{last}"
    );
    assert!(
        last["detail"]
            .as_str()
            .unwrap()
            .contains("was changed while"),
        "{last}"
    );
    // and nothing of the new steps ran in its place
    let sent = mailer.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].subject, "First");
}
