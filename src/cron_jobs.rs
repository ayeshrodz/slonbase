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
use crate::roles::Role;
use crate::services::access::{self, Access};
use crate::services::{self, Failure, mail, outbound};
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
    // a name ends up in the subject of notification emails: one plain line
    if name.chars().any(char::is_control) {
        errors.invalid(
            "name",
            "Must not contain line breaks or other control characters.",
        );
    }
    if crate::cron::Schedule::parse(&def.expression).is_none() {
        errors.invalid(
            "expression",
            "Not a cron expression: minute hour day month weekday, e.g. 0 3 * * *.",
        );
    } else if def.kind == "mail"
        && min_gap_minutes(&def.expression, core.settings().timezone())
            .is_some_and(|gap| gap < MIN_MAIL_GAP_MINUTES)
    {
        errors.invalid(
            "expression",
            format!("An email job can run at most every {MIN_MAIL_GAP_MINUTES} minutes."),
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

/// An email job runs at most this often: a schedule typed wrongly mustn't mail people every
/// minute, and the mail settings' sender reputation is shared by everything that sends.
const MIN_MAIL_GAP_MINUTES: i64 = 5;

/// The shortest time between two of the next runs of a schedule, in minutes.
fn min_gap_minutes(expression: &str, tz: chrono_tz::Tz) -> Option<i64> {
    let schedule = crate::cron::Schedule::parse(expression)?;
    let mut after = chrono::Utc::now();
    let mut previous: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut shortest: Option<i64> = None;
    for _ in 0..24 {
        let next = schedule.next_after(&after, tz)?;
        if let Some(p) = previous {
            let gap = (next - p).num_minutes();
            shortest = Some(shortest.map_or(gap, |s| s.min(gap)));
        }
        previous = Some(next);
        after = next;
    }
    shortest
}

async fn validate_notify(core: &Arc<Core>, notify: &Notify, errors: &mut Errors) {
    if !notify.emails.is_empty() {
        if let Err(message) = mail::check_addresses(&notify.emails) {
            errors.invalid("notify.emails", message);
        } else if !mail::enabled(core) {
            errors.invalid("notify.emails", mail::DISABLED);
        }
    }
    if !notify.webhook.is_empty() {
        if let Err(message) = outbound::check(&notify.webhook).await {
            errors.invalid("notify.webhook", message);
        }
    }
}

fn validate_mail(core: &Arc<Core>, config: &Value, errors: &mut Errors) {
    if let Err(message) = mail::check_addresses(&mail::recipients(config.get("to"))) {
        errors.invalid("config.to", message);
    }
    let subject = text(config, "subject").trim();
    if subject.is_empty() || subject.chars().count() > 200 || !mail::plain_line(subject) {
        errors.invalid("config.subject", "1 to 200 characters, on one line.");
    }
    if text(config, "body").chars().count() > 20_000 {
        errors.invalid("config.body", "At most 20 000 characters.");
    }
    if !mail::enabled(core) {
        errors.invalid("config.to", mail::DISABLED);
    }
}

fn validate_sql(core: &Arc<Core>, config: &Value, errors: &mut Errors) {
    if let Err(message) = services::sql::check(core, text(config, "statement")) {
        errors.invalid("config.statement", message);
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
    match outbound::check(text(config, "url").trim()).await {
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

/// One attempt at a job's work.
async fn attempt(core: &Arc<Core>, def: &JobDef) -> Result<Value, Failure> {
    match def.kind.as_str() {
        "cleanup" => cleanup(core, &def.config).await.map_err(Failure::transient),
        "http" => call_url(&def.config).await,
        "mail" => send_mail(core, &def.config).await,
        "sql" => run_sql(core, &def.config).await,
        other => Err(Failure::permanent(format!("Unknown type of job: {other}."))),
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

async fn send_mail(core: &Arc<Core>, config: &Value) -> Result<Value, Failure> {
    let sent = mail::send_text(
        core,
        &mail::recipients(config.get("to")),
        text(config, "subject").trim(),
        text(config, "body"),
    )
    .await?;
    Ok(json!({ "sent": sent }))
}

async fn run_sql(core: &Arc<Core>, config: &Value) -> Result<Value, Failure> {
    // (a scheduled job: an owner set it up, the server runs it)
    let outcome = services::sql::run(core, Access::System, text(config, "statement"), 100).await?;
    // what it did, not what it read: rows may hold anything
    Ok(json!({
        "affectedRows": outcome.affected_rows,
        "rows": outcome.rows,
        "readOnly": outcome.read_only,
    }))
}

async fn call_url(config: &Value) -> Result<Value, Failure> {
    let method = match text(config, "method") {
        "" => "GET",
        m => m,
    };
    let timeout =
        Duration::from_secs(number(config, "timeoutSeconds").unwrap_or(30).clamp(1, 60) as u64);
    let reply = outbound::send(outbound::Request {
        method,
        url: text(config, "url").trim(),
        headers: &headers_of(config),
        body: Some(text(config, "body")).filter(|b| !b.is_empty()),
        timeout,
    })
    .await?;
    let ok = match number(config, "expectStatus") {
        Some(want) => i64::from(reply.status) == want,
        None => (200..300).contains(&reply.status),
    };
    if !ok {
        return Err(Failure::transient(format!(
            "The server answered {}.",
            reply.status
        )));
    }
    // what happened, not what was answered: a response can hold anything the called service
    // returns, and the history is readable by more people than can see the job's settings
    Ok(http_result(&reply))
}

/// What the history keeps of a call: the status and the size of the answer, never its content.
fn http_result(reply: &outbound::Reply) -> Value {
    json!({ "status": reply.status, "bytes": reply.bytes })
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

/// A scheduled run that waited this long in the queue (the workers were busy, or no instance
/// was up) is dropped: the schedule has moved on, and running the minutes that were missed one
/// after the other helps nobody.
const STALE_AFTER_MINUTES: i64 = 10;

pub(crate) fn stale(scheduled_for: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    chrono::NaiveDateTime::parse_from_str(scheduled_for, "%Y-%m-%dT%H:%M")
        .is_ok_and(|at| (now.naive_utc() - at).num_minutes() >= STALE_AFTER_MINUTES)
}

/// One attempt of a queued run, recorded in the jobs' history.
///
/// A run is one row of the history however many attempts it takes: the first attempt starts
/// it, a failed attempt that can be retried leaves it running and hands the wait to the queue
/// (so the retry survives a restart and may happen on another instance), and the last attempt
/// closes it and tells the job's people.
async fn run_queued(core: Arc<Core>, job: crate::queue::Job) -> Result<(), Failure> {
    let manual = job.payload["trigger"] == "manual";
    let trigger = if manual { "manual" } else { "schedule" };
    // (a history that can't be read is not a history that isn't there: tried again later)
    let resumed = crate::jobs::resume(&core, job.id)
        .await
        .map_err(|e| Failure::transient(format!("{e:#}")))?;
    let settings = core.settings();
    let def = settings.cron_jobs.iter().find(|d| d.id == job.reference);
    // deleted, switched off or paused since it was queued: not run on its schedule
    let def = match def {
        Some(def) if manual || (def.enabled && !settings.schedule.pause_jobs) => def.clone(),
        _ => {
            if let Some(record) = resumed {
                let why = "The job was removed or switched off before it could be tried again.";
                record.finish(Err(why.to_string())).await;
            }
            return Ok(());
        }
    };
    let record = match resumed {
        Some(record) => record,
        None => {
            let scheduled_for = job.payload["minute"].as_str().unwrap_or_default();
            if !manual && stale(scheduled_for, chrono::Utc::now()) {
                tracing::warn!(job = %def.id, scheduled_for, "a cron job's run waited too long: dropped");
                return Ok(());
            }
            // (a run whose instance stopped doesn't count as going)
            let _ = crate::jobs::close_orphans(&core, crate::jobs::CRON, Some(&def.id)).await;
            let started =
                crate::jobs::start_exclusive(&core, crate::jobs::CRON, &def.id, trigger, job.id)
                    .await
                    .map_err(|e| Failure::transient(format!("{e:#}")))?;
            match started {
                Some(record) => record,
                None => {
                    tracing::warn!(job = %def.id, "a cron job is still running, or this run was already recorded: not started");
                    return Ok(());
                }
            }
        }
    };
    let tries = job.attempt.max(1) as u32;
    let result = match tokio::time::timeout(RUN_LIMIT, attempt(&core, &def)).await {
        Ok(result) => result,
        Err(_) => Err(Failure::transient(format!(
            "Stopped after {} minutes.",
            RUN_LIMIT.as_secs() / 60
        ))),
    };
    let result = match result {
        Ok(mut meta) => {
            if tries > 1 {
                meta["attempts"] = tries.into();
            }
            Ok(meta)
        }
        Err(fail) if fail.retry && job.attempt < job.max_attempts => {
            let delay = match def.retry_delay_seconds {
                0 => DEFAULT_RETRY_DELAY,
                d => d,
            };
            record
                .note(&format!(
                    "Attempt {tries} failed: {} It will be tried again.",
                    fail.message
                ))
                .await;
            return Err(fail.after(backoff(delay, tries)));
        }
        Err(fail) if tries > 1 => Err(format!("{} (after {tries} attempts)", fail.message)),
        Err(fail) => Err(fail.message),
    };
    if let Err(detail) = &result {
        tracing::error!(job = %def.id, error = %detail, "a cron job failed");
    }
    let error = result.as_ref().err().cloned().unwrap_or_default();
    // how the run before this one ended, to tell the first failure of a streak from the rest
    let event = match manual || def.notify.is_empty() {
        true => None,
        false => {
            let before = last_finished(&core, &def.id, record.id()).await;
            notice(before.as_deref(), result.is_ok())
        }
    };
    record.finish(result).await;
    if let Some(event) = event {
        queue_notices(&core, &def, job.id, event, &error, tries).await;
    }
    match error.is_empty() {
        true => Ok(()),
        false => Err(Failure::permanent(error)),
    }
}

/// What the job's finished run before `this` one came to: `ok`, `error` or nothing yet.
async fn last_finished(core: &Core, id: &str, this: Option<i64>) -> Option<String> {
    let found = async {
        let client = core
            .db()
            .get()
            .await
            .map_err(crate::db::describe_pool_error)?;
        let row = client
            .query_opt(
                "SELECT status FROM _slonbase.job_runs
                 WHERE kind = $1 AND job = $2 AND status IN ('ok', 'error')
                   AND ($3::int8 IS NULL OR id <> $3::int8)
                 ORDER BY started DESC, id DESC LIMIT 1",
                &[&crate::jobs::CRON, &id, &this],
            )
            .await?;
        anyhow::Ok(row.map(|row| row.get::<_, String>(0)))
    };
    match found.await {
        Ok(status) => status,
        Err(err) => {
            tracing::warn!(job = %id, error = %format!("{err:#}"), "failed to read a cron job's last run");
            None
        }
    }
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

impl Notice {
    fn name(self) -> &'static str {
        match self {
            Notice::Failed => "failed",
            Notice::Recovered => "recovered",
        }
    }
}

/// The queue's kind for one notice to one address.
pub const NOTICE_KIND: &str = "cron.notice";
/// How many times a notice is tried.
const NOTICE_ATTEMPTS: i32 = 3;

/// Queues a notice for each of the job's people: each is delivered on its own (one that fails
/// is tried again without the others being sent twice) and never changes how the run ended.
async fn queue_notices(
    core: &Arc<Core>,
    def: &JobDef,
    run: i64,
    event: Notice,
    error: &str,
    tries: u32,
) {
    let at = crate::datetime::format(&chrono::Utc::now());
    let mut targets: Vec<Value> = Vec::new();
    if !def.notify.emails.is_empty() {
        if mail::enabled(core) {
            targets.extend(def.notify.emails.iter().map(|a| json!({ "email": a })));
        } else {
            tracing::warn!(job = %def.id, "email is off: a cron job's notice was not sent");
        }
    }
    if !def.notify.webhook.is_empty() {
        // (the address stays in the job's settings: it may carry a token)
        targets.push(json!({ "webhook": true }));
    }
    for (n, target) in targets.into_iter().enumerate() {
        let queued = crate::queue::enqueue(
            core,
            crate::queue::NewJob {
                kind: NOTICE_KIND,
                reference: &def.id,
                payload: json!({
                    "event": event.name(), "error": error, "tries": tries, "at": at,
                    "target": target,
                }),
                dedupe: Some(&format!("notice:{run}:{n}")),
                max_attempts: NOTICE_ATTEMPTS,
            },
        )
        .await;
        if let Err(err) = queued {
            tracing::warn!(job = %def.id, error = %format!("{err:#}"), "failed to queue a cron job's notice");
        }
    }
}

/// Delivers one queued notice.
async fn deliver_notice(core: Arc<Core>, job: crate::queue::Job) -> Result<(), Failure> {
    let settings = core.settings();
    // (a job deleted meanwhile has nobody to tell)
    let Some(def) = settings.cron_jobs.iter().find(|d| d.id == job.reference) else {
        return Ok(());
    };
    let event = match job.payload["event"].as_str() {
        Some("recovered") => Notice::Recovered,
        _ => Notice::Failed,
    };
    let error = job.payload["error"].as_str().unwrap_or_default();
    let tries = job.payload["tries"].as_u64().unwrap_or(1) as u32;
    let at = job.payload["at"].as_str().unwrap_or_default();
    let target = &job.payload["target"];
    let sent = if let Some(address) = target["email"].as_str() {
        // (still one of the job's addresses)
        if !def.notify.emails.iter().any(|a| a == address) {
            return Ok(());
        }
        let (subject, body) = notice_text(def, event, error, tries, at);
        mail::send_text_to(&core, address, &subject, &body).await
    } else if def.notify.webhook.is_empty() {
        return Ok(());
    } else {
        let body = notice_json(def, event, error, tries, at).to_string();
        let reply = outbound::send(outbound::Request {
            method: "POST",
            url: &def.notify.webhook,
            headers: &[("content-type".to_string(), "application/json".to_string())],
            body: Some(&body),
            timeout: Duration::from_secs(10),
        })
        .await;
        match reply {
            Ok(reply) if !(200..300).contains(&reply.status) => Err(Failure::transient(format!(
                "The server answered {}.",
                reply.status
            ))),
            Ok(_) => Ok(()),
            Err(fail) => Err(fail),
        }
    };
    if let Err(fail) = &sent {
        tracing::warn!(job = %def.id, error = %fail, "a cron job's notice could not be delivered");
    }
    sent
}

/// Runs the dashboard jobs due this minute; one instance runs each (claimed like hook jobs).
pub(crate) async fn run_scheduled(
    core: &Arc<Core>,
    now: chrono::DateTime<chrono::Utc>,
    minute: &str,
    tz: chrono_tz::Tz,
) {
    let settings = core.settings();
    // paused: nothing is claimed either, so resuming within the minute still runs what is due
    if settings.schedule.pause_jobs {
        return;
    }
    for def in settings.cron_jobs.iter().filter(|d| d.enabled) {
        let due =
            crate::cron::Schedule::parse(&def.expression).is_some_and(|s| s.matches(&now, tz));
        if !due {
            continue;
        }
        // queued once for this minute, whichever instance notices first; a worker runs it
        let dedupe = format!("cron:{}:{minute}", def.id);
        if let Err(err) = enqueue(core, def, "schedule", minute, Some(&dedupe)).await {
            tracing::warn!(job = %def.id, error = %format!("{err:#}"), "failed to schedule a cron job");
        }
    }
}

/// The queue's kind for a run of a dashboard job.
pub const QUEUE_KIND: &str = "cron.user";

/// Queues a run of a job (`trigger`: `schedule` or `manual`), to be tried as many times as
/// the job says.
#[doc(hidden)]
pub async fn enqueue(
    core: &Core,
    def: &JobDef,
    trigger: &str,
    minute: &str,
    dedupe: Option<&str>,
) -> anyhow::Result<bool> {
    crate::queue::enqueue(
        core,
        crate::queue::NewJob {
            kind: QUEUE_KIND,
            reference: &def.id,
            payload: json!({ "trigger": trigger, "minute": minute }),
            dedupe,
            max_attempts: 1 + def.retries.clamp(0, MAX_RETRIES) as i32,
        },
    )
    .await
}

/// Tells the queue what runs a dashboard job and what delivers its notices. A run uses the job
/// as it is defined when the worker picks it up.
pub(crate) fn register(handlers: &mut crate::queue::Handlers) {
    handlers.register(QUEUE_KIND, run_queued);
    handlers.register(NOTICE_KIND, deliver_notice);
}

/// How the jobs list shows a dashboard job (the fields of a hook job, and its settings).
///
/// What a job does is shown by role: everyone who can open the page sees that a job exists, when
/// it runs and how it went; its settings (a query, an email's text and addresses, a filter, a
/// URL) only to those who can change it (admins and owners), and an SQL statement only to owners.
pub(crate) fn listed(
    def: &JobDef,
    tz: chrono_tz::Tz,
    now: chrono::DateTime<chrono::Utc>,
    last: Option<Value>,
    role: Role,
) -> Value {
    let sees_settings = role >= Role::Admin && (def.kind != "sql" || role >= Role::Owner);
    let (config, emails, webhook) = if sees_settings {
        (
            masked(def),
            def.notify.emails.clone(),
            !def.notify.webhook.is_empty(),
        )
    } else {
        (json!({}), Vec::new(), false)
    };
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
        "config": config,
        "notify": {
            "emails": emails,
            "webhook": if webhook { MASK } else { "" },
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

/// A header (or the webhook) sent back as the mask keeps its saved value. A saved header never
/// travels to another address: when the URL changes, the values have to be typed again.
fn unmask(def: &mut JobDef, saved: &JobDef) -> Result<(), String> {
    if def.notify.webhook == MASK {
        def.notify.webhook = saved.notify.webhook.clone();
    }
    if def.kind != "http" {
        return Ok(());
    }
    let was = headers_of(&saved.config);
    let mut kept = false;
    if let Some(h) = def.config.get_mut("headers").and_then(Value::as_object_mut) {
        for (name, value) in h.iter_mut() {
            if value.as_str() == Some(MASK) {
                kept = true;
                *value = json!(
                    was.iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                        .map(|(_, v)| v.as_str())
                        .unwrap_or_default()
                );
            }
        }
    }
    if kept && saved.kind == "http" && text(&def.config, "url").trim() != text(&saved.config, "url")
    {
        return Err("The URL changed: type the header values again, saved ones only go to the address they were saved with.".into());
    }
    Ok(())
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
            emails: mail::recipients(n.get("emails")),
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
    access::require(
        core,
        auth,
        Role::Owner,
        "Only owners can make or change SQL jobs.",
    )
    .map(|_| ())
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
    let role = access::role(&core, &auth)?;
    let snapshot = core.settings();
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
    // a copy keeps the secrets of the job it was made from (the form only has them masked)
    let kept = match body.get("copyOf").and_then(Value::as_str) {
        Some(from) => match snapshot.cron_jobs.iter().find(|j| j.id == from) {
            Some(source) => unmask(&mut def, source),
            None => return Err(ApiError::not_found()),
        },
        None => Ok(()),
    };
    let mut errors = validate(&core, &def, &snapshot.cron_jobs).await;
    if let Err(message) = kept {
        errors.invalid("config.headers", message);
    }
    if !errors.is_empty() {
        return Err(invalid(&errors));
    }
    def.config = normalized(&def);
    // added to the jobs as stored: the limits are checked again there, so jobs made at the same
    // time all stay, and no more than the limit can be made
    let added = def.clone();
    crate::settings::modify(&core, move |current| {
        let mut settings = current.clone();
        let mut errors = Errors::new();
        if settings.cron_jobs.len() >= MAX_JOBS {
            errors.invalid("name", "An installation has at most 50 jobs made here.");
        } else if settings
            .cron_jobs
            .iter()
            .any(|o| o.name.trim().eq_ignore_ascii_case(added.name.trim()))
        {
            errors.invalid("name", "Another job has this name.");
        }
        if !errors.is_empty() {
            return Err(invalid(&errors));
        }
        settings.cron_jobs.push(added);
        Ok(settings)
    })
    .await?;
    Ok(Json(listed(
        &def,
        core.settings().timezone(),
        chrono::Utc::now(),
        None,
        role,
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
            "to": mail::recipients(c.get("to")),
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
    let role = access::role(&core, &auth)?;
    let snapshot = core.settings();
    let Some(saved) = snapshot.cron_jobs.iter().find(|j| j.id == id).cloned() else {
        return Err(ApiError::not_found());
    };
    let mut def = read(&body, saved.clone());
    let kept = unmask(&mut def, &saved);
    if saved.kind == "sql" || def.kind == "sql" {
        owner_only(&core, &auth)?;
    }
    def.id = id.clone();
    // switching a job off must always work, whatever else about it has gone stale
    let only_off = body.len() == 1 && body.get("enabled") == Some(&json!(false));
    let mut errors = if only_off {
        Errors::new()
    } else {
        validate(&core, &def, &snapshot.cron_jobs).await
    };
    if let Err(message) = kept {
        errors.invalid("config.headers", message);
    }
    if !errors.is_empty() {
        return Err(invalid(&errors));
    }
    def.config = normalized(&def);
    def.updated = crate::datetime::format(&chrono::Utc::now());
    let replacement = def.clone();
    crate::settings::modify(&core, move |current| {
        let mut settings = current.clone();
        let Some(pos) = settings.cron_jobs.iter().position(|j| j.id == id) else {
            return Err(ApiError::not_found());
        };
        settings.cron_jobs[pos] = replacement;
        Ok(settings)
    })
    .await?;
    Ok(Json(listed(
        &def,
        core.settings().timezone(),
        chrono::Utc::now(),
        None,
        role,
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
    crate::settings::modify(&core, |current| {
        let mut settings = current.clone();
        let before = settings.cron_jobs.len();
        settings.cron_jobs.retain(|j| j.id != id);
        if settings.cron_jobs.len() == before {
            return Err(ApiError::not_found());
        }
        Ok(settings)
    })
    .await?;
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
    let from = match body.get("copyOf").and_then(Value::as_str) {
        Some(from) => from,
        None => def.id.as_str(),
    };
    let kept = match others.iter().find(|j| j.id == from) {
        Some(saved) => unmask(&mut def, saved),
        None => Ok(()),
    };
    let mut errors = validate(&core, &def, &others).await;
    if let Err(message) = kept {
        errors.invalid("config.headers", message);
    }
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
    fn the_history_of_a_call_has_no_response_content() {
        let reply = outbound::Reply {
            status: 200,
            excerpt: "{\"token\":\"s3cret\"}".into(),
            bytes: 17,
            url: "https://example.com/".into(),
        };
        let meta = http_result(&reply);
        assert_eq!(meta, json!({ "status": 200, "bytes": 17 }));
        assert!(!meta.to_string().contains("s3cret"));
    }

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

    #[test]
    fn a_scheduled_run_that_waited_too_long_is_stale() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-10T12:30:20Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(!stale("2026-11-10T12:30", now));
        assert!(!stale("2026-11-10T12:21", now));
        assert!(stale("2026-11-10T12:20", now));
        assert!(stale("2026-11-09T12:30", now));
        // a run with no minute (started by hand) never is
        assert!(!stale("", now));
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
