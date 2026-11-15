//! The template library: emails written once (a subject and an HTML body in Jinja syntax, see
//! [`crate::templating`]) and sent by automations with the data of each run.
//!
//! - Templates live in the settings' private `templates` section, like API keys and
//!   automations: sealed with the rest when there is an encryption key, in every backup, and
//!   known to every instance as soon as one changes.
//! - A template only produces text from the data it is given; values are HTML-escaped in the
//!   body and the subject is one plain line, so data can't bring markup or headers in.
//! - The body is put in the mail layout (Settings → Mail) like every other email.
//! - Everyone on the team can read them and preview one (`/api/templates`); admins and owners
//!   make, change and delete them. A template in use by an automation can't be deleted.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::app::App;
use crate::auth::RequestAuth;
use crate::core::Core;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::services::{Failure, mail};
use crate::settings::Settings;
use crate::templating::{self, Mode};
use crate::validation::Errors;

const MAX_TEMPLATES: usize = 50;
const MAX_BODY: usize = 100_000;
const MAX_SAMPLE: usize = 20_000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Template {
    pub id: String,
    pub name: String,
    /// What it is for, for the people who pick it.
    pub description: String,
    pub subject: String,
    /// The email's HTML.
    pub body: String,
    /// Data to preview it with, shaped like what an automation will give it.
    pub sample: Value,
    pub created: String,
    pub updated: String,
}

/// A rendered email.
pub struct Rendered {
    pub subject: String,
    pub html: String,
}

/// Renders a subject and a body with `data`: the subject as one plain line, the body as HTML in
/// the mail layout.
pub fn render(
    settings: &Settings,
    subject: &str,
    body: &str,
    data: &Value,
) -> Result<Rendered, String> {
    let subject = templating::render(subject, data, Mode::Text)
        .map_err(|e| format!("Subject: {e}"))?
        .trim()
        .to_string();
    if subject.is_empty() || subject.chars().count() > 200 || !mail::plain_line(&subject) {
        return Err("Subject: must come to 1 to 200 characters on one line.".into());
    }
    let body = templating::render(body, data, Mode::Html).map_err(|e| format!("Body: {e}"))?;
    let html = crate::mail::render(&crate::mail::wrap(settings, &body), settings, "");
    Ok(Rendered { subject, html })
}

/// Sends the template `id`, rendered with `data`, to each address. Returns how many were sent.
pub async fn send(
    core: &Arc<Core>,
    id: &str,
    to: &[String],
    data: &Value,
) -> Result<usize, Failure> {
    let settings = core.settings();
    let Some(template) = settings.templates.iter().find(|t| t.id == id) else {
        return Err(Failure::permanent("The template no longer exists."));
    };
    if !mail::enabled(core) {
        return Err(Failure::permanent(mail::DISABLED));
    }
    mail::check_addresses(to).map_err(Failure::permanent)?;
    let rendered =
        render(&settings, &template.subject, &template.body, data).map_err(Failure::permanent)?;
    let mut sent = 0;
    for address in to {
        core.send_mail_now(crate::mail::Message {
            to: address.clone(),
            subject: rendered.subject.clone(),
            html: rendered.html.clone(),
        })
        .await
        .map_err(|e| Failure::transient(format!("Sending to {address} failed: {e}")))?;
        sent += 1;
    }
    Ok(sent)
}

fn validate(template: &Template, others: &[Template]) -> Errors {
    let mut errors = Errors::new();
    let name = template.name.trim();
    if name.is_empty() || name.chars().count() > 60 || !mail::plain_line(name) {
        errors.invalid("name", "Must be 1 to 60 characters on one line.");
    } else if others
        .iter()
        .any(|o| o.id != template.id && o.name.trim().eq_ignore_ascii_case(name))
    {
        errors.invalid("name", "Another template has this name.");
    }
    if template.description.chars().count() > 300 {
        errors.invalid("description", "At most 300 characters.");
    }
    let subject = template.subject.trim();
    if subject.is_empty() || subject.chars().count() > 300 || !mail::plain_line(subject) {
        errors.invalid("subject", "Must be 1 to 300 characters on one line.");
    } else if let Err(message) = templating::check(subject) {
        errors.invalid("subject", message);
    }
    if template.body.trim().is_empty() || template.body.chars().count() > MAX_BODY {
        errors.invalid("body", "Must be 1 to 100 000 characters.");
    } else if let Err(message) = templating::check(&template.body) {
        errors.invalid("body", message);
    }
    if !template.sample.is_object() || template.sample.to_string().len() > MAX_SAMPLE {
        errors.invalid(
            "sample",
            "Must be a JSON object of at most 20 000 characters.",
        );
    }
    errors
}

fn read(body: &Map<String, Value>, base: Template) -> Template {
    let mut template = base;
    let text = |key: &str| body.get(key).and_then(Value::as_str);
    if let Some(v) = text("name") {
        template.name = v.trim().to_string();
    }
    if let Some(v) = text("description") {
        template.description = v.trim().to_string();
    }
    if let Some(v) = text("subject") {
        template.subject = v.trim().to_string();
    }
    if let Some(v) = text("body") {
        template.body = v.to_string();
    }
    if let Some(v) = body.get("sample") {
        template.sample = v.clone();
    }
    template
}

fn invalid(errors: &Errors) -> ApiError {
    ApiError::invalid_data(
        "An error occurred while validating the submitted data.",
        errors,
    )
}

/// `GET /api/templates`: the templates, by name.
pub async fn list(State(app): State<Arc<App>>, auth: RequestAuth) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let mut items = core.settings().templates.clone();
    items.sort_by_key(|t| t.name.to_lowercase());
    Ok(Json(json!({ "items": items })))
}

/// `POST /api/templates`: makes a template.
pub async fn create(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Template>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let now = crate::datetime::format(&chrono::Utc::now());
    let template = read(
        &body,
        Template {
            id: crate::ids::random_id(15),
            sample: json!({}),
            created: now.clone(),
            updated: now,
            ..Default::default()
        },
    );
    let errors = validate(&template, &core.settings().templates);
    if !errors.is_empty() {
        return Err(invalid(&errors));
    }
    let added = template.clone();
    crate::settings::modify(&core, move |current| {
        let mut settings = current.clone();
        // checked again on the templates as stored: two made at the same time both count
        let mut errors = validate(&added, &settings.templates);
        if settings.templates.len() >= MAX_TEMPLATES {
            errors.invalid("name", "An installation has at most 50 templates.");
        }
        if !errors.is_empty() {
            return Err(invalid(&errors));
        }
        settings.templates.push(added);
        Ok(settings)
    })
    .await?;
    Ok(Json(template))
}

/// `PATCH /api/templates/{id}`: changes a template.
pub async fn update(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Template>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let wanted = id.clone();
    let changed = crate::settings::modify(&core, move |current| {
        let mut settings = current.clone();
        let Some(pos) = settings.templates.iter().position(|t| t.id == id) else {
            return Err(ApiError::not_found());
        };
        let mut template = read(&body, settings.templates[pos].clone());
        template.updated = crate::datetime::format(&chrono::Utc::now());
        let errors = validate(&template, &settings.templates);
        if !errors.is_empty() {
            return Err(invalid(&errors));
        }
        settings.templates[pos] = template;
        Ok(settings)
    })
    .await?;
    let template = changed
        .templates
        .iter()
        .find(|t| t.id == wanted)
        .cloned()
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(template))
}

/// `DELETE /api/templates/{id}`: deletes a template no automation uses.
pub async fn delete(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    auth: RequestAuth,
) -> Result<StatusCode, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    crate::settings::modify(&core, move |current| {
        let mut settings = current.clone();
        let before = settings.templates.len();
        settings.templates.retain(|t| t.id != id);
        if settings.templates.len() == before {
            return Err(ApiError::not_found());
        }
        if let Some(user) = crate::automations::using_template(&settings.cron_jobs, &id) {
            return Err(ApiError::bad_request(format!(
                "The automation \"{user}\" sends this template: change or delete it first."
            )));
        }
        Ok(settings)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The data of a preview: what was sent, or nothing.
fn data_of(body: &Map<String, Value>) -> Value {
    match body.get("data") {
        Some(data) if data.is_object() => data.clone(),
        _ => json!({}),
    }
}

/// `POST /api/templates/preview`: renders `{subject, body, data}` as the email would be, without
/// saving or sending anything.
pub async fn preview(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let text = |key: &str| body.get(key).and_then(Value::as_str).unwrap_or_default();
    if text("body").chars().count() > MAX_BODY || text("subject").chars().count() > 300 {
        return Err(ApiError::bad_request("The template is too long."));
    }
    let rendered = render(
        &core.settings(),
        text("subject"),
        text("body"),
        &data_of(&body),
    )
    .map_err(ApiError::bad_request)?;
    Ok(Json(
        json!({ "subject": rendered.subject, "html": rendered.html }),
    ))
}

/// `POST /api/templates/{id}/test`: sends the template to `{email}`, rendered with `{data}` or
/// its sample.
pub async fn test(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<StatusCode, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let settings = core.settings();
    let Some(template) = settings.templates.iter().find(|t| t.id == id) else {
        return Err(ApiError::not_found());
    };
    let to = mail::recipients(body.get("email"));
    if to.len() != 1 || mail::check_addresses(&to).is_err() {
        let mut errors = Errors::new();
        errors.invalid("email", "Must be one valid email address.");
        return Err(invalid(&errors));
    }
    let data = match body.get("data") {
        Some(data) if data.is_object() => data.clone(),
        _ => template.sample.clone(),
    };
    send(&core, &id, &to, &data)
        .await
        .map_err(|f| ApiError::bad_request(f.message))?;
    Ok(StatusCode::NO_CONTENT)
}

/// The queue's kind for an email sent with `POST /api/templates/{id}/send`.
pub const SEND_KIND: &str = "template.send";
const SEND_ATTEMPTS: i32 = 3;
/// The most data a send may carry, as JSON.
const MAX_SEND_DATA: usize = 64 * 1024;
/// The most emails one caller may queue per minute.
const SENDS_PER_MINUTE: u32 = 100;

/// `POST /api/templates/{id}/send`: sends the template to `{to}` (one address or up to 10),
/// rendered with `{data}`. Developers, admins and owners.
///
/// The email is checked now (the template, the addresses, that it renders with this data)
/// and sent in the background, one queued job per address, each tried up to 3 times: the
/// answer is `202` with the id of the send. An `Idempotency-Key` header makes a request safe
/// to repeat: the same key for the same template queues nothing the second time (for 7 days).
pub async fn send_api(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    auth: RequestAuth,
    headers: axum::http::HeaderMap,
    JsonObject(body): JsonObject,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    use sha2::Digest;
    let core = app.core()?;
    auth.require_superuser()?;
    let settings = core.settings();
    let Some(template) = settings.templates.iter().find(|t| t.id == id) else {
        return Err(ApiError::not_found());
    };
    let mut errors = Errors::new();
    let to = mail::recipients(body.get("to"));
    if let Err(message) = mail::check_addresses(&to) {
        errors.invalid("to", message);
    } else if !mail::enabled(&core) {
        errors.invalid("to", mail::DISABLED);
    }
    let data = data_of(&body);
    if data.to_string().len() > MAX_SEND_DATA {
        errors.invalid("data", "At most 64 KB of data.");
    } else if let Err(message) = render(&settings, &template.subject, &template.body, &data) {
        errors.invalid("data", message);
    }
    if !errors.is_empty() {
        return Err(invalid(&errors));
    }
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|k| !k.is_empty());
    if key.is_some_and(|k| k.len() > 200) {
        return Err(ApiError::bad_request(
            "The Idempotency-Key is at most 200 characters.",
        ));
    }
    let caller = auth
        .superuser
        .as_ref()
        .map(|s| s.str("id"))
        .unwrap_or_default();
    let allowed = crate::guard::within_limit(
        &core,
        &format!("template-send|{caller}"),
        to.len() as u32,
        SENDS_PER_MINUTE,
        60,
    )
    .await
    .map_err(ApiError::internal)?;
    if !allowed {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("At most {SENDS_PER_MINUTE} emails a minute can be sent this way."),
        ));
    }
    // the send's id: what the key comes to for this template, or a new one
    let send = match key {
        Some(key) => {
            hex::encode(sha2::Sha256::digest(format!("{id}\n{key}").as_bytes()))[..20].to_string()
        }
        None => crate::ids::random_id(20),
    };
    // all of the send or none of it: a failure midway leaves nothing queued, so asking again
    // can't send to the first addresses twice
    let mut client = core
        .db()
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let tx = client
        .transaction()
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    let mut queued = 0;
    for (n, address) in to.iter().enumerate() {
        queued += tx
            .execute(
                "INSERT INTO _slonbase.jobs (kind, ref, payload, dedupe, max_attempts)
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT (dedupe) DO NOTHING",
                &[
                    &SEND_KIND,
                    &id,
                    &json!({ "to": address, "data": data, "send": send }),
                    &format!("template-send:{send}:{n}"),
                    &SEND_ATTEMPTS,
                ],
            )
            .await
            .map_err(|e| ApiError::internal(e.into()))?;
    }
    tx.commit()
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    drop(client);
    if queued > 0 {
        core.queue.wake();
    }
    tracing::info!(template = %id, send = %send, by = %caller, emails = queued, "a template was queued to be sent");
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "id": send, "queued": queued, "duplicate": queued == 0 })),
    ))
}

/// Sends one queued email of a template.
async fn deliver_queued(core: Arc<Core>, job: crate::queue::Job) -> Result<(), Failure> {
    let to = job.payload["to"].as_str().unwrap_or_default().to_string();
    let sent = send(&core, &job.reference, &[to], &job.payload["data"]).await;
    if let Err(fail) = &sent {
        tracing::warn!(template = %job.reference, send = %job.payload["send"], error = %fail, "an email of a template could not be sent");
    }
    sent.map(|_| ())
}

pub(crate) fn register_jobs(handlers: &mut crate::queue::Handlers) {
    handlers.register(SEND_KIND, deliver_queued);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(name: &str) -> Template {
        Template {
            id: name.to_string(),
            name: name.to_string(),
            subject: "Hello {{ name }}".into(),
            body: "<p>Hi {{ name }}</p>".into(),
            sample: json!({ "name": "Ann" }),
            ..Default::default()
        }
    }

    #[test]
    fn a_template_is_checked() {
        assert!(validate(&template("a"), &[]).is_empty());
        let mut other = template("b");
        other.name = "A".into();
        assert!(validate(&other, &[template("a")]).contains("name"));
        let mut bad = template("c");
        bad.subject = "Hi {{ name".into();
        bad.body = "{% if x %}".into();
        bad.sample = json!([1]);
        let errors = validate(&bad, &[]);
        assert!(errors.contains("subject") && errors.contains("body") && errors.contains("sample"));
        let mut lines = template("d");
        lines.subject = "one\ntwo".into();
        assert!(validate(&lines, &[]).contains("subject"));
    }

    #[test]
    fn data_cant_bring_markup_or_lines_in() {
        let settings = Settings::default();
        let data = json!({ "name": "<script>x</script>" });
        let out = render(&settings, "Hi {{ name }}", "<p>{{ name }}</p>", &data).unwrap();
        assert_eq!(out.subject, "Hi <script>x</script>");
        assert!(out.html.contains("&lt;script&gt;"), "{}", out.html);
        assert!(!out.html.contains("<script>"), "{}", out.html);
        // a subject must stay one line whatever the data holds
        let data = json!({ "name": "a\nBcc: x@y.co" });
        assert!(render(&settings, "Hi {{ name }}", "x", &data).is_err());
        assert!(render(&settings, "{{ nothing }}", "x", &data).is_err());
    }
}
