//! What hook files add beyond handlers: functions automations run as steps, settings pages,
//! templates sent from code, and the limits a handler runs within. See `setup_flow.rs` for
//! how to point the tests at a database.
#![cfg(feature = "js")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;
use slonbase::config::HooksConfig;

const HOOKS: &str = r#"
automationStep("double", (input, context) => {
  return { twice: input.n * 2, by: context.automation.name, item: context.item };
});
automationStep("note", (input) => {
  const record = new Record($app.findCollectionByNameOrId("notes"));
  record.set("note", String(input.note));
  $app.save(record);
  return record.id;
});
automationStep("fails", () => { throw new BadRequestError("not today"); });

settingsAdd("billing", {
  title: "Billing",
  fields: [
    { name: "currency", label: "Currency", type: "select", options: ["EUR", "USD"], default: "EUR" },
    { name: "limit", label: "Limit", type: "number", default: 10 },
    { name: "live", type: "bool" },
    { name: "apiKey", label: "API key", type: "secret", required: true, default: "sk_default" },
  ],
});

routerAdd("GET", "/api/js/billing", (e) => e.json(200, $app.settings().ext("billing")));
routerAdd("POST", "/api/js/welcome", (e) => {
  const body = e.requestInfo().body;
  e.json(200, { sent: $templates.send(body.template, body.to, { name: body.name }) });
});
routerAdd("GET", "/api/js/spin", (e) => { for (;;) {} });
routerAdd("GET", "/api/js/wait", (e) => { sleep(1500); e.json(200, { ok: true }); });
routerAdd("GET", "/api/js/grow", (e) => {
  const kept = [];
  for (;;) kept.push("x".repeat(1024 * 1024) + kept.length);
});
routerAdd("GET", "/api/js/ok", (e) => e.json(200, { ok: true }));
// (a handler that waits in next() for longer than its limit is not stopped for it)
routerUse((e) => {
  if (e.request.url.path === "/api/js/wait") sleep(700);
  e.next();
  // (and one that goes on running after the wait still has its limit)
  if (e.request.url.path === "/api/js/late") for (;;) {}
});
routerAdd("GET", "/api/js/late", (e) => { sleep(1200); e.json(200, { ok: true }); });
"#;

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
    for _ in 0..200 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn app_with_hooks(tdb: &TestDb) -> (Arc<App>, String, tempfile::TempDir, tempfile::TempDir) {
    let hooks_dir = tempfile::tempdir().unwrap();
    std::fs::write(hooks_dir.path().join("main.pb.js"), HOOKS).unwrap();
    let mut ext = slonbase::Extensions::default();
    slonbase::js::load(
        &HooksConfig {
            dir: hooks_dir.path().to_path_buf(),
            pool_size: 2,
            watch: false,
            timeout_seconds: 1,
            memory_limit_mb: 48,
        },
        &mut ext,
    )
    .await
    .unwrap();
    let (app, token, dir) = ready_app_with(tdb, ext).await;
    (app, token, dir, hooks_dir)
}

/// Runs an automation now and returns its run once it has ended.
async fn run(app: &Arc<App>, token: &str, id: &str, seen: u64) -> Value {
    let (status, res) = call(app, token, "POST", &format!("/api/automations/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{res}");
    let query = format!("/api/jobs/runs?kind=cron&job={id}");
    let mut last = Value::Null;
    assert!(
        eventually(async || {
            let (_, res) = call(app, token, "GET", &query, None).await;
            last = res["items"][0].clone();
            res["total"].as_u64() == Some(seen + 1) && last["status"] != "running"
        })
        .await,
        "the run ended: {last}"
    );
    last
}

const NEVER: &str = "0 0 29 2 *";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_step_runs_a_function_of_a_hook_file() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir, _hooks) = app_with_hooks(&tdb).await;
    ok(
        &app,
        &token,
        "POST",
        "/api/collections",
        json!({ "name": "notes", "fields": [{ "name": "note", "type": "text" }] }),
    )
    .await;
    let (_, loaded) = call(&app, &token, "GET", "/api/extensions", None).await;
    assert_eq!(loaded["stepFunctions"], json!(["double", "fails", "note"]));
    assert_eq!(loaded["settingsPages"][0]["name"], "billing");

    // a function nobody registered is refused when the automation is saved
    let step = |function: &str, input: Value| json!({ "name": "s", "type": "script", "config": { "function": function, "input": input } });
    let automation = |name: &str, steps: Value| json!({ "name": name, "expression": NEVER, "type": "steps", "config": { "steps": steps } });
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/automations",
        Some(automation("x", json!([step("nope", json!({}))]))),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(
        res["data"]["config"]["steps"]["0"]["config"]["function"].is_object(),
        "{res}"
    );

    // its result is the step's, for the steps after it; it writes records like any hook
    let saved = ok(
        &app,
        &token,
        "POST",
        "/api/automations",
        automation(
            "Doubling",
            json!([
                { "name": "calc", "type": "script", "config": { "function": "double", "input": { "n": 21 } } },
                { "name": "keep", "type": "script", "forEach": "[steps.calc.result.twice, steps.calc.result.by]",
                  "config": { "function": "note", "input": { "note": "{{ item }}" } } },
            ]),
        ),
    )
    .await;
    let (_, shown) = call(&app, &token, "GET", "/api/automations", None).await;
    assert!(
        shown.to_string().contains("\"function\":\"double\""),
        "{shown}"
    );
    let id = saved["id"].as_str().unwrap();
    let done = run(&app, &token, id, 0).await;
    assert_eq!(done["status"], "ok", "{done}");
    let (_, notes) = call(
        &app,
        &token,
        "GET",
        "/api/collections/notes/records?sort=note",
        None,
    )
    .await;
    let notes: Vec<&str> = notes["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["note"].as_str().unwrap())
        .collect();
    assert_eq!(notes, ["42", "Doubling"]);

    // an error a function throws fails the run with its message
    let failing = ok(
        &app,
        &token,
        "POST",
        "/api/automations",
        automation("Failing", json!([step("fails", json!({}))])),
    )
    .await;
    let failed = run(&app, &token, failing["id"].as_str().unwrap(), 0).await;
    assert_eq!(failed["status"], "error");
    assert!(
        failed["detail"].as_str().unwrap().contains("not today"),
        "{failed}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hook_file_declares_a_settings_page() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir, _hooks) = app_with_hooks(&tdb).await;
    // the page, with the defaults while nothing is saved
    let (status, pages) = call(&app, &token, "GET", "/api/extensions/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pages[0]["title"], "Billing");
    assert_eq!(pages[0]["fields"].as_array().unwrap().len(), 4);
    assert_eq!(
        pages[0]["values"],
        json!({ "currency": "EUR", "limit": 10, "live": null, "apiKey": "••••••••" })
    );
    // (a secret's default is used, and shown to nobody: neither as a value nor in the form)
    assert!(!pages.to_string().contains("sk_default"), "{pages}");
    let (_, read) = call(&app, &token, "GET", "/api/js/billing", None).await;
    assert_eq!(read["apiKey"], "sk_default");

    // checked against the form
    for (values, field) in [
        (json!({ "currency": "GBP", "apiKey": "k" }), "currency"),
        (json!({ "limit": "many", "apiKey": "k" }), "limit"),
        (json!({ "live": "yes", "apiKey": "k" }), "live"),
        (json!({ "apiKey": "" }), "apiKey"),
        (json!({ "apiKey": "k", "other": 1 }), "other"),
    ] {
        let (status, res) = call(
            &app,
            &token,
            "PATCH",
            "/api/extensions/settings/billing",
            Some(json!({ "values": values })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {res}");
        assert!(res["data"]["values"][field].is_object(), "{field}: {res}");
    }
    let (status, _) = call(
        &app,
        &token,
        "PATCH",
        "/api/extensions/settings/nope",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // saved; the secret is never sent back, and the mask keeps it
    let saved = ok(
        &app,
        &token,
        "PATCH",
        "/api/extensions/settings/billing",
        json!({ "values": { "currency": "USD", "limit": "25", "live": true, "apiKey": "sk_live_1" } }),
    )
    .await;
    assert_eq!(
        saved["values"],
        json!({ "currency": "USD", "limit": 25.0, "live": true, "apiKey": "••••••••" })
    );
    ok(
        &app,
        &token,
        "PATCH",
        "/api/extensions/settings/billing",
        json!({ "values": { "limit": 30, "apiKey": "••••••••" } }),
    )
    .await;
    let (_, settings) = call(&app, &token, "GET", "/api/settings", None).await;
    assert!(!settings.to_string().contains("sk_live_1"), "{settings}");
    assert!(settings.get("extensions").is_none(), "{settings}");
    // the code reads the values as they are
    let (_, read) = call(&app, &token, "GET", "/api/js/billing", None).await;
    assert_eq!(
        read,
        json!({ "currency": "USD", "limit": 30, "live": true, "apiKey": "sk_live_1" })
    );

    // developers see the pages, admins change them
    let developer = ok(
        &app,
        &token,
        "POST",
        "/api/team/keys",
        json!({ "name": "d", "role": "developer" }),
    )
    .await;
    let developer = developer["key"].as_str().unwrap();
    let (status, _) = call(&app, developer, "GET", "/api/extensions/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        developer,
        "PATCH",
        "/api/extensions/settings/billing",
        Some(json!({ "values": { "limit": 1, "apiKey": "••••••••" } })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handler_runs_within_limits() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir, _hooks) = app_with_hooks(&tdb).await;
    // a handler that never ends is stopped at the limit (1 second here) and fails
    let started = std::time::Instant::now();
    let (status, res) = call(&app, &token, "GET", "/api/js/spin", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{res}");
    assert_eq!(
        res["message"],
        "Something went wrong while processing your request."
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    // and one that takes all the memory it may
    // (an error thrown in JavaScript that isn't an ApiError is answered as PocketBase answers
    // it: 400, nothing told)
    let (status, res) = call(&app, &token, "GET", "/api/js/grow", None).await;
    assert!(!status.is_success(), "{res}");
    assert_eq!(
        res["message"],
        "Something went wrong while processing your request."
    );
    // the runtimes were replaced: the next requests are served (more of them than runtimes)
    for _ in 0..4 {
        let (status, res) = call(&app, &token, "GET", "/api/js/ok", None).await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    // what a handler waits for is not its own time: the middleware sleeps 0.7 s and waits in
    // next() for the route, which sleeps 1.5 s (longer than the limit), and the request is
    // answered
    let (status, res) = call(&app, &token, "GET", "/api/js/wait", None).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    // the wait doesn't take the limit away: a handler that spins after next() is stopped
    let started = std::time::Instant::now();
    let (status, res) = call(&app, &token, "GET", "/api/js/late", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{res}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_template_is_sent_from_code_and_through_the_api() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir, _hooks) = app_with_hooks(&tdb).await;
    let mailer = Arc::new(slonbase::mail::MemoryMailer::default());
    app.core().unwrap().set_mailer(mailer.clone());
    ok(
        &app,
        &token,
        "PATCH",
        "/api/settings",
        json!({ "smtp": { "enabled": true, "host": "mail.acme.test", "port": 2525 } }),
    )
    .await;
    let template = ok(
        &app,
        &token,
        "POST",
        "/api/templates",
        json!({ "name": "Welcome", "subject": "Welcome {{ name }}", "body": "<p>Hi {{ name }}</p>", "sample": { "name": "Ann" } }),
    )
    .await;
    let id = template["id"].as_str().unwrap();

    // from a hook file
    let sent = ok(
        &app,
        &token,
        "POST",
        "/api/js/welcome",
        json!({ "template": id, "to": ["a@example.com", "b@example.com"], "name": "Zoe" }),
    )
    .await;
    assert_eq!(sent["sent"], 2);
    assert_eq!(mailer.sent.lock().unwrap().len(), 2);
    assert_eq!(mailer.sent.lock().unwrap()[0].subject, "Welcome Zoe");

    // through the API: checked now, sent in the background
    let send = format!("/api/templates/{id}/send");
    for (body, field) in [
        (json!({ "to": [], "data": { "name": "x" } }), "to"),
        (
            json!({ "to": "not an address", "data": { "name": "x" } }),
            "to",
        ),
        (
            json!({ "to": "c@example.com", "data": { "name": "x".repeat(70_000) } }),
            "data",
        ),
    ] {
        let (status, res) = call(&app, &token, "POST", &send, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {res}");
        assert!(res["data"][field].is_object(), "{field}: {res}");
    }
    let (status, _) = call(
        &app,
        &token,
        "POST",
        "/api/templates/nope/send",
        Some(json!({ "to": "c@example.com" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, queued) = call(
        &app,
        &token,
        "POST",
        &send,
        Some(json!({ "to": ["c@example.com", "d@example.com"], "data": { "name": "Cy" } })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    assert_eq!(queued["queued"], 2);
    assert!(
        eventually(async || mailer.sent.lock().unwrap().len() == 4).await,
        "{}",
        mailer.sent.lock().unwrap().len()
    );
    assert!(
        mailer
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.to == "d@example.com" && m.subject == "Welcome Cy")
    );

    // the same key queues nothing the second time
    let key = [
        ("authorization", format!("Bearer {token}")),
        ("idempotency-key", "order-1".to_string()),
    ];
    let headers: Vec<(&str, &str)> = key.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let body = json!({ "to": "e@example.com", "data": { "name": "Eve" } });
    let (status, first) = request(&app, "POST", &send, &headers, Some(body.clone())).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(first["queued"], 1);
    let (status, again) = request(&app, "POST", &send, &headers, Some(body)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(again["queued"], 0);
    assert_eq!(again["duplicate"], true);
    assert_eq!(again["id"], first["id"]);
    assert!(eventually(async || mailer.sent.lock().unwrap().len() == 5).await);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(mailer.sent.lock().unwrap().len(), 5);

    // developers may send, viewers may not
    for (role, want) in [
        ("developer", StatusCode::ACCEPTED),
        ("viewer", StatusCode::FORBIDDEN),
    ] {
        let made = ok(
            &app,
            &token,
            "POST",
            "/api/team/keys",
            json!({ "name": role, "role": role }),
        )
        .await;
        let (status, res) = call(
            &app,
            made["key"].as_str().unwrap(),
            "POST",
            &send,
            Some(json!({ "to": "f@example.com", "data": { "name": "F" } })),
        )
        .await;
        assert_eq!(status, want, "{role}: {res}");
    }
}
