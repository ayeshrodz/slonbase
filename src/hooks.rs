//! Hooks: code that runs around record requests and after record changes, plus cron jobs
//! and startup callbacks. They're registered before the server starts, from Rust (see
//! [`crate::Extensions`]) or from JavaScript files (`*.pb.js`).
//!
//! A hook receives the event and a [`Next`] handle. Calling `next.run(e)` runs the
//! following hooks and then the action itself (writing the record, for example), and
//! returns the event as the action left it. A hook can change the event before calling
//! `next`, change it afterwards, or return early (with an error, or with a custom
//! `response`) to skip the action:
//!
//! ```no_run
//! # let mut hooks = slonbase::hooks::Hooks::default();
//! hooks.on_record_create_request.add(&["posts"], |mut e, next| async move {
//!     e.record.insert("status".into(), "draft".into());
//!     next.run(e).await
//! });
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::response::Response;
use serde_json::{Map, Value};

use crate::auth::RequestAuth;
use crate::collections::model::Collection;
use crate::core::Core;
use crate::error::ApiError;
use crate::filter::RequestInfo;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
pub type HookResult<E> = Result<E, ApiError>;

type HandlerFn<E> = dyn Fn(E, Next<E>) -> BoxFuture<HookResult<E>> + Send + Sync;
type LastFn<E> = Box<dyn FnOnce(E) -> BoxFuture<HookResult<E>> + Send>;

struct Handler<E> {
    /// Collection ids or names the handler is limited to (empty: all).
    collections: Vec<String>,
    f: Arc<HandlerFn<E>>,
}

/// Runs the rest of a hook chain: the following handlers, then the action.
pub struct Next<E> {
    handlers: Arc<[Arc<HandlerFn<E>>]>,
    index: usize,
    last: LastFn<E>,
}

impl<E: Send + 'static> Next<E> {
    pub async fn run(self, e: E) -> HookResult<E> {
        match self.handlers.get(self.index).cloned() {
            Some(handler) => {
                let next = Next {
                    handlers: self.handlers,
                    index: self.index + 1,
                    last: self.last,
                };
                handler(e, next).await
            }
            None => (self.last)(e).await,
        }
    }
}

/// The handlers of one event.
pub struct Hook<E> {
    handlers: Vec<Handler<E>>,
}

impl<E> Default for Hook<E> {
    fn default() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }
}

impl<E: Send + 'static> Hook<E> {
    /// Adds a handler, limited to the given collections (ids or names; empty for all).
    pub fn add<F, Fut>(&mut self, collections: &[&str], f: F)
    where
        F: Fn(E, Next<E>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HookResult<E>> + Send + 'static,
    {
        self.add_boxed(
            collections.iter().map(|c| c.to_string()).collect(),
            Arc::new(move |e, next| Box::pin(f(e, next))),
        );
    }

    pub(crate) fn add_boxed(&mut self, collections: Vec<String>, f: Arc<HandlerFn<E>>) {
        self.handlers.push(Handler { collections, f });
    }

    fn applicable(&self, c: Option<&Collection>) -> Arc<[Arc<HandlerFn<E>>]> {
        self.handlers
            .iter()
            .filter(|h| {
                h.collections.is_empty()
                    || c.is_some_and(|c| h.collections.iter().any(|n| *n == c.id || *n == c.name))
            })
            .map(|h| h.f.clone())
            .collect()
    }

    /// Whether any handler runs for the collection.
    pub fn has_handlers(&self, c: Option<&Collection>) -> bool {
        !self.applicable(c).is_empty()
    }

    /// Runs the handlers for the collection, then `last` (the action).
    pub async fn trigger<L, LF>(&self, c: Option<&Collection>, e: E, last: L) -> HookResult<E>
    where
        L: FnOnce(E) -> LF + Send + 'static,
        LF: Future<Output = HookResult<E>> + Send + 'static,
    {
        let next = Next {
            handlers: self.applicable(c),
            index: 0,
            last: Box::new(move |e| Box::pin(last(e))),
        };
        next.run(e).await
    }
}

/// A record request (create, update, delete or view) before it's handled.
pub struct RecordRequestEvent {
    pub core: Arc<Core>,
    pub collection: Arc<Collection>,
    /// Create: the submitted data. Update: the current record with the submitted data
    /// applied. Delete and view: the record. After `next`, the record as the caller sees
    /// it (create, update), which is what's sent back.
    pub record: Map<String, Value>,
    pub auth: RequestAuth,
    pub request: Arc<RequestInfo>,
    /// Set to answer with this response instead of the record.
    pub response: Option<Response>,
}

/// A records list request, with the page of records about to be returned.
pub struct RecordsListEvent {
    pub core: Arc<Core>,
    pub collection: Arc<Collection>,
    pub records: Vec<Value>,
    pub page: i64,
    pub per_page: i64,
    pub total_items: i64,
    pub total_pages: i64,
    pub auth: RequestAuth,
    pub request: Arc<RequestInfo>,
    pub response: Option<Response>,
}

/// Any request (the API, the dashboard and custom routes), before it's routed: global
/// middleware. `next` hands the request to the router and sets the response; a handler
/// that doesn't call it answers with its own response (or an empty 200).
pub struct RequestEvent {
    pub core: Arc<Core>,
    /// The caller (none for a missing or invalid token).
    pub auth: RequestAuth,
    /// The request, until `next` takes it.
    pub request: Option<axum::extract::Request>,
    pub response: Option<Response>,
}

/// A record that was created, updated or deleted (after its transaction committed).
pub struct RecordEvent {
    pub core: Arc<Core>,
    pub collection: Arc<Collection>,
    /// All values of the record, including hidden fields (but not password hashes and
    /// token keys).
    pub record: Map<String, Value>,
}

/// A cron job, run by one instance each minute its schedule matches.
pub struct CronJob {
    pub id: String,
    pub expression: String,
    pub(crate) schedule: crate::cron::Schedule,
    pub(crate) run: Arc<CronFn>,
}

type ReadyFn = dyn Fn(Arc<Core>) -> BoxFuture<()> + Send + Sync;
pub(crate) type CronFn = dyn Fn(Arc<Core>) -> BoxFuture<Result<(), ApiError>> + Send + Sync;

/// All registered hooks.
#[derive(Default)]
pub struct Hooks {
    /// Runs for every request when the server is ready (see [`RequestEvent`]).
    pub on_request: Hook<RequestEvent>,
    pub on_record_create_request: Hook<RecordRequestEvent>,
    pub on_record_update_request: Hook<RecordRequestEvent>,
    pub on_record_delete_request: Hook<RecordRequestEvent>,
    pub on_record_view_request: Hook<RecordRequestEvent>,
    pub on_records_list_request: Hook<RecordsListEvent>,
    pub on_record_after_create_success: Hook<RecordEvent>,
    pub on_record_after_update_success: Hook<RecordEvent>,
    pub on_record_after_delete_success: Hook<RecordEvent>,
    pub(crate) crons: Vec<CronJob>,
    pub(crate) on_ready: Vec<Arc<ReadyFn>>,
}

impl Hooks {
    /// Adds a cron job (`minute hour day month weekday`, in UTC). The id must be unique.
    pub fn cron<F, Fut>(&mut self, id: &str, expression: &str, f: F) -> anyhow::Result<()>
    where
        F: Fn(Arc<Core>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), ApiError>> + Send + 'static,
    {
        self.add_cron(id, expression, Arc::new(move |core| Box::pin(f(core))))
    }

    pub(crate) fn add_cron(
        &mut self,
        id: &str,
        expression: &str,
        run: Arc<CronFn>,
    ) -> anyhow::Result<()> {
        let schedule = crate::cron::Schedule::parse(expression)
            .ok_or_else(|| anyhow::anyhow!("invalid cron expression {expression:?}"))?;
        anyhow::ensure!(
            !id.is_empty() && !id.starts_with("__"),
            "invalid cron job id {id:?}"
        );
        // re-adding a job replaces it (like PocketBase)
        self.crons.retain(|job| job.id != id);
        self.crons.push(CronJob {
            id: id.to_string(),
            expression: expression.to_string(),
            schedule,
            run,
        });
        Ok(())
    }

    /// Removes a cron job.
    pub fn remove_cron(&mut self, id: &str) {
        self.crons.retain(|job| job.id != id);
    }

    pub fn crons(&self) -> &[CronJob] {
        &self.crons
    }

    /// Runs `f` once the app is ready (database prepared and set up), on every start.
    pub fn on_ready<F, Fut>(&mut self, f: F)
    where
        F: Fn(Arc<Core>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.on_ready.push(Arc::new(move |core| Box::pin(f(core))));
    }

    pub(crate) async fn ready(&self, core: &Arc<Core>) {
        for f in &self.on_ready {
            f(core.clone()).await;
        }
    }

    /// Fires the after-success hooks of committed changes, in the background.
    pub(crate) fn committed(core: &Arc<Core>, changes: Vec<Change>) {
        let hooks = core.hooks.clone();
        let changes: Vec<Change> = changes
            .into_iter()
            .filter(|change| {
                hooks
                    .after_hook(change.action)
                    .has_handlers(Some(&change.collection))
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        let core = core.clone();
        tokio::spawn(async move {
            for mut change in changes {
                // hidden fields are included, secrets aren't
                for field in change.collection.fields.iter().filter(|f| f.is_secret()) {
                    change.values.remove(&field.name);
                }
                let e = RecordEvent {
                    core: core.clone(),
                    collection: change.collection.clone(),
                    record: change.values,
                };
                let hook = hooks.after_hook(change.action);
                if let Err(err) = hook
                    .trigger(Some(&change.collection), e, |e| async { Ok(e) })
                    .await
                {
                    tracing::error!(
                        collection = %change.collection.name,
                        error = %err.details.as_deref().unwrap_or(&err.message),
                        "an after-success record hook failed"
                    );
                }
            }
        });
    }

    fn after_hook(&self, action: crate::realtime::Action) -> &Hook<RecordEvent> {
        use crate::realtime::Action;
        match action {
            Action::Create => &self.on_record_after_create_success,
            Action::Update => &self.on_record_after_update_success,
            Action::Delete => &self.on_record_after_delete_success,
        }
    }
}

/// A record change of a transaction, reported to the after-success hooks once it commits.
pub(crate) struct Change {
    pub collection: Arc<Collection>,
    pub action: crate::realtime::Action,
    pub values: Map<String, Value>,
}

/// Runs the custom cron jobs whose schedule fires this minute. Only one instance runs
/// each job each minute (claimed in the params table).
pub(crate) async fn run_scheduled_crons(core: &Arc<Core>) {
    let now = chrono::Utc::now();
    let minute = now.format("%Y-%m-%dT%H:%M").to_string();
    for job in core.hooks.crons.iter().filter(|j| j.schedule.matches(&now)) {
        let claimed = async {
            let client = core
                .pool
                .get()
                .await
                .map_err(crate::db::describe_pool_error)?;
            let rows = client
                .execute(
                    "INSERT INTO _slonbase.params (key, value) VALUES ($1, to_jsonb($2::text))
                     ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated = now()
                     WHERE _slonbase.params.value <> EXCLUDED.value",
                    &[&format!("cron_last:{}", job.id), &minute],
                )
                .await?;
            anyhow::Ok(rows == 1)
        };
        match claimed.await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(err) => {
                tracing::warn!(job = %job.id, error = %format!("{err:#}"), "failed to schedule a cron job");
                continue;
            }
        }
        let (core, run, id) = (core.clone(), job.run.clone(), job.id.clone());
        tokio::spawn(async move {
            if let Err(err) = run(core).await {
                tracing::error!(job = %id, error = %err.details.as_deref().unwrap_or(&err.message), "a cron job failed");
            }
        });
    }
}

/// Runs the [`Hooks::on_request`] handlers around the router.
pub(crate) async fn request_middleware(
    axum::extract::State(app): axum::extract::State<Arc<crate::app::App>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    use axum::extract::FromRequestParts;
    use axum::response::IntoResponse;

    let Ok(core) = app.core() else {
        return next.run(req).await;
    };
    let hook = &core.hooks.on_request;
    if !hook.has_handlers(None) {
        return next.run(req).await;
    }
    let (mut parts, body) = req.into_parts();
    let auth = RequestAuth::from_request_parts(&mut parts, &app)
        .await
        .unwrap_or_default();
    let e = RequestEvent {
        core: core.clone(),
        auth,
        request: Some(axum::extract::Request::from_parts(parts, body)),
        response: None,
    };
    let result = hook
        .trigger(None, e, move |mut e| async move {
            if let Some(req) = e.request.take() {
                e.response = Some(next.run(req).await);
            }
            Ok(e)
        })
        .await;
    match result {
        Ok(e) => e
            .response
            .unwrap_or_else(|| axum::http::StatusCode::OK.into_response()),
        Err(err) => err.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chain_order_and_short_circuit() {
        let mut hook: Hook<Vec<&'static str>> = Hook::default();
        hook.add(&[], |mut e, next| async move {
            e.push("a");
            let mut e = next.run(e).await?;
            e.push("a after");
            Ok(e)
        });
        hook.add(&["posts"], |mut e, next| async move {
            e.push("b");
            next.run(e).await
        });
        let e = hook
            .trigger(None, vec![], |mut e| async move {
                e.push("action");
                Ok(e)
            })
            .await
            .unwrap();
        // "b" is limited to a collection
        assert_eq!(e, vec!["a", "action", "a after"]);

        let mut stop: Hook<Vec<&'static str>> = Hook::default();
        stop.add(&[], |_, _| async { Err(ApiError::bad_request("no")) });
        let err = stop
            .trigger(None, vec![], |_| async {
                panic!("the action must not run")
            })
            .await
            .unwrap_err();
        assert_eq!(err.message, "no");
    }
}
