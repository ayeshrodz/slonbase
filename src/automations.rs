//! Automations: a schedule and a list of steps, where each step can use what the steps before it
//! produced. "Every morning, find the invoices that are overdue and send each customer the
//! reminder template" is three steps and no code.
//!
//! An automation is a dashboard job (see [`crate::cron_jobs`]) of type `steps`: its schedule,
//! retries, notices, history, roles and storage are the job's, and this module is what its
//! type does. So everything that holds for a job holds for an automation.
//!
//! - **Steps** are the building blocks jobs already had (`http`, `sql`, `mail`, `cleanup`) and
//!   four more: `query` (records of a collection), `condition` (stop unless something holds),
//!   `email` (a template of the library) and `write` (create, update or delete a record). Each
//!   goes through the same service as everything else that does that work, with its rules:
//!   public addresses only, the SQL console's policy, the limit on recipients, the record
//!   service.
//! - **Data** flows by name: a step called `overdue` is `steps.overdue` to the steps after it,
//!   in Jinja expressions (see [`crate::templating`]). A step with `forEach` runs once per item
//!   of a list, with the item as `item`.
//! - **SQL is never built from data.** A statement is run as it was saved; a filter takes data
//!   only through its `{:name}` parameters, which are bound, not pasted.
//! - **A retry continues.** What the finished steps produced is kept with the run, so an attempt
//!   that follows a failed one starts at the step (and the item) that failed: an email that was
//!   sent is not sent again.
//! - **The history says what happened, not what was read:** counts and statuses, never rows or
//!   responses.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::collections::model::CollectionKind;
use crate::core::Core;
use crate::cron_jobs::{self, JobDef};
use crate::error::ApiError;
use crate::jobs::Run;
use crate::services::access::Access;
use crate::services::{self, Failure, mail, outbound};
use crate::templating::{self, Mode};
use crate::validation::Errors;

/// The type of a job that is a list of steps.
pub const KIND: &str = "steps";
pub const MAX_STEPS: usize = 20;
/// The most rows a `query` or `sql` step hands on, and the most items of a `forEach`.
pub const MAX_ROWS: i64 = 1000;
/// The most one step's result may hold, as JSON.
const MAX_RESULT: usize = 512 * 1024;
const TYPES: &[&str] = &[
    "query",
    "sql",
    "http",
    "condition",
    "email",
    "mail",
    "write",
    "cleanup",
];
/// The steps that can run once per item of a list.
const REPEATABLE: &[&str] = &["http", "email", "mail", "write"];
const ACTIONS: &[&str] = &["create", "update", "delete"];

/// One step, as it is stored.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Step {
    /// How later steps refer to it: `steps.<name>`.
    pub name: String,
    pub kind: String,
    pub config: Value,
    /// An expression for a list: the step runs once per item.
    pub for_each: String,
}

fn text<'a>(config: &'a Value, key: &str) -> &'a str {
    config.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The steps of a job's settings.
pub(crate) fn steps_of(config: &Value) -> Vec<Step> {
    config
        .get("steps")
        .and_then(Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .map(|s| Step {
                    name: text(s, "name").trim().to_string(),
                    kind: text(s, "type").to_string(),
                    config: s.get("config").cloned().unwrap_or_else(|| json!({})),
                    for_each: text(s, "forEach").trim().to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn step_json(step: &Step, config: Value) -> Value {
    let mut out = json!({ "name": step.name, "type": step.kind, "config": config });
    if !step.for_each.is_empty() {
        out["forEach"] = json!(step.for_each);
    }
    out
}

/// Whether a job does something of `kind`, as its type or as one of its steps.
pub(crate) fn does(def: &JobDef, kinds: &[&str]) -> bool {
    match def.kind == KIND {
        true => steps_of(&def.config)
            .iter()
            .any(|s| kinds.contains(&s.kind.as_str())),
        false => kinds.contains(&def.kind.as_str()),
    }
}

/// The name of an automation that sends the template `id`, if one does.
pub(crate) fn using_template(jobs: &[JobDef], id: &str) -> Option<String> {
    jobs.iter()
        .find(|job| {
            job.kind == KIND
                && steps_of(&job.config)
                    .iter()
                    .any(|s| s.kind == "email" && text(&s.config, "template") == id)
        })
        .map(|job| job.name.clone())
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 30
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A collection steps may read and write: not a system one.
fn user_collection(
    core: &Core,
    name: &str,
    writable: bool,
) -> Result<Arc<crate::collections::model::Collection>, String> {
    let Ok(collection) = core.collection(name) else {
        return Err("Choose a collection.".into());
    };
    if collection.system || collection.name.starts_with('_') {
        return Err("System collections can't be used in a step.".into());
    }
    if writable && collection.kind == CollectionKind::View {
        return Err("A view is read-only.".into());
    }
    Ok(collection)
}

/// Checks the steps of a job as the dashboard sends them. Errors are keyed
/// `config.steps.<n>.<field>`.
pub(crate) async fn validate(core: &Arc<Core>, config: &Value, errors: &mut Errors) {
    let steps = steps_of(config);
    if steps.is_empty() || steps.len() > MAX_STEPS {
        errors.invalid("config.steps", "An automation has 1 to 20 steps.");
        return;
    }
    let mut names: Vec<&str> = Vec::new();
    for (n, step) in steps.iter().enumerate() {
        let at = |field: &str| format!("config.steps.{n}.{field}");
        if !valid_name(&step.name) {
            errors.invalid(
                at("name"),
                "A short name in lower case letters, digits and _, starting with a letter.",
            );
        } else if names.contains(&step.name.as_str()) {
            errors.invalid(at("name"), "Another step has this name.");
        }
        names.push(&step.name);
        if !TYPES.contains(&step.kind.as_str()) {
            errors.invalid(at("type"), "Choose what the step does.");
            continue;
        }
        if !step.for_each.is_empty() {
            if !REPEATABLE.contains(&step.kind.as_str()) {
                errors.invalid(at("forEach"), "This kind of step runs once.");
            } else if let Err(message) = templating::check_expression(&step.for_each) {
                errors.invalid(at("forEach"), message);
            }
        }
        let mut own = Errors::new();
        validate_step(core, step, &mut own).await;
        errors.extend_prefixed(&format!("config.steps.{n}"), own);
    }
}

/// Checks one step; errors are keyed `config.<field>` as for a job of that type.
async fn validate_step(core: &Arc<Core>, step: &Step, errors: &mut Errors) {
    let c = &step.config;
    // what is a template is checked as one; what it comes to is checked when it runs
    if let Err(message) = templating::check_value(c) {
        errors.invalid("config", message);
        return;
    }
    match step.kind.as_str() {
        "cleanup" => cron_jobs::validate_cleanup(core, c, errors).await,
        "sql" => {
            cron_jobs::validate_sql(core, c, errors);
            if templating::is_template(text(c, "statement")) {
                errors.invalid(
                    "config.statement",
                    "A statement is run as it is saved: it can't be built from data.",
                );
            }
        }
        "http" => {
            // (an address built from data is checked when it is called)
            let mut fixed = c.clone();
            if templating::is_template(text(c, "url")) {
                fixed["url"] = json!("https://example.com/");
            }
            cron_jobs::validate_http(&fixed, errors).await;
        }
        "mail" => {
            let mut fixed = c.clone();
            if templated(c.get("to")) {
                fixed["to"] = json!(["someone@example.com"]);
            }
            if templating::is_template(text(c, "subject")) {
                fixed["subject"] = json!("Subject");
            }
            cron_jobs::validate_mail(core, &fixed, errors);
        }
        "query" => {
            let name = text(c, "collection");
            if let Err(message) = user_collection(core, name, false) {
                errors.invalid("config.collection", message);
                return;
            }
            if text(c, "filter").chars().count() > 2000 {
                errors.invalid("config.filter", "At most 2000 characters.");
            } else if templating::is_template(text(c, "filter")) {
                errors.invalid(
                    "config.filter",
                    "A filter takes data through its {:name} parameters, not as text.",
                );
            }
            if text(c, "sort").chars().count() > 200 {
                errors.invalid("config.sort", "At most 200 characters.");
            }
            if c.get("limit").is_some_and(|v| !v.is_null())
                && !matches!(cron_jobs::number(c, "limit"), Some(1..=MAX_ROWS))
            {
                errors.invalid("config.limit", "Must be between 1 and 1000.");
            }
            if c.get("params")
                .is_some_and(|p| !p.is_object() && !p.is_null())
            {
                errors.invalid("config.params", "Parameters are name and value pairs.");
            }
            // the filter must compile against the collection (with its parameters as texts)
            if errors.is_empty() {
                let params: Map<String, Value> = c
                    .get("params")
                    .and_then(Value::as_object)
                    .map(|p| p.keys().map(|k| (k.clone(), json!(""))).collect())
                    .unwrap_or_default();
                if let Err(err) = core.count_records(name, text(c, "filter"), &params).await {
                    errors.invalid("config.filter", err.message);
                }
            }
        }
        "condition" => {
            let expression = text(c, "expression").trim();
            if expression.is_empty() || expression.chars().count() > 500 {
                errors.invalid("config.expression", "1 to 500 characters.");
            } else if let Err(message) = templating::check_expression(expression) {
                errors.invalid("config.expression", message);
            }
        }
        "email" => {
            let settings = core.settings();
            if !settings
                .templates
                .iter()
                .any(|t| t.id == text(c, "template"))
            {
                errors.invalid("config.template", "Choose a template.");
            }
            if !templated(c.get("to")) {
                if let Err(message) = mail::check_addresses(&mail::recipients(c.get("to"))) {
                    errors.invalid("config.to", message);
                }
            }
            if c.get("data")
                .is_some_and(|d| !d.is_object() && !d.is_null())
            {
                errors.invalid("config.data", "Data is a JSON object.");
            }
            if !mail::enabled(core) {
                errors.invalid("config.to", mail::DISABLED);
            }
        }
        "write" => {
            let action = text(c, "action");
            if !ACTIONS.contains(&action) {
                errors.invalid("config.action", "Choose create, update or delete.");
            }
            if let Err(message) = user_collection(core, text(c, "collection"), true) {
                errors.invalid("config.collection", message);
            }
            if action != "create" && text(c, "id").trim().is_empty() {
                errors.invalid("config.id", "Which record: its id.");
            }
            if action != "delete" && !c.get("data").is_some_and(Value::is_object) {
                errors.invalid("config.data", "The fields to set, as a JSON object.");
            }
        }
        _ => {}
    }
}

/// Whether an address list is, or holds, a template.
fn templated(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(s)) => templating::is_template(s),
        Some(Value::Array(items)) => items.iter().any(|v| templated(Some(v))),
        _ => false,
    }
}

/// The steps as stored: for each, only the keys its type uses.
pub(crate) fn normalized(config: &Value) -> Value {
    let steps: Vec<Value> = steps_of(config)
        .iter()
        .map(|step| {
            let c = &step.config;
            let config = match step.kind.as_str() {
                "query" => json!({
                    "collection": text(c, "collection"),
                    "filter": text(c, "filter").trim(),
                    "sort": text(c, "sort").trim(),
                    "limit": cron_jobs::number(c, "limit").unwrap_or(100),
                    "params": c.get("params").filter(|p| p.is_object()).cloned().unwrap_or_else(|| json!({})),
                }),
                "condition" => json!({ "expression": text(c, "expression").trim() }),
                "email" => json!({
                    "template": text(c, "template"),
                    "to": c.get("to").cloned().unwrap_or_else(|| json!([])),
                    "data": c.get("data").filter(|d| d.is_object()).cloned().unwrap_or_else(|| json!({})),
                }),
                "write" => json!({
                    "action": text(c, "action"),
                    "collection": text(c, "collection"),
                    "id": text(c, "id").trim(),
                    "data": c.get("data").filter(|d| d.is_object()).cloned().unwrap_or_else(|| json!({})),
                }),
                // (a mail step's addresses may be a template: kept as typed)
                "mail" if templated(c.get("to")) => json!({
                    "to": c.get("to").cloned().unwrap_or_default(),
                    "subject": text(c, "subject").trim(),
                    "body": text(c, "body"),
                }),
                kind => cron_jobs::normalized_config(kind, c),
            };
            step_json(step, config)
        })
        .collect();
    json!({ "steps": steps })
}

/// The steps as the API shows them: header values hidden.
pub(crate) fn masked(config: &Value) -> Value {
    let steps: Vec<Value> = steps_of(config)
        .iter()
        .map(|step| step_json(step, cron_jobs::masked_config(&step.kind, &step.config)))
        .collect();
    json!({ "steps": steps })
}

/// Puts the saved header values back where the mask came back, step by step (a step is the
/// same step when it has the same name). A job that had one type before is its first step.
pub(crate) fn unmask(config: &mut Value, saved: &JobDef) -> Result<(), String> {
    let saved_steps = match saved.kind == KIND {
        true => steps_of(&saved.config),
        false => vec![Step {
            name: String::new(),
            kind: saved.kind.clone(),
            config: saved.config.clone(),
            for_each: String::new(),
        }],
    };
    let Some(steps) = config.get_mut("steps").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for (n, step) in steps.iter_mut().enumerate() {
        let name = text(step, "name").trim().to_string();
        let kind = text(step, "type").to_string();
        let was = saved_steps
            .iter()
            .find(|s| s.name == name)
            .or_else(|| saved_steps.get(n).filter(|s| s.name.is_empty()));
        let empty = json!({});
        let (was_kind, was_config) = match was {
            Some(s) => (s.kind.as_str(), &s.config),
            None => ("", &empty),
        };
        if let Some(c) = step.get_mut("config") {
            cron_jobs::unmask_config(&kind, c, was_kind, was_config)?;
        }
    }
    Ok(())
}

fn api_failure(err: ApiError) -> Failure {
    let message = err.details.as_deref().unwrap_or(&err.message).to_string();
    match err.status.is_client_error() {
        true => Failure::permanent(message),
        false => Failure::transient(message),
    }
}

fn bad(message: impl Into<String>) -> Failure {
    Failure::permanent(message)
}

/// Runs the steps of `def` for `record`'s run, continuing where an earlier attempt stopped.
/// Returns what to record: each step with what it came to.
pub(crate) async fn run(core: &Arc<Core>, def: &JobDef, record: &Run) -> Result<Value, Failure> {
    let steps = steps_of(&def.config);
    let kept = record
        .kept()
        .await
        .map_err(|e| Failure::transient(format!("{e:#}")))?
        .unwrap_or_else(|| json!({}));
    let next = kept["next"].as_u64().unwrap_or(0) as usize;
    let row = kept["row"].as_u64().unwrap_or(0) as usize;
    let mut outputs = kept["outputs"].as_object().cloned().unwrap_or_default();
    let mut summary = kept["summary"].as_array().cloned().unwrap_or_default();
    let mut stopped = None;
    for (i, step) in steps.iter().enumerate().skip(next) {
        let named = |f: Failure| Failure {
            message: format!("Step {}: {}", step.name, f.message),
            ..f
        };
        let context = |item: &Value, index: usize| {
            json!({
                "steps": outputs,
                "item": item,
                "index": index,
                "automation": { "id": def.id, "name": def.name },
                "now": crate::datetime::format(&chrono::Utc::now()),
            })
        };
        let output = if step.for_each.is_empty() {
            run_step(core, step, &context(&Value::Null, 0))
                .await
                .map_err(named)?
        } else {
            let list = templating::eval(&step.for_each, &context(&Value::Null, 0))
                .map_err(|e| named(bad(format!("For each: {e}"))))?;
            let items = match list {
                Value::Array(items) => items,
                Value::Null => Vec::new(),
                _ => return Err(named(bad("For each: the expression is not a list."))),
            };
            if items.len() > MAX_ROWS as usize {
                return Err(named(bad(format!(
                    "For each: {} items, at most {MAX_ROWS}.",
                    items.len()
                ))));
            }
            let start = if i == next { row } else { 0 };
            for (r, item) in items.iter().enumerate().skip(start) {
                run_step(core, step, &context(item, r))
                    .await
                    .map_err(named)?;
                // (kept per item: a retry goes on with the next one)
                record
                    .keep_number("row", r as i64 + 1)
                    .await
                    .map_err(|e| Failure::transient(format!("{e:#}")))?;
            }
            json!({ "count": items.len() })
        };
        if output.to_string().len() > MAX_RESULT {
            return Err(named(bad(format!(
                "The result is larger than {} KB: lower the step's limit.",
                MAX_RESULT / 1024
            ))));
        }
        let halts = step.kind == "condition" && output["passed"] != true;
        summary.push(brief(step, &output));
        outputs.insert(step.name.clone(), output);
        record
            .keep(&json!({ "next": i + 1, "row": 0, "outputs": outputs, "summary": summary }))
            .await
            .map_err(|e| Failure::transient(format!("{e:#}")))?;
        if halts {
            stopped = Some(step.name.clone());
            break;
        }
    }
    let mut meta = json!({ "steps": summary });
    if let Some(name) = stopped {
        meta["stoppedAt"] = json!(name);
    }
    Ok(meta)
}

/// What the history keeps of a step: what it came to in numbers, never what it read.
fn brief(step: &Step, output: &Value) -> Value {
    let mut out = json!({ "name": step.name, "type": step.kind });
    let keys: &[&str] = match step.kind.as_str() {
        _ if !step.for_each.is_empty() => &["count"],
        "query" => &["count"],
        "sql" => &["count", "affectedRows"],
        "http" => &["status"],
        "condition" => &["passed"],
        "email" | "mail" => &["sent"],
        "cleanup" => &["deleted"],
        _ => &[],
    };
    for key in keys {
        if let Some(v) = output.get(*key).filter(|v| v.is_number() || v.is_boolean()) {
            out[*key] = v.clone();
        }
    }
    out
}

/// A record as steps see it: without the fields no API shows (password hashes, token keys).
fn visible(
    collection: &crate::collections::model::Collection,
    mut record: Map<String, Value>,
) -> Value {
    for field in collection.fields.iter().filter(|f| f.is_secret()) {
        record.remove(&field.name);
    }
    Value::Object(record)
}

/// The addresses a rendered `to` comes to.
fn addresses(to: Option<&Value>, context: &Value) -> Result<Vec<String>, Failure> {
    let rendered = match to {
        Some(to) => templating::render_value(to, context).map_err(|e| bad(format!("To: {e}")))?,
        None => Value::Null,
    };
    let list = mail::recipients(Some(&rendered));
    mail::check_addresses(&list).map_err(|e| bad(format!("To: {e}")))?;
    Ok(list)
}

/// Runs one step once and returns what it produced, for the steps after it.
async fn run_step(core: &Arc<Core>, step: &Step, context: &Value) -> Result<Value, Failure> {
    let c = &step.config;
    let rendered = |key: &str| -> Result<Value, Failure> {
        match c.get(key) {
            Some(v) => templating::render_value(v, context).map_err(|e| bad(format!("{key}: {e}"))),
            None => Ok(Value::Null),
        }
    };
    let rendered_text = |key: &str| -> Result<String, Failure> {
        templating::render(text(c, key), context, Mode::Text)
            .map_err(|e| bad(format!("{key}: {e}")))
    };
    match step.kind.as_str() {
        "query" => {
            let name = text(c, "collection");
            let collection = user_collection(core, name, false).map_err(bad)?;
            let params = match rendered("params")? {
                Value::Object(params) => params,
                _ => Map::new(),
            };
            let limit = cron_jobs::number(c, "limit")
                .unwrap_or(100)
                .clamp(1, MAX_ROWS);
            let records = core
                .find_records_by_filter(name, text(c, "filter"), text(c, "sort"), limit, 0, &params)
                .await
                .map_err(api_failure)?;
            let rows: Vec<Value> = records
                .into_iter()
                .map(|r| visible(&collection, r))
                .collect();
            Ok(json!({ "count": rows.len(), "rows": rows }))
        }
        "sql" => {
            // (a scheduled statement: an owner set it up, the server runs it)
            let result = services::sql::query(
                core,
                Access::System,
                text(c, "statement"),
                MAX_ROWS as usize,
            )
            .await?;
            let columns: Vec<&str> = result["columns"]
                .as_array()
                .map(|cols| cols.iter().map(|col| text(col, "name")).collect())
                .unwrap_or_default();
            let rows: Vec<Value> = result["rows"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .map(|row| {
                            let cells = row.as_array().cloned().unwrap_or_default();
                            Value::Object(
                                columns
                                    .iter()
                                    .zip(cells)
                                    .map(|(name, cell)| (name.to_string(), cell))
                                    .collect(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(json!({
                "count": rows.len(),
                "rows": rows,
                "affectedRows": result["affectedRows"],
            }))
        }
        "http" => {
            let url = rendered_text("url")?;
            let body = rendered_text("body")?;
            let headers: Vec<(String, String)> = match rendered("headers")? {
                Value::Object(h) => h
                    .into_iter()
                    .map(|(k, v)| {
                        let value = match v {
                            Value::String(s) => s,
                            other => other.to_string(),
                        };
                        (k.trim().to_string(), value)
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let timeout = cron_jobs::number(c, "timeoutSeconds")
                .unwrap_or(30)
                .clamp(1, 60);
            let reply = outbound::send(outbound::Request {
                method: match text(c, "method") {
                    "" => "GET",
                    m => m,
                },
                url: url.trim(),
                headers: &headers,
                body: Some(body.as_str()).filter(|b| !b.is_empty()),
                timeout: Duration::from_secs(timeout as u64),
            })
            .await?;
            let ok = match cron_jobs::number(c, "expectStatus") {
                Some(want) => i64::from(reply.status) == want,
                None => (200..300).contains(&reply.status),
            };
            if !ok {
                return Err(Failure::transient(format!(
                    "The server answered {}.",
                    reply.status
                )));
            }
            Ok(json!({
                "status": reply.status,
                "json": serde_json::from_str::<Value>(&reply.body).unwrap_or(Value::Null),
                "text": reply.excerpt,
            }))
        }
        "condition" => {
            let passed = templating::holds(text(c, "expression"), context).map_err(bad)?;
            Ok(json!({ "passed": passed }))
        }
        "email" => {
            let to = addresses(c.get("to"), context)?;
            // the template sees the step's data, and under it what every step sees
            let mut data = context.as_object().cloned().unwrap_or_default();
            if let Value::Object(own) = rendered("data")? {
                data.extend(own);
            }
            let sent = crate::templates::send(core, text(c, "template"), &to, &Value::Object(data))
                .await?;
            Ok(json!({ "sent": sent }))
        }
        "mail" => {
            let to = addresses(c.get("to"), context)?;
            let subject = rendered_text("subject")?;
            let subject = subject.trim();
            if subject.is_empty() || subject.chars().count() > 200 || !mail::plain_line(subject) {
                return Err(bad(
                    "The subject must come to 1 to 200 characters on one line.",
                ));
            }
            let sent = mail::send_text(core, &to, subject, &rendered_text("body")?).await?;
            Ok(json!({ "sent": sent }))
        }
        "write" => {
            let name = text(c, "collection");
            user_collection(core, name, true).map_err(bad)?;
            let id = rendered_text("id")?.trim().to_string();
            let data = match rendered("data")? {
                Value::Object(data) => data,
                _ => Map::new(),
            };
            match text(c, "action") {
                "create" => {
                    let mut data = data;
                    // (an id that exists would change that record: a create never does)
                    data.remove("id");
                    let record = core.save_record(name, data).await.map_err(api_failure)?;
                    Ok(json!({ "id": record.get("id") }))
                }
                "update" => {
                    let found = core
                        .find_record_by_id(name, &id)
                        .await
                        .map_err(api_failure)?;
                    if id.is_empty() || found.is_none() {
                        return Err(bad(format!("No record {id:?} in {name}.")));
                    }
                    let mut data = data;
                    data.insert("id".into(), json!(id));
                    core.save_record(name, data).await.map_err(api_failure)?;
                    Ok(json!({ "id": id }))
                }
                "delete" => {
                    if id.is_empty() {
                        return Err(bad("No record id."));
                    }
                    match core.delete_record(name, &id).await {
                        Ok(()) => {}
                        // (gone already: what the step wanted)
                        Err(err) if err.status == axum::http::StatusCode::NOT_FOUND => {}
                        Err(err) => return Err(api_failure(err)),
                    }
                    Ok(json!({ "id": id }))
                }
                other => Err(bad(format!("Unknown action: {other}."))),
            }
        }
        "cleanup" => cron_jobs::cleanup(core, c)
            .await
            .map_err(Failure::transient),
        other => Err(bad(format!("Unknown kind of step: {other}."))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_names_are_identifiers() {
        for good in ["a", "overdue", "step_2", "x1"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "2x", "Overdue", "a b", "a-b", "é", &"x".repeat(31)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_history_keeps_numbers_not_content() {
        let step = |kind: &str, for_each: &str| Step {
            name: "s".into(),
            kind: kind.into(),
            config: json!({}),
            for_each: for_each.into(),
        };
        let rows = json!({ "count": 2, "rows": [{ "email": "a@b.co" }, { "email": "c@d.co" }] });
        let kept = brief(&step("query", ""), &rows);
        assert_eq!(kept, json!({ "name": "s", "type": "query", "count": 2 }));
        let reply = json!({ "status": 200, "json": { "token": "s3cret" }, "text": "s3cret" });
        let kept = brief(&step("http", ""), &reply);
        assert_eq!(kept, json!({ "name": "s", "type": "http", "status": 200 }));
        assert!(!kept.to_string().contains("s3cret"));
        assert_eq!(
            brief(&step("email", "steps.q.rows"), &json!({ "count": 2 })),
            json!({ "name": "s", "type": "email", "count": 2 })
        );
    }

    #[test]
    fn a_job_is_asked_what_it_does() {
        let steps = JobDef {
            kind: KIND.into(),
            config: json!({ "steps": [
                { "name": "q", "type": "query", "config": {} },
                { "name": "m", "type": "email", "config": { "template": "t1" } },
            ] }),
            name: "Reminders".into(),
            ..Default::default()
        };
        assert!(does(&steps, &["mail", "email"]));
        assert!(!does(&steps, &["sql"]));
        let plain = JobDef {
            kind: "sql".into(),
            ..Default::default()
        };
        assert!(does(&plain, &["sql"]));
        assert_eq!(
            using_template(&[plain, steps], "t1").as_deref(),
            Some("Reminders")
        );
    }

    #[test]
    fn steps_are_stored_with_their_own_keys() {
        let config = json!({ "steps": [
            { "name": "q", "type": "query", "config": { "collection": "orders", "extra": 1 } },
            { "name": "c", "type": "condition", "config": { "expression": " steps.q.count > 0 " } },
            { "name": "m", "type": "mail", "forEach": "steps.q.rows",
              "config": { "to": "{{ item.email }}", "subject": " Hi ", "body": "x" } },
        ] });
        let stored = normalized(&config);
        assert_eq!(
            stored["steps"][0]["config"],
            json!({ "collection": "orders", "filter": "", "sort": "", "limit": 100, "params": {} })
        );
        assert_eq!(
            stored["steps"][1]["config"],
            json!({ "expression": "steps.q.count > 0" })
        );
        assert_eq!(stored["steps"][2]["forEach"], "steps.q.rows");
        assert_eq!(stored["steps"][2]["config"]["to"], "{{ item.email }}");
        assert_eq!(stored["steps"][2]["config"]["subject"], "Hi");
        assert!(stored["steps"][0].get("forEach").is_none());
    }
}
