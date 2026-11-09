//! Cron jobs made in the dashboard (Settings → Cron jobs), as opposed to the ones code registers
//! with `cronAdd()` or `Hooks::cron`. A job is a schedule and a *type* with its settings; the
//! types are small, safe building blocks, so making one needs no hook file and no restart.
//!
//! - They live in the settings' private `cronJobs` section (like API keys): `/api/settings` never
//!   shows them, and changing one goes through `/api/crons` and notifies every instance.
//! - Each minute one instance runs a due job (the same claim as hook jobs), never two runs of the
//!   same job at once, with a time limit; the run and its result are in the jobs' history.
//! - Their ids start with `user:`, which hook jobs can't use, so the two can't be confused.
//! - Creating, changing and deleting are for admins and owners (the roles guard); running one
//!   now is for developers too, as for every job.
//!
//! Types today: `cleanup` (delete the records of a collection that match a filter and/or are
//! older than some days) and `http` (call a URL: a webhook, a health ping; through the guarded
//! request of `http.rs`, so never to this server or a private network). Header values are
//! secrets: the API shows them masked and keeps the saved value when the mask comes back.
//! `mail` sends an email (through the mail settings) and `sql` runs a statement under exactly
//! the SQL console's policy (console on, writes only when allowed, its role and limits); a job of
//! type `sql` can only be made or changed by an owner, since it can read and change anything.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::app::App;
use crate::auth::RequestAuth;
use crate::collections::model::CollectionKind;
use crate::core::Core;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::validation::Errors;

/// The prefix of every dashboard job's id.
pub const ID_PREFIX: &str = "user:";
const MAX_JOBS: usize = 50;
const TYPES: &[&str] = &["cleanup", "http", "mail", "sql"];
/// What the API shows instead of a saved header value.
pub const MASK: &str = "••••••••";
const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE"];
/// A run that takes longer is stopped and recorded as failed.
const RUN_LIMIT: Duration = Duration::from_secs(300);
const DEFAULT_MAX_ROWS: i64 = 10_000;
const MAX_RETRIES: i64 = 5;
const DEFAULT_RETRY_DELAY: i64 = 30;
const MAX_RETRY_WAIT: Duration = Duration::from_secs(300);

/// Who hears about a job's failures (and when it works again).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Notify {
    /// Up to 10 addresses, mailed through the mail settings.
    pub emails: Vec<String>,
    /// A public URL that gets a JSON `POST` (a secret, like a header value: shown masked).
    pub webhook: String,
}

impl Notify {
    fn is_empty(&self) -> bool {
        self.emails.is_empty() && self.webhook.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JobDef {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub expression: String,
    /// The type of job (`cleanup`).
    #[serde(rename = "type")]
    pub kind: String,
    pub config: Value,
    /// How many times a failed run is tried again (0 to 5).
    pub retries: i64,
    /// The wait before the first retry; each next one waits twice as long (at most 5 minutes).
    pub retry_delay_seconds: i64,
    pub notify: Notify,
    pub created: String,
    pub updated: String,
}

fn text<'a>(config: &'a Value, key: &str) -> &'a str {
    config.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn number(config: &Value, key: &str) -> Option<i64> {
    config.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

/// The filter a cleanup run deletes by, with its parameters: the user's filter and/or "older
/// than N days" on `created` or `updated`.
fn cleanup_filter(
    config: &Value,
    now: chrono::DateTime<chrono::Utc>,
) -> (String, Map<String, Value>) {
    let mut parts = Vec::new();
    let mut params = Map::new();
    let filter = text(config, "filter").trim();
    if !filter.is_empty() {
        parts.push(format!("({filter})"));
    }
    if let Some(days) = number(config, "olderThanDays") {
        let field = match text(config, "field") {
            "updated" => "updated",
            _ => "created",
        };
        let cutoff = now - chrono::Duration::days(days);
        params.insert(
            "cutoff".into(),
            Value::String(crate::datetime::format(&cutoff)),
        );
        parts.push(format!("{field} < {{:cutoff}}"));
    }
    (parts.join(" && "), params)
}

/// Checks a job as the dashboard sends it; `others` are the other jobs (names are unique).
pub async fn validate(core: &Arc<Core>, def: &JobDef, others: &[JobDef]) -> Errors {
    let mut errors = Errors::new();
    let name = def.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        errors.invalid("name", "Must be 1 to 60 characters.");
    } else if others
        .iter()
        .any(|o| o.id != def.id && o.name.trim().eq_ignore_ascii_case(name))
    {
        errors.invalid("name", "Another job has this name.");
    }
    if crate::cron::Schedule::parse(&def.expression).is_none() {
        errors.invalid(
            "expression",
            "Not a cron expression: minute hour day month weekday, e.g. 0 3 * * *.",
        );
    }
    if !(0..=MAX_RETRIES).contains(&def.retries) {
        errors.invalid("retries", "Between 0 and 5 retries.");
    }
    if def.retries > 0 && !(1..=300).contains(&def.retry_delay_seconds) {
        errors.invalid("retryDelaySeconds", "Between 1 and 300 seconds.");
    }
    validate_notify(core, &def.notify, &mut errors).await;
    if !TYPES.contains(&def.kind.as_str()) {
        errors.invalid("type", "Choose a type of job.");
        return errors;
    }
    if def.kind == "cleanup" {
        validate_cleanup(core, &def.config, &mut errors).await;
    } else if def.kind == "http" {
        validate_http(&def.config, &mut errors).await;
    } else if def.kind == "mail" {
        validate_mail(core, &def.config, &mut errors);
    } else if def.kind == "sql" {
        validate_sql(core, &def.config, &mut errors);
    }
    errors
}

/// The addresses of a `mail` job (a list, or one text with commas, spaces or lines between).
fn recipients(config: &Value) -> Vec<String> {
    let raw: Vec<String> = match config.get("to") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => vec![],
    };
    raw.iter()
        .flat_map(|s| s.split([',', ';', '\n', ' ']))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn plain_line(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

/// One to 10 plausible addresses, none with a control character (no header injection).
fn check_addresses(to: &[String]) -> Result<(), String> {
    if to.is_empty() || to.len() > 10 {
        return Err("One to 10 email addresses.".into());
    }
    match to.iter().find(|a| {
        a.len() > 254
            || !plain_line(a)
            || a.matches('@').count() != 1
            || a.starts_with('@')
            || a.ends_with('@')
            || !a.rsplit('@').next().is_some_and(|d| d.contains('.'))
    }) {
        Some(bad) => Err(format!("{bad:?} is not an email address.")),
        None => Ok(()),
    }
}

async fn validate_notify(core: &Arc<Core>, notify: &Notify, errors: &mut Errors) {
    if !notify.emails.is_empty() {
        if let Err(message) = check_addresses(&notify.emails) {
            errors.invalid("notify.emails", message);
        } else if !core.settings().smtp.enabled {
            errors.invalid(
                "notify.emails",
                "Email isn't set up: turn on SMTP in Settings → Mail first.",
            );
        }
    }
    if !notify.webhook.is_empty() {
        if let Err(message) = crate::http::check_target(&notify.webhook).await {
            errors.invalid("notify.webhook", message);
        }
    }
}

fn validate_mail(core: &Arc<Core>, config: &Value, errors: &mut Errors) {
    if let Err(message) = check_addresses(&recipients(config)) {
        errors.invalid("config.to", message);
    }
    let subject = text(config, "subject").trim();
    if subject.is_empty() || subject.chars().count() > 200 || !plain_line(subject) {
        errors.invalid("config.subject", "1 to 200 characters, on one line.");
    }
    if text(config, "body").chars().count() > 20_000 {
        errors.invalid("config.body", "At most 20 000 characters.");
    }
    if !core.settings().smtp.enabled {
        errors.invalid(
            "config.to",
            "Email isn't set up: turn on SMTP in Settings → Mail first.",
        );
    }
}

fn validate_sql(core: &Arc<Core>, config: &Value, errors: &mut Errors) {
    let statement = text(config, "statement").trim();
    if !core.sql_console {
        errors.invalid(
            "config.statement",
            "SQL jobs need the SQL console: start slonbase with --sql-console.",
        );
    } else if statement.is_empty()
        || statement.chars().count() > crate::sql_console::MAX_QUERY_LENGTH
    {
        errors.invalid("config.statement", "1 to 20 000 characters.");
    } else if !core.sql_console_writes
        && !crate::sql_console::READ_KEYWORDS
            .contains(&crate::sql_console::first_keyword(statement).as_str())
    {
        errors.invalid(
            "config.statement",
            "The SQL console is read-only, so is this job: start slonbase with --sql-console-writes to change data.",
        );
    }
}

/// The headers of an `http` job as (name, value) pairs.
fn headers_of(config: &Value) -> Vec<(String, String)> {
    config
        .get("headers")
        .and_then(Value::as_object)
        .map(|h| {
            h.iter()
                .map(|(k, v)| {
                    (
                        k.trim().to_string(),
                        v.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn validate_http(config: &Value, errors: &mut Errors) {
    match crate::http::check_target(text(config, "url").trim()).await {
        Ok(()) => {}
        Err(message) => errors.invalid("config.url", &message),
    }
    let method = match text(config, "method") {
        "" => "GET",
        m => m,
    };
    if !METHODS.contains(&method) {
        errors.invalid("config.method", "Use GET, POST, PUT, PATCH or DELETE.");
    }
    if let Some(h) = config.get("headers")
        && !h.is_object()
    {
        errors.invalid("config.headers", "Headers are name and value pairs.");
    }
    let headers = headers_of(config);
    if headers.len() > 20 {
        errors.invalid("config.headers", "At most 20 headers.");
    }
    for (name, value) in &headers {
        let lower = name.to_ascii_lowercase();
        if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err()
            || reqwest::header::HeaderValue::from_str(value).is_err()
            || value.len() > 2000
        {
            errors.invalid("config.headers", format!("{name:?} is not a valid header."));
        } else if ["host", "content-length", "transfer-encoding", "connection"]
            .contains(&lower.as_str())
        {
            errors.invalid("config.headers", format!("{name} is set by the server."));
        }
    }
    if text(config, "body").len() > 20_000 {
        errors.invalid("config.body", "At most 20 000 characters.");
    }
    if let Some(t) = number(config, "timeoutSeconds")
        && !(1..=60).contains(&t)
    {
        errors.invalid("config.timeoutSeconds", "Between 1 and 60 seconds.");
    }
    if let Some(s) = number(config, "expectStatus")
        && !(100..=599).contains(&s)
    {
        errors.invalid("config.expectStatus", "A status code such as 200.");
    }
}

async fn validate_cleanup(core: &Arc<Core>, config: &Value, errors: &mut Errors) {
    let name = text(config, "collection");
    let Ok(collection) = core.collection(name) else {
        errors.invalid("config.collection", "Choose a collection.");
        return;
    };
    if collection.system || collection.name.starts_with('_') {
        errors.invalid(
            "config.collection",
            "System collections can't be cleaned up.",
        );
        return;
    }
    if collection.kind == CollectionKind::View {
        errors.invalid("config.collection", "A view is read-only.");
        return;
    }
    let has_age = config.get("olderThanDays").is_some_and(|v| !v.is_null());
    let field = match text(config, "field") {
        "" => "created",
        f => f,
    };
    // the age is read from a date field the collection must have (created and updated are
    // fields to add, as in PocketBase 0.23 and later)
    if !["created", "updated"].contains(&field) {
        errors.invalid("config.field", "Use created or updated.");
    } else if has_age && !collection.fields.iter().any(|f| f.name == field) {
        errors.invalid(
            "config.field",
            format!("The collection has no \"{field}\" field to read the age from."),
        );
    }
    if config.get("olderThanDays").is_some_and(|v| !v.is_null())
        && !matches!(number(config, "olderThanDays"), Some(1..=36_500))
    {
        errors.invalid("config.olderThanDays", "Must be between 1 and 36500 days.");
    }
    if text(config, "filter").chars().count() > 2000 {
        errors.invalid("config.filter", "At most 2000 characters.");
    }
    if config.get("maxRows").is_some_and(|v| !v.is_null())
        && !matches!(number(config, "maxRows"), Some(1..=100_000))
    {
        errors.invalid("config.maxRows", "Must be between 1 and 100000.");
    }
    if text(config, "filter").trim().is_empty() && !has_age {
        errors.invalid(
            "config.filter",
            "Give a filter or an age, so a run can't delete every record.",
        );
    }
    if !errors.is_empty() {
        return;
    }
    // the filter must compile against the collection
    let (filter, params) = cleanup_filter(config, chrono::Utc::now());
    if let Err(err) = core.count_records(name, &filter, &params).await {
        errors.invalid("config.filter", err.message);
    }
}

fn failure(err: crate::error::ApiError) -> String {
    err.details.as_deref().unwrap_or(&err.message).to_string()
}

/// Why an attempt failed, and whether trying again could help (a missing setting or a refused
/// address won't fix itself; a timeout or a busy database might).
#[derive(Debug)]
pub(crate) struct Fail {
    pub message: String,
    pub retry: bool,
}

impl From<String> for Fail {
    fn from(message: String) -> Self {
        Self {
            message,
            retry: true,
        }
    }
}

impl Fail {
    fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry: false,
        }
    }
}

/// One attempt at a job's work.
async fn attempt(core: &Arc<Core>, def: &JobDef) -> Result<Value, Fail> {
    match def.kind.as_str() {
        "cleanup" => cleanup(core, &def.config).await.map_err(Fail::from),
        "http" => call_url(&def.config).await,
        "mail" => send_mail(core, &def.config).await,
        "sql" => run_sql(core, &def.config).await,
        other => Err(Fail::fatal(format!("Unknown type of job: {other}."))),
    }
}

/// Runs a job's work once and returns what to record about it.
pub async fn execute(core: &Arc<Core>, def: &JobDef) -> Result<Value, String> {
    attempt(core, def).await.map_err(|f| f.message)
}

/// The wait before retry number `n` (1 is the first): the job's delay, doubled each time.
fn backoff(delay_seconds: i64, n: u32) -> Duration {
    let first = Duration::from_secs(delay_seconds.max(1) as u64);
    first
        .saturating_mul(1u32 << (n - 1).min(16))
        .min(MAX_RETRY_WAIT)
}

/// Tries `work` up to `1 + retries` times, waiting `wait(n)` before retry `n` and stopping at a
/// failure that retrying can't fix. Returns the outcome and how many attempts were made.
async fn with_retries<F, Fut>(
    retries: u32,
    wait: impl Fn(u32) -> Duration,
    mut work: F,
) -> (Result<Value, String>, u32)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Value, Fail>>,
{
    let mut tries = 0;
    loop {
        tries += 1;
        match work().await {
            Ok(mut meta) => {
                if tries > 1 {
                    meta["attempts"] = tries.into();
                }
                return (Ok(meta), tries);
            }
            Err(fail) if fail.retry && tries <= retries => tokio::time::sleep(wait(tries)).await,
            Err(fail) => {
                let message = if tries > 1 {
                    format!("{} (after {tries} attempts)", fail.message)
                } else {
                    fail.message
                };
                return (Err(message), tries);
            }
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

async fn send_mail(core: &Arc<Core>, config: &Value) -> Result<Value, Fail> {
    if !core.settings().smtp.enabled {
        return Err(Fail::fatal(
            "Email isn't set up: turn on SMTP in Settings → Mail.",
        ));
    }
    let to = recipients(config);
    let html = escape(text(config, "body")).replace('\n', "<br>");
    let mut sent = 0;
    for address in &to {
        core.send_mail_now(crate::mail::Message {
            to: address.clone(),
            subject: text(config, "subject").trim().to_string(),
            html: html.clone(),
        })
        .await
        .map_err(|e| Fail::from(format!("Sending to {address} failed: {e}")))?;
        sent += 1;
    }
    Ok(json!({ "sent": sent }))
}

async fn run_sql(core: &Arc<Core>, config: &Value) -> Result<Value, Fail> {
    let result = crate::sql_console::execute(core, text(config, "statement"), 100)
        .await
        .map_err(|e| Fail {
            // the console is off or read-only for this statement: that won't change by itself
            retry: e.status != StatusCode::FORBIDDEN,
            message: failure(e),
        })?;
    // what it did, not what it read: rows may hold anything
    Ok(json!({
        "affectedRows": result["affectedRows"],
        "rows": result["rows"].as_array().map_or(0, Vec::len),
        "readOnly": result["readOnly"],
    }))
}

async fn call_url(config: &Value) -> Result<Value, Fail> {
    let method = match text(config, "method") {
        "" => "GET",
        m => m,
    };
    let body = Some(text(config, "body")).filter(|b| !b.is_empty());
    let timeout =
        Duration::from_secs(number(config, "timeoutSeconds").unwrap_or(30).clamp(1, 60) as u64);
    let reply = crate::http::guarded(
        method,
        text(config, "url").trim(),
        &headers_of(config),
        body,
        timeout,
    )
    .await
    .map_err(|message| Fail {
        retry: !crate::http::refused(&message),
        message,
    })?;
    let ok = match number(config, "expectStatus") {
        Some(want) => i64::from(reply.status) == want,
        None => (200..300).contains(&reply.status),
    };
    if !ok {
        return Err(Fail::from(format!("The server answered {}.", reply.status)));
    }
    Ok(json!({ "status": reply.status, "response": reply.excerpt }))
}

async fn cleanup(core: &Arc<Core>, config: &Value) -> Result<Value, String> {
    let collection = text(config, "collection");
    let (filter, params) = cleanup_filter(config, chrono::Utc::now());
    let max = number(config, "maxRows").unwrap_or(DEFAULT_MAX_ROWS).max(1);
    let mut deleted = 0i64;
    while deleted < max {
        let batch = (max - deleted).min(200);
        let records = core
            .find_records_by_filter(collection, &filter, "", batch, 0, &params)
            .await
            .map_err(failure)?;
        if records.is_empty() {
            break;
        }
        let before = deleted;
        for record in records {
            let Some(id) = record.get("id").and_then(Value::as_str) else {
                continue;
            };
            core.delete_record(collection, id).await.map_err(failure)?;
            deleted += 1;
        }
        if deleted == before {
            break;
        }
    }
    Ok(json!({ "collection": collection, "deleted": deleted, "limit": max }))
}

/// Whether the job's last run is still going (started within the hour), so a slow run isn't
/// started again on top of itself.
async fn still_running(core: &Core, id: &str) -> bool {
    let check = async {
        let client = core
            .pool
            .get()
            .await
            .map_err(crate::db::describe_pool_error)?;
        let row = client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM _slonbase.job_runs
                 WHERE kind = $1 AND job = $2 AND status = 'running'
                 AND started > now() - interval '1 hour')",
                &[&crate::jobs::CRON, &id],
            )
            .await?;
        anyhow::Ok(row.get::<_, bool>(0))
    };
    check.await.unwrap_or(false)
}

/// Runs a job, recorded in the jobs' history (`trigger`: `schedule` or `manual`).
pub async fn run_recorded(core: Arc<Core>, def: JobDef, trigger: &'static str) {
    if still_running(&core, &def.id).await {
        tracing::warn!(job = %def.id, "a cron job is still running: not started again");
        return;
    }
    // how the run before this one ended, to tell the first failure of a streak from the rest
    let before = match def.notify.is_empty() {
        true => None,
        false => last_finished(&core, &def.id).await,
    };
    let record = crate::jobs::start(&core, crate::jobs::CRON, &def.id, trigger).await;
    let retries = def.retries.clamp(0, MAX_RETRIES) as u32;
    let delay = match def.retry_delay_seconds {
        0 => DEFAULT_RETRY_DELAY,
        d => d,
    };
    let (result, tries) = with_retries(
        retries,
        |n| backoff(delay, n),
        || async {
            match tokio::time::timeout(RUN_LIMIT, attempt(&core, &def)).await {
                Ok(result) => result,
                Err(_) => Err(Fail::from(format!(
                    "Stopped after {} minutes.",
                    RUN_LIMIT.as_secs() / 60
                ))),
            }
        },
    )
    .await;
    if let Err(detail) = &result {
        tracing::error!(job = %def.id, error = %detail, "a cron job failed");
    }
    let event = (trigger == "schedule")
        .then(|| notice(before.as_deref(), result.is_ok()))
        .flatten();
    let error = result.as_ref().err().cloned().unwrap_or_default();
    record.finish(result).await;
    if let Some(event) = event {
        send_notices(&core, &def, event, &error, tries).await;
    }
}

/// What a job's last finished run came to: `ok`, `error` or nothing yet.
async fn last_finished(core: &Core, id: &str) -> Option<String> {
    let client = core.pool.get().await.ok()?;
    client
        .query_opt(
            "SELECT status FROM _slonbase.job_runs
             WHERE kind = $1 AND job = $2 AND status IN ('ok', 'error')
             ORDER BY started DESC, id DESC LIMIT 1",
            &[&crate::jobs::CRON, &id],
        )
        .await
        .ok()
        .flatten()
        .map(|row| row.get::<_, String>(0))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Notice {
    Failed,
    Recovered,
}

/// When to tell: the first failure after a success (or the first run ever), and the first
/// success after a failure; a job that keeps failing doesn't send a message a minute.
fn notice(before: Option<&str>, ok: bool) -> Option<Notice> {
    match (before, ok) {
        (Some("error"), true) => Some(Notice::Recovered),
        (Some("error"), false) => None,
        (_, false) => Some(Notice::Failed),
        (_, true) => None,
    }
}

/// The subject and the text of the email.
fn notice_text(def: &JobDef, event: Notice, error: &str, tries: u32, at: &str) -> (String, String) {
    let (subject, line) = match event {
        Notice::Failed => (
            format!("Cron job failed: {}", def.name),
            format!("The job \"{}\" failed.", def.name),
        ),
        Notice::Recovered => (
            format!("Cron job recovered: {}", def.name),
            format!(
                "The job \"{}\" ran successfully again after failing.",
                def.name
            ),
        ),
    };
    let mut body = format!("{line}\nWhen: {at}\n");
    if event == Notice::Failed {
        body.push_str(&format!("Tries: {tries}\nError: {error}\n"));
    }
    (subject, body)
}

/// The JSON the webhook gets.
fn notice_json(def: &JobDef, event: Notice, error: &str, tries: u32, at: &str) -> Value {
    json!({
        "event": match event { Notice::Failed => "cron.failed", Notice::Recovered => "cron.recovered" },
        "job": { "id": def.id, "name": def.name },
        "error": if event == Notice::Failed { error } else { "" },
        "attempts": tries,
        "at": at,
    })
}

/// Tells the job's people. A notice that can't be sent is logged and changes nothing about the
/// run: it is never retried and never makes the job fail.
async fn send_notices(core: &Arc<Core>, def: &JobDef, event: Notice, error: &str, tries: u32) {
    let at = crate::datetime::format(&chrono::Utc::now());
    if !def.notify.emails.is_empty() {
        if core.settings().smtp.enabled {
            let (subject, body) = notice_text(def, event, error, tries, &at);
            let html = escape(&body).replace('\n', "<br>");
            for address in &def.notify.emails {
                let sent = core
                    .send_mail_now(crate::mail::Message {
                        to: address.clone(),
                        subject: subject.clone(),
                        html: html.clone(),
                    })
                    .await;
                if let Err(err) = sent {
                    tracing::warn!(job = %def.id, error = %format!("{err:#}"), "failed to mail a cron job's notice");
                }
            }
        } else {
            tracing::warn!(job = %def.id, "email is off: a cron job's notice was not sent");
        }
    }
    if !def.notify.webhook.is_empty() {
        let body = notice_json(def, event, error, tries, &at).to_string();
        let sent = crate::http::guarded(
            "POST",
            &def.notify.webhook,
            &[("content-type".to_string(), "application/json".to_string())],
            Some(&body),
            Duration::from_secs(10),
        )
        .await;
        match sent {
            Ok(reply) if !(200..300).contains(&reply.status) => {
                tracing::warn!(job = %def.id, status = reply.status, "a cron job's webhook answered with an error");
            }
            Err(err) => {
                tracing::warn!(job = %def.id, error = %err, "failed to call a cron job's webhook");
            }
            Ok(_) => {}
        }
    }
}

/// Runs the dashboard jobs due this minute; one instance runs each (claimed like hook jobs).
pub(crate) async fn run_scheduled(
    core: &Arc<Core>,
    now: chrono::DateTime<chrono::Utc>,
    minute: &str,
    tz: chrono_tz::Tz,
) {
    let settings = core.settings();
    for def in settings.cron_jobs.iter().filter(|d| d.enabled) {
        let due =
            crate::cron::Schedule::parse(&def.expression).is_some_and(|s| s.matches(&now, tz));
        if !due {
            continue;
        }
        match crate::hooks::claim_minute(core, &def.id, minute).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(err) => {
                tracing::warn!(job = %def.id, error = %format!("{err:#}"), "failed to schedule a cron job");
                continue;
            }
        }
        tokio::spawn(run_recorded(core.clone(), def.clone(), "schedule"));
    }
}

/// How the jobs list shows a dashboard job (the fields of a hook job, and its settings).
pub(crate) fn listed(
    def: &JobDef,
    tz: chrono_tz::Tz,
    now: chrono::DateTime<chrono::Utc>,
    last: Option<Value>,
) -> Value {
    let next = crate::cron::Schedule::parse(&def.expression)
        .filter(|_| def.enabled)
        .and_then(|s| s.next_after(&now, tz))
        .map(|t| crate::datetime::format(&t));
    json!({
        "id": def.id,
        "name": def.name,
        "enabled": def.enabled,
        "expression": def.expression,
        "description": crate::cron::Schedule::describe(&def.expression),
        "timezone": tz.name(),
        "next": next,
        "kind": "user",
        "type": def.kind,
        "config": masked(def),
        "notify": {
            "emails": def.notify.emails,
            "webhook": if def.notify.webhook.is_empty() { "" } else { MASK },
        },
        "retries": def.retries,
        "retryDelaySeconds": if def.retries > 0 { def.retry_delay_seconds.max(1) } else { DEFAULT_RETRY_DELAY },
        "lastRun": last,
    })
}

/// The config as the API shows it: header values hidden.
fn masked(def: &JobDef) -> Value {
    let mut config = def.config.clone();
    if def.kind == "http"
        && let Some(h) = config.get_mut("headers").and_then(Value::as_object_mut)
    {
        for v in h.values_mut() {
            *v = json!(MASK);
        }
    }
    config
}

/// A header (or the webhook) sent back as the mask keeps its saved value.
fn unmask(def: &mut JobDef, saved: &JobDef) {
    if def.notify.webhook == MASK {
        def.notify.webhook = saved.notify.webhook.clone();
    }
    if def.kind != "http" {
        return;
    }
    let was = headers_of(&saved.config);
    if let Some(h) = def.config.get_mut("headers").and_then(Value::as_object_mut) {
        for (name, value) in h.iter_mut() {
            if value.as_str() == Some(MASK) {
                *value = json!(
                    was.iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                        .map(|(_, v)| v.as_str())
                        .unwrap_or_default()
                );
            }
        }
    }
}

fn read(body: &Map<String, Value>, base: JobDef) -> JobDef {
    let mut def = base;
    if let Some(v) = body.get("name").and_then(Value::as_str) {
        def.name = v.trim().to_string();
    }
    if let Some(v) = body.get("expression").and_then(Value::as_str) {
        def.expression = v.trim().to_string();
    }
    if let Some(v) = body.get("type").and_then(Value::as_str) {
        def.kind = v.to_string();
    }
    if let Some(v) = body.get("enabled").and_then(Value::as_bool) {
        def.enabled = v;
    }
    if let Some(v) = body.get("config").filter(|v| v.is_object()) {
        def.config = v.clone();
    }
    if let Some(v) = body.get("retries").and_then(whole) {
        def.retries = v;
    }
    if let Some(v) = body.get("retryDelaySeconds").and_then(whole) {
        def.retry_delay_seconds = v;
    }
    if let Some(n) = body.get("notify").and_then(Value::as_object) {
        def.notify = Notify {
            emails: recipients(&json!({ "to": n.get("emails").cloned().unwrap_or_default() })),
            webhook: n
                .get("webhook")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string(),
        };
    }
    def
}

/// A whole number sent as a number or as text.
fn whole(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Jobs of type `sql` can read and change anything the console can: owners only.
fn owner_only(core: &Core, auth: &RequestAuth) -> Result<(), ApiError> {
    crate::roles::require_role(core, auth, crate::roles::Role::Owner)
        .map(|_| ())
        .map_err(|_| ApiError::forbidden("Only owners can make or change SQL jobs."))
}

fn invalid(errors: &Errors) -> ApiError {
    ApiError::invalid_data(
        "An error occurred while validating the submitted data.",
        errors,
    )
}

/// `POST /api/crons`: makes a job.
pub async fn create(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let mut settings = (*core.settings()).clone();
    let now = crate::datetime::format(&chrono::Utc::now());
    let mut def = read(
        &body,
        JobDef {
            id: format!("{ID_PREFIX}{}", crate::ids::random_id(10)),
            enabled: true,
            config: json!({}),
            created: now.clone(),
            updated: now,
            ..Default::default()
        },
    );
    if def.kind == "sql" {
        owner_only(&core, &auth)?;
    }
    let mut errors = validate(&core, &def, &settings.cron_jobs).await;
    if settings.cron_jobs.len() >= MAX_JOBS {
        errors.invalid("name", "An installation has at most 50 jobs made here.");
    }
    if !errors.is_empty() {
        return Err(invalid(&errors));
    }
    def.config = normalized(&def);
    settings.cron_jobs.push(def.clone());
    crate::roles::save_settings(&core, settings).await?;
    Ok(Json(listed(
        &def,
        core.settings().timezone(),
        chrono::Utc::now(),
        None,
    )))
}

/// The config as stored: only the keys the type uses, with defaults filled in.
fn normalized(def: &JobDef) -> Value {
    let c = &def.config;
    match def.kind.as_str() {
        "cleanup" => {
            let mut out = json!({
                "collection": text(c, "collection"),
                "field": match text(c, "field") { "" => "created", f => f },
                "filter": text(c, "filter").trim(),
                "maxRows": number(c, "maxRows").unwrap_or(DEFAULT_MAX_ROWS),
            });
            if let Some(days) = number(c, "olderThanDays") {
                out["olderThanDays"] = days.into();
            }
            out
        }
        "http" => {
            let mut out = json!({
                "url": text(c, "url").trim(),
                "method": match text(c, "method") { "" => "GET", m => m },
                "headers": headers_of(c).into_iter().map(|(k, v)| (k, json!(v))).collect::<Map<_, _>>(),
                "body": text(c, "body"),
                "timeoutSeconds": number(c, "timeoutSeconds").unwrap_or(30),
            });
            if let Some(s) = number(c, "expectStatus") {
                out["expectStatus"] = s.into();
            }
            out
        }
        "mail" => json!({
            "to": recipients(c),
            "subject": text(c, "subject").trim(),
            "body": text(c, "body"),
        }),
        "sql" => json!({ "statement": text(c, "statement").trim() }),
        _ => c.clone(),
    }
}

/// `PATCH /api/crons/{id}`: changes a job.
pub async fn update(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let mut settings = (*core.settings()).clone();
    let Some(pos) = settings.cron_jobs.iter().position(|j| j.id == id) else {
        return Err(ApiError::not_found());
    };
    let saved = settings.cron_jobs[pos].clone();
    let mut def = read(&body, saved.clone());
    unmask(&mut def, &saved);
    if saved.kind == "sql" || def.kind == "sql" {
        owner_only(&core, &auth)?;
    }
    def.id = id;
    let errors = validate(&core, &def, &settings.cron_jobs).await;
    if !errors.is_empty() {
        return Err(invalid(&errors));
    }
    def.config = normalized(&def);
    def.updated = crate::datetime::format(&chrono::Utc::now());
    settings.cron_jobs[pos] = def.clone();
    crate::roles::save_settings(&core, settings).await?;
    Ok(Json(listed(
        &def,
        core.settings().timezone(),
        chrono::Utc::now(),
        None,
    )))
}

/// `DELETE /api/crons/{id}`: deletes a job (its history stays until it ages out).
pub async fn delete(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    auth: RequestAuth,
) -> Result<StatusCode, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let mut settings = (*core.settings()).clone();
    let before = settings.cron_jobs.len();
    settings.cron_jobs.retain(|j| j.id != id);
    if settings.cron_jobs.len() == before {
        return Err(ApiError::not_found());
    }
    crate::roles::save_settings(&core, settings).await?;
    // the claim of its last minute has no use now
    if let Ok(client) = core.pool.get().await {
        let _ = client
            .execute(
                "DELETE FROM _slonbase.params WHERE key = $1",
                &[&format!("cron_last:{id}")],
            )
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/cron-preview`: checks a job as it is being made (nothing is saved or run): its
/// schedule in words and the next runs, and for a cleanup how many records match now.
pub async fn preview(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let mut def = read(
        &body,
        JobDef {
            id: body
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            enabled: true,
            config: json!({}),
            ..Default::default()
        },
    );
    let others = core.settings().cron_jobs.clone();
    if let Some(saved) = others.iter().find(|j| j.id == def.id) {
        unmask(&mut def, saved);
    }
    let errors = validate(&core, &def, &others).await;
    if !errors.is_empty() {
        return Err(invalid(&errors));
    }
    let tz = core.settings().timezone();
    let schedule = crate::cron::Schedule::parse(&def.expression).expect("validated");
    let mut next = Vec::new();
    let mut after = chrono::Utc::now();
    while next.len() < 5 {
        let Some(t) = schedule.next_after(&after, tz) else {
            break;
        };
        next.push(crate::datetime::format(&t));
        after = t;
    }
    let mut answer = json!({
        "description": crate::cron::Schedule::describe(&def.expression),
        "timezone": tz.name(),
        "next": next,
    });
    if def.kind == "cleanup" {
        let (filter, params) = cleanup_filter(&def.config, chrono::Utc::now());
        let matching = core
            .count_records(text(&def.config, "collection"), &filter, &params)
            .await
            .map_err(|e| ApiError::bad_request(e.message))?;
        answer["matching"] = matching.into();
    }
    Ok(Json(answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_failure_of_a_streak_and_the_recovery_are_told() {
        assert_eq!(
            notice(None, false),
            Some(Notice::Failed),
            "the first run ever"
        );
        assert_eq!(notice(Some("ok"), false), Some(Notice::Failed));
        assert_eq!(notice(Some("error"), false), None, "still failing: quiet");
        assert_eq!(notice(Some("error"), true), Some(Notice::Recovered));
        assert_eq!(notice(Some("ok"), true), None);
        assert_eq!(notice(None, true), None);
    }

    #[test]
    fn the_notices_say_what_happened() {
        let def = JobDef {
            id: "user:abc".into(),
            name: "Nightly sync".into(),
            ..Default::default()
        };
        let (subject, body) = notice_text(
            &def,
            Notice::Failed,
            "The server answered 500.",
            3,
            "2026-11-09 03:00:00.000Z",
        );
        assert_eq!(subject, "Cron job failed: Nightly sync");
        assert!(body.contains("The server answered 500.") && body.contains("Tries: 3"));
        let (subject, body) = notice_text(&def, Notice::Recovered, "", 1, "x");
        assert_eq!(subject, "Cron job recovered: Nightly sync");
        assert!(!body.contains("Error"));

        let json = notice_json(&def, Notice::Failed, "boom", 2, "t");
        assert_eq!(json["event"], "cron.failed");
        assert_eq!(json["job"]["id"], "user:abc");
        assert_eq!(
            (json["error"].as_str(), json["attempts"].as_i64()),
            (Some("boom"), Some(2))
        );
        assert_eq!(
            notice_json(&def, Notice::Recovered, "old", 1, "t")["event"],
            "cron.recovered"
        );
    }

    use std::sync::atomic::{AtomicU32, Ordering};

    fn quick(_: u32) -> Duration {
        Duration::from_millis(1)
    }

    #[tokio::test]
    async fn a_failing_job_is_tried_again_and_the_attempts_are_counted() {
        let calls = AtomicU32::new(0);
        let (result, tries) = with_retries(3, quick, || async {
            if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                Err(Fail::from("busy".to_string()))
            } else {
                Ok(json!({ "sent": 1 }))
            }
        })
        .await;
        assert_eq!(tries, 3);
        let meta = result.unwrap();
        assert_eq!(meta["attempts"], 3);
        assert_eq!(meta["sent"], 1);

        // no retries configured: one attempt, no `attempts` in the result
        let (result, tries) = with_retries(0, quick, || async { Ok(json!({})) }).await;
        assert_eq!(tries, 1);
        assert!(result.unwrap().get("attempts").is_none());
    }

    #[tokio::test]
    async fn retries_run_out_and_a_failure_that_cant_change_is_not_retried() {
        let calls = AtomicU32::new(0);
        let (result, tries) = with_retries(2, quick, || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(Fail::from("down".to_string()))
        })
        .await;
        assert_eq!((tries, calls.load(Ordering::SeqCst)), (3, 3));
        assert_eq!(result.unwrap_err(), "down (after 3 attempts)");

        let calls = AtomicU32::new(0);
        let (result, tries) = with_retries(5, quick, || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(Fail::fatal("Email isn't set up."))
        })
        .await;
        assert_eq!((tries, calls.load(Ordering::SeqCst)), (1, 1));
        assert_eq!(result.unwrap_err(), "Email isn't set up.");
    }

    #[test]
    fn the_wait_doubles_and_stops_at_five_minutes() {
        let secs = |n| backoff(30, n).as_secs();
        assert_eq!([secs(1), secs(2), secs(3), secs(4)], [30, 60, 120, 240]);
        assert_eq!([secs(5), secs(6), secs(40)], [300, 300, 300]);
        assert_eq!(backoff(0, 1).as_secs(), 1);
    }

    #[test]
    fn the_cleanup_filter_combines_the_filter_and_the_age() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let (f, p) = cleanup_filter(&json!({ "filter": "done = true" }), now);
        assert_eq!(f, "(done = true)");
        assert!(p.is_empty());
        let (f, p) = cleanup_filter(&json!({ "olderThanDays": 30 }), now);
        assert_eq!(f, "created < {:cutoff}");
        assert_eq!(p["cutoff"], "2026-10-11 12:00:00.000Z");
        let (f, _) = cleanup_filter(
            &json!({ "filter": "a = 1", "olderThanDays": "7", "field": "updated" }),
            now,
        );
        assert_eq!(f, "(a = 1) && updated < {:cutoff}");
        // nothing to go by: nothing is deleted by a filter that matches everything (it is
        // refused when the job is saved)
        assert_eq!(cleanup_filter(&json!({}), now).0, "");
    }

    #[test]
    fn numbers_come_as_numbers_or_text() {
        assert_eq!(number(&json!({ "n": 5 }), "n"), Some(5));
        assert_eq!(number(&json!({ "n": " 7 " }), "n"), Some(7));
        assert_eq!(number(&json!({ "n": "x" }), "n"), None);
        assert_eq!(number(&json!({}), "n"), None);
    }
}
