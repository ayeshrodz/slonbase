//! What this instance has loaded (`GET /api/extensions`; Settings, Extensions in the dashboard):
//! the hook files with their handlers and routes, the cron jobs registered by code, the kinds of
//! jobs its queue workers run and the sections of the settings. It answers "is my code running
//! here?" without reading logs, and lists the registries a feature plugs into.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Map, Value, json};

use crate::app::App;
use crate::auth::RequestAuth;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::validation::Errors;

/// What loading the hook files registered, kept for this page.
#[derive(Debug, Default, Clone)]
pub struct Loaded {
    /// The hook files, by name.
    pub files: Vec<String>,
    /// Each handler: its event as hook files name it (`onRecordCreate`), and the collections
    /// it is limited to (none: all).
    pub handlers: Vec<(String, Vec<String>)>,
    /// Each route: its method and path.
    pub routes: Vec<(String, String)>,
}

/// What a kind of queued job does, for the page.
fn kind_title(kind: &str) -> &'static str {
    match kind {
        crate::cron_jobs::QUEUE_KIND => "A run of a cron job made in the dashboard",
        crate::cron_jobs::NOTICE_KIND => "A failure or recovery notice of a cron job",
        crate::hooks::CRON_QUEUE_KIND => "A run of a cron job registered by code",
        crate::backups::QUEUE_KIND => "The automatic backup",
        crate::templates::SEND_KIND => "An email of a template sent through the API",
        _ => "",
    }
}

/// `GET /api/extensions`: developers, admins and owners (the role is checked by the guard).
pub async fn list(State(app): State<Arc<App>>, auth: RequestAuth) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let loaded = &core.hooks.loaded;
    let handlers: Vec<Value> = loaded
        .handlers
        .iter()
        .map(|(event, collections)| json!({ "event": event, "collections": collections }))
        .collect();
    let routes: Vec<Value> = loaded
        .routes
        .iter()
        .map(|(method, path)| json!({ "method": method, "path": path }))
        .collect();
    let crons: Vec<Value> = core
        .hooks
        .crons()
        .iter()
        .map(|job| json!({ "id": job.id, "expression": job.expression }))
        .collect();
    let kinds: Vec<Value> = core
        .queue
        .kinds()
        .into_iter()
        .map(|kind| json!({ "kind": kind, "title": kind_title(kind) }))
        .collect();
    let sections: Vec<Value> = crate::settings::SECTIONS
        .iter()
        .map(|s| json!({ "name": s.name, "title": s.title, "private": s.private }))
        .collect();
    Ok(Json(json!({
        "hooks": { "files": loaded.files, "handlers": handlers, "routes": routes },
        "cronJobs": crons,
        "stepFunctions": core.hooks.steps(),
        "settingsPages": core.hooks.settings_forms().iter().map(|f| json!({ "name": f.name, "title": f.title })).collect::<Vec<_>>(),
        "jobKinds": kinds,
        "settingsSections": sections,
    })))
}

/// A settings page with its values as the API shows them: secrets as the mask when set.
fn form_json(form: &crate::hooks::SettingsForm, saved: Option<&Map<String, Value>>) -> Value {
    let mut values = form.values(saved);
    for field in form.fields.iter().filter(|f| f.kind == "secret") {
        let set = values
            .get(&field.name)
            .and_then(Value::as_str)
            .is_some_and(|v| !v.is_empty());
        values.insert(
            field.name.clone(),
            json!(if set { crate::cron_jobs::MASK } else { "" }),
        );
    }
    // (a secret's default is a secret too: the form is shown without it)
    let fields: Vec<crate::hooks::SettingsField> = form
        .fields
        .iter()
        .map(|field| crate::hooks::SettingsField {
            default: match field.kind == "secret" {
                true => Value::Null,
                false => field.default.clone(),
            },
            ..field.clone()
        })
        .collect();
    json!({ "name": form.name, "title": form.title, "fields": fields, "values": values })
}

/// `GET /api/extensions/settings`: the settings pages extensions declared, with their values.
/// Developers, admins and owners.
pub async fn settings(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let saved = core.settings();
    let forms: Vec<Value> = core
        .hooks
        .settings_forms()
        .iter()
        .map(|form| form_json(form, saved.extensions.get(&form.name)))
        .collect();
    Ok(Json(json!(forms)))
}

/// Checks `sent` against the form and returns the values to store. A secret sent as the mask
/// (or left out) keeps its saved value.
fn checked(
    form: &crate::hooks::SettingsForm,
    sent: &Map<String, Value>,
    saved: Option<&Map<String, Value>>,
) -> Result<Map<String, Value>, Errors> {
    let mut errors = Errors::new();
    let mut out = Map::new();
    for key in sent.keys() {
        if !form.fields.iter().any(|f| &f.name == key) {
            errors.invalid(format!("values.{key}"), "The page has no such field.");
        }
    }
    for field in &form.fields {
        let at = format!("values.{}", field.name);
        let kept = saved.and_then(|s| s.get(&field.name)).cloned();
        let value = match sent.get(&field.name) {
            // (the mask stands for what the page showed: the saved value, or the default)
            Some(Value::String(v)) if field.kind == "secret" && v == crate::cron_jobs::MASK => {
                kept.unwrap_or_else(|| field.default.clone())
            }
            Some(value) => value.clone(),
            None => kept.unwrap_or_else(|| field.default.clone()),
        };
        let value = match (field.kind.as_str(), value) {
            ("bool", Value::Bool(b)) => json!(b),
            ("bool", Value::Null) => json!(false),
            ("number", Value::Number(n)) => json!(n),
            ("number", Value::String(s)) if s.trim().is_empty() => Value::Null,
            ("number", Value::String(s)) => match s.trim().parse::<f64>() {
                Ok(n) if n.is_finite() => json!(n),
                _ => {
                    errors.invalid(at, "Must be a number.");
                    continue;
                }
            },
            ("number", Value::Null) => Value::Null,
            ("text" | "textarea" | "secret" | "select", Value::Null) => json!(""),
            ("text" | "textarea" | "secret" | "select", Value::String(s)) => {
                if s.chars().count() > 10_000 {
                    errors.invalid(at, "At most 10 000 characters.");
                    continue;
                }
                if field.kind == "select" && !s.is_empty() && !field.options.contains(&s) {
                    errors.invalid(at, "Choose one of the options.");
                    continue;
                }
                json!(s)
            }
            _ => {
                errors.invalid(at, "Not a value of this field's type.");
                continue;
            }
        };
        let empty = value.is_null() || value == json!("");
        if field.required && empty {
            errors.invalid(format!("values.{}", field.name), "Cannot be blank.");
            continue;
        }
        out.insert(field.name.clone(), value);
    }
    match errors.is_empty() {
        true => Ok(out),
        false => Err(errors),
    }
}

/// `PATCH /api/extensions/settings/{name}`: saves `{values}` of a settings page. Admins and
/// owners.
pub async fn save_settings(
    State(app): State<Arc<App>>,
    Path(name): Path<String>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let Some(form) = core
        .hooks
        .settings_forms()
        .iter()
        .find(|f| f.name == name)
        .cloned()
    else {
        return Err(ApiError::not_found());
    };
    let sent = body
        .get("values")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let checking = form.clone();
    crate::settings::modify(&core, move |current| {
        let values = checked(&checking, &sent, current.extensions.get(&checking.name)).map_err(
            |errors| {
                ApiError::invalid_data(
                    "An error occurred while validating the submitted data.",
                    &errors,
                )
            },
        )?;
        let mut settings = current.clone();
        settings.extensions.insert(checking.name.clone(), values);
        Ok(settings)
    })
    .await?;
    Ok(Json(form_json(
        &form,
        core.settings().extensions.get(&form.name),
    )))
}

/// The name hook files use for a registration's kind: `recordCreate` is `onRecordCreate`.
pub(crate) fn event_name(kind: &str) -> String {
    match kind {
        "routerUse" => "routerUse".to_string(),
        "ready" => "onBootstrap".to_string(),
        _ => {
            let mut name = String::with_capacity(kind.len() + 2);
            name.push_str("on");
            let mut chars = kind.chars();
            if let Some(first) = chars.next() {
                name.extend(first.to_uppercase());
            }
            name.push_str(chars.as_str());
            name
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_named_as_hook_files_name_them() {
        assert_eq!(event_name("recordCreate"), "onRecordCreate");
        assert_eq!(
            event_name("recordAuthWithOTPRequest"),
            "onRecordAuthWithOTPRequest"
        );
        assert_eq!(event_name("routerUse"), "routerUse");
        assert_eq!(event_name("ready"), "onBootstrap");
    }

    #[test]
    fn every_kind_of_job_has_a_title() {
        let mut handlers = crate::queue::Handlers::default();
        crate::cron_jobs::register(&mut handlers);
        crate::hooks::register_jobs(&mut handlers);
        crate::backups::register_jobs(&mut handlers);
        crate::templates::register_jobs(&mut handlers);
        let queue = crate::queue::Queue::new(handlers);
        assert_eq!(queue.kinds().len(), 5);
        for kind in queue.kinds() {
            assert!(!kind_title(kind).is_empty(), "{kind}");
        }
    }
}
