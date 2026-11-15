//! Automations started by a record's change. See `setup_flow.rs` for how to point the tests
//! at a database.

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
    for _ in 0..200 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn collection(app: &Arc<App>, token: &str, name: &str, fields: Value) {
    ok(
        app,
        token,
        "POST",
        "/api/collections",
        json!({ "name": name, "fields": fields }),
    )
    .await;
}

/// The `note` of every record of a collection, sorted.
async fn notes(app: &Arc<App>, token: &str, name: &str) -> Vec<String> {
    let (_, res) = call(
        app,
        token,
        "GET",
        &format!("/api/collections/{name}/records?perPage=200"),
        None,
    )
    .await;
    let mut notes: Vec<String> = res["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|r| r["note"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    notes.sort();
    notes
}

/// An automation that, when `watched` changes, adds a record to `into` with `note`.
fn noting(name: &str, watched: &str, actions: &[&str], into: &str, note: &str) -> Value {
    json!({
        "name": name,
        "type": "steps",
        "trigger": { "type": "record", "collection": watched, "actions": actions },
        "config": { "steps": [
            { "name": "note", "type": "write", "config": {
                "action": "create", "collection": into, "data": { "note": note } } },
        ] },
    })
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(1500)).await;
}

#[tokio::test]
async fn a_change_starts_its_automations_once() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    collection(
        &app,
        &token,
        "orders",
        json!([{ "name": "customer", "type": "text" }, { "name": "paid", "type": "bool" }, { "name": "secret", "type": "text", "hidden": true }]),
    )
    .await;
    collection(
        &app,
        &token,
        "audit",
        json!([{ "name": "note", "type": "text" }]),
    )
    .await;

    // checked when saved
    for (trigger, field) in [
        (
            json!({ "type": "record", "collection": "nope", "actions": ["create"] }),
            "collection",
        ),
        (
            json!({ "type": "record", "collection": "_superusers", "actions": ["create"] }),
            "collection",
        ),
        (
            json!({ "type": "record", "collection": "orders", "actions": [] }),
            "actions",
        ),
        (
            json!({ "type": "record", "collection": "orders", "actions": ["drop"] }),
            "actions",
        ),
        (
            json!({ "type": "record", "collection": "orders", "actions": ["create"], "condition": "trigger.record.paid ==" }),
            "condition",
        ),
    ] {
        let mut body = noting("bad", "orders", &["create"], "audit", "x");
        body["trigger"] = trigger;
        let (status, res) = call(&app, &token, "POST", "/api/automations", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {res}");
        assert!(res["data"]["trigger"][field].is_object(), "{field}: {res}");
    }
    // a schedule or a change, not both
    let mut both = noting("both", "orders", &["create"], "audit", "x");
    both["expression"] = json!("* * * * *");
    let (status, res) = call(&app, &token, "POST", "/api/automations", Some(both)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");

    // creates and updates of paid orders are noted, with what changed
    let mut paid = noting(
        "Paid orders",
        "orders",
        &["create", "update"],
        "audit",
        "{{ trigger.action }}:{{ trigger.record.customer }}:{{ 'paid' in trigger.changed }}",
    );
    paid["trigger"]["condition"] = json!("trigger.record.paid");
    let saved = ok(&app, &token, "POST", "/api/automations", paid).await;
    let id = saved["id"].as_str().unwrap().to_string();
    assert_eq!(saved["trigger"]["type"], "record");
    assert_eq!(saved["expression"], "");
    assert_eq!(saved["description"], "When a record changes");
    // (stored by the collection's id, so a rename keeps it)
    assert_ne!(saved["trigger"]["collection"], "orders");
    // a second automation on the same collection: deletes
    ok(
        &app,
        &token,
        "POST",
        "/api/automations",
        noting(
            "Gone",
            "orders",
            &["delete"],
            "audit",
            "gone:{{ trigger.record.customer }}",
        ),
    )
    .await;
    // (the dispatcher starts from the moment the first automation of this kind exists)
    settle().await;

    let ann = ok(
        &app,
        &token,
        "POST",
        "/api/collections/orders/records",
        json!({ "customer": "Ann", "paid": true, "secret": "s3cr3t" }),
    )
    .await;
    let bob = ok(
        &app,
        &token,
        "POST",
        "/api/collections/orders/records",
        json!({ "customer": "Bob", "paid": false }),
    )
    .await;
    assert!(
        eventually(async || notes(&app, &token, "audit").await == ["create:Ann:False"]).await,
        "{:?}",
        notes(&app, &token, "audit").await
    );
    // Bob pays: an update that the condition now holds for
    ok(
        &app,
        &token,
        "PATCH",
        &format!(
            "/api/collections/orders/records/{}",
            bob["id"].as_str().unwrap()
        ),
        json!({ "paid": true }),
    )
    .await;
    // Ann is deleted: the other automation's
    let (status, _) = call(
        &app,
        &token,
        "DELETE",
        &format!(
            "/api/collections/orders/records/{}",
            ann["id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let want = ["create:Ann:False", "gone:Ann", "update:Bob:True"];
    assert!(
        eventually(async || notes(&app, &token, "audit").await == want).await,
        "{:?}",
        notes(&app, &token, "audit").await
    );
    // each change ran once, and the history says what started it
    settle().await;
    assert_eq!(notes(&app, &token, "audit").await, want);
    let (_, runs) = call(
        &app,
        &token,
        "GET",
        &format!("/api/jobs/runs?kind=cron&job={id}"),
        None,
    )
    .await;
    assert_eq!(runs["total"], 2, "{runs}");
    assert_eq!(runs["items"][0]["trigger"], "record");
    assert_eq!(runs["items"][0]["status"], "ok");

    // switched off: changes made meanwhile start nothing, then or later
    ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        json!({ "enabled": false }),
    )
    .await;
    ok(
        &app,
        &token,
        "POST",
        "/api/collections/orders/records",
        json!({ "customer": "Cy", "paid": true }),
    )
    .await;
    settle().await;
    ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        json!({ "enabled": true }),
    )
    .await;
    settle().await;
    assert_eq!(notes(&app, &token, "audit").await, want);
}

#[tokio::test]
async fn a_chain_of_changes_ends() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    for name in ["a", "b", "c", "d", "e"] {
        collection(
            &app,
            &token,
            name,
            json!([{ "name": "note", "type": "text" }, { "name": "seen", "type": "number" }]),
        )
        .await;
    }
    // an automation that changes the record that started it: its own change doesn't start it
    let own = ok(
        &app,
        &token,
        "POST",
        "/api/automations",
        json!({
            "name": "Count",
            "type": "steps",
            "trigger": { "type": "record", "collection": "a", "actions": ["create", "update"] },
            "config": { "steps": [
                { "name": "count", "type": "write", "config": {
                    "action": "update", "collection": "a", "id": "{{ trigger.record.id }}",
                    "data": { "seen": 1 } } },
            ] },
        }),
    )
    .await;
    // a → b → c → d → e, each by an automation: three deep, and no further
    for (from, to) in [("a", "b"), ("b", "c"), ("c", "d"), ("d", "e")] {
        ok(
            &app,
            &token,
            "POST",
            "/api/automations",
            noting(
                &format!("{from} to {to}"),
                from,
                &["create"],
                to,
                "{{ trigger.record.note }}",
            ),
        )
        .await;
    }
    settle().await;
    let first = ok(
        &app,
        &token,
        "POST",
        "/api/collections/a/records",
        json!({ "note": "n", "seen": 0 }),
    )
    .await;
    assert!(eventually(async || notes(&app, &token, "d").await == ["n"]).await);
    settle().await;
    assert_eq!(notes(&app, &token, "b").await, ["n"]);
    assert_eq!(notes(&app, &token, "c").await, ["n"]);
    assert_eq!(notes(&app, &token, "d").await, ["n"]);
    assert!(
        notes(&app, &token, "e").await.is_empty(),
        "the chain ends at three"
    );
    // counted once: the update the automation made did not start it again
    let (_, record) = call(
        &app,
        &token,
        "GET",
        &format!(
            "/api/collections/a/records/{}",
            first["id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(record["seen"], 1, "{record}");
    let (_, runs) = call(
        &app,
        &token,
        "GET",
        &format!(
            "/api/jobs/runs?kind=cron&job={}",
            own["id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(runs["total"], 1, "{runs}");
    // someone else's update of the same record does start it
    ok(
        &app,
        &token,
        "PATCH",
        &format!(
            "/api/collections/a/records/{}",
            first["id"].as_str().unwrap()
        ),
        json!({ "note": "again" }),
    )
    .await;
    assert!(
        eventually(async || {
            let (_, runs) = call(
                &app,
                &token,
                "GET",
                &format!(
                    "/api/jobs/runs?kind=cron&job={}",
                    own["id"].as_str().unwrap()
                ),
                None,
            )
            .await;
            runs["total"] == 2
        })
        .await
    );
    settle().await;
    let (_, runs) = call(
        &app,
        &token,
        "GET",
        &format!(
            "/api/jobs/runs?kind=cron&job={}",
            own["id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(runs["total"], 2, "{runs}");
}

#[tokio::test]
async fn a_webhook_is_signed_with_a_secret_made_here() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let core = app.core().unwrap();
    let hook = |name: &str, config: Value| {
        json!({
            "name": "Tell",
            "expression": "0 0 29 2 *",
            "type": "steps",
            "config": { "steps": [{ "name": name, "type": "webhook", "config": config }] },
        })
    };
    // only to https, and the address is typed
    for url in [
        "http://93.184.216.34/hook",
        "https://{{ item.host }}/hook",
        "https://127.0.0.1/hook",
    ] {
        let (status, res) = call(
            &app,
            &token,
            "POST",
            "/api/automations",
            Some(hook("tell", json!({ "url": url }))),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url}: {res}");
        assert!(
            res["data"]["config"]["steps"]["0"]["config"]["url"].is_object(),
            "{url}: {res}"
        );
    }
    // a secret is made, shown this once, and hidden from then on
    let saved = ok(
        &app,
        &token,
        "POST",
        "/api/automations",
        hook("tell", json!({ "url": "https://93.184.216.34/hooks/{{ trigger.type }}", "payload": { "kind": "{{ trigger.type }}" } })),
    )
    .await;
    let id = saved["id"].as_str().unwrap().to_string();
    let first = saved["secrets"]["steps"]["tell"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(first.starts_with("whsec_"), "{saved}");
    assert_eq!(saved["config"]["steps"][0]["config"]["secret"], "••••••••");
    let (_, list) = call(&app, &token, "GET", "/api/automations", None).await;
    assert!(!list.to_string().contains(&first), "{list}");
    let stored = |core: &slonbase::core::Core| {
        core.settings().cron_jobs[0].config["steps"][0]["config"].clone()
    };
    assert_eq!(stored(&core)["secret"], first.as_str());

    // sent back hidden (and the step renamed): kept, nothing new is shown
    let kept = ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        json!({ "config": { "steps": [{ "name": "notify", "was": "tell", "type": "webhook", "config": {
            "url": "https://93.184.216.34/hooks/x", "secret": "••••••••" } }] } }),
    )
    .await;
    assert!(kept.get("secrets").is_none(), "{kept}");
    assert_eq!(stored(&core)["secret"], first.as_str());

    // replaced: a new one is shown, and the old one keeps signing for a day
    let rotated = ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        json!({ "config": { "steps": [{ "name": "notify", "type": "webhook", "config": {
            "url": "https://93.184.216.34/hooks/x", "secret": "••••••••", "rotate": true } }] } }),
    )
    .await;
    let second = rotated["secrets"]["steps"]["notify"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(second, first);
    assert_eq!(stored(&core)["secret"], second.as_str());
    assert_eq!(stored(&core)["previousSecret"], first.as_str());
    let shown = &rotated["config"]["steps"][0]["config"];
    assert!(shown["previousUntil"].is_string(), "{rotated}");
    assert!(shown.get("previousSecret").is_none(), "{rotated}");
    // (and the old one stays through a later save)
    ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{id}"),
        json!({ "name": "Tell them", "config": { "steps": [{ "name": "notify", "type": "webhook", "config": {
            "url": "https://93.184.216.34/hooks/x", "secret": "••••••••" } }] } }),
    )
    .await;
    assert_eq!(stored(&core)["previousSecret"], first.as_str());
    // a hidden secret with none saved, or one that isn't a secret, is refused
    for secret in ["••••••••", "mine"] {
        let (status, res) = call(
            &app,
            &token,
            "POST",
            "/api/automations",
            Some(hook(
                "other",
                json!({ "url": "https://93.184.216.34/h", "secret": secret }),
            )),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{secret}: {res}");
    }

    // a notice webhook is signed the same way
    let mut noticed = hook("tell", json!({ "url": "https://93.184.216.34/hooks/x" }));
    noticed["name"] = json!("Noticed");
    noticed["notify"] = json!({ "webhook": "http://93.184.216.34/notice" });
    let (status, res) = call(
        &app,
        &token,
        "POST",
        "/api/automations",
        Some(noticed.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
    assert!(res["data"]["notify"]["webhook"].is_object(), "{res}");
    noticed["notify"] = json!({ "webhook": "https://93.184.216.34/notice" });
    let saved = ok(&app, &token, "POST", "/api/automations", noticed).await;
    let notice = saved["secrets"]["notify"].as_str().unwrap().to_string();
    assert!(notice.starts_with("whsec_"));
    let other = saved["id"].as_str().unwrap();
    let again = ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{other}"),
        json!({ "notify": { "webhook": "••••••••" } }),
    )
    .await;
    assert!(again["secrets"].get("notify").is_none(), "{again}");
    let rotated = ok(
        &app,
        &token,
        "PATCH",
        &format!("/api/automations/{other}"),
        json!({ "notify": { "webhook": "••••••••", "rotateSecret": true } }),
    )
    .await;
    assert_ne!(rotated["secrets"]["notify"], notice.as_str());
    let settings = core.settings();
    let notify = &settings
        .cron_jobs
        .iter()
        .find(|j| j.id == other)
        .unwrap()
        .notify;
    assert_eq!(notify.previous_secret, notice);
    assert!(!notify.previous_until.is_empty());
}
