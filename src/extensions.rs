//! What this instance has loaded (`GET /api/extensions`; Settings, Extensions in the dashboard):
//! the hook files with their handlers and routes, the cron jobs registered by code, the kinds of
//! jobs its queue workers run and the sections of the settings. It answers "is my code running
//! here?" without reading logs, and lists the registries a feature plugs into.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::app::App;
use crate::auth::RequestAuth;
use crate::error::ApiError;

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
        "jobKinds": kinds,
        "settingsSections": sections,
    })))
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
        let queue = crate::queue::Queue::new(handlers);
        assert_eq!(queue.kinds().len(), 4);
        for kind in queue.kinds() {
            assert!(!kind_title(kind).is_empty(), "{kind}");
        }
    }
}
