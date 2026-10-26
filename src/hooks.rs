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

use crate::auth::{AuthRecord, RequestAuth};
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

/// An auth request (signing in, refreshing a token, verification, password resets, email
/// changes and OTPs), before it's handled. `next` runs the action, which sets `response`;
/// a handler that doesn't call it answers with its own `response` (or an empty 200).
///
/// `data` holds what the hook has besides the record, by PocketBase's names, and changes
/// to it are used by the action:
///
/// | Hook | `data` |
/// |---|---|
/// | `on_record_auth_request` (every sign-in and refresh) | `token`, `meta`, `authMethod` |
/// | `on_record_auth_with_password_request` | `identity`, `identityField`, `password` |
/// | `on_record_auth_with_oauth2_request` | `providerName`, `oauth2User`, `createData`, `isNewRecord` |
/// | `on_record_request_otp_request` | `password` (the one-time password to send) |
/// | `on_record_auth_with_otp_request` | `otp` (`id`, `collectionRef`, `recordRef`, `sentTo`) |
/// | `on_record_request_email_change_request`, `on_record_confirm_email_change_request` | `newEmail` |
pub struct AuthRequestEvent {
    pub core: Arc<Core>,
    pub collection: Arc<Collection>,
    /// The auth record, with its secrets. `None` when no record matched the identity or
    /// email (password sign-ins and OTP requests); a handler can set it, to find records
    /// its own way.
    pub record: Option<AuthRecord>,
    pub data: Map<String, Value>,
    pub auth: RequestAuth,
    pub request: Arc<RequestInfo>,
    pub response: Option<Response>,
}

impl AuthRequestEvent {
    /// A string of `data` (empty when missing).
    pub fn str(&self, key: &str) -> &str {
        self.data
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
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

/// A collection change, before it's applied (create, update, delete) or after it was.
pub struct CollectionEvent {
    pub core: Arc<Core>,
    /// The collection as JSON (with secrets before the change, masked after it). Changes
    /// made by a create or update handler are applied and validated like the request.
    pub collection: Map<String, Value>,
}

/// A record about to be sent to a client: in record responses (lists, views, creates,
/// updates, batches and auth responses), as an expanded relation and in realtime messages.
/// `record` is the record as the client sees it (hidden fields left out, with `expand`);
/// changes to it are sent.
pub struct RecordEnrichEvent {
    pub core: Arc<Core>,
    pub collection: Arc<Collection>,
    pub record: Map<String, Value>,
    /// The request the record is sent for (for realtime, the subscriber's auth).
    pub request: Arc<RequestInfo>,
}

/// An email about to be sent (`next` sends it; a handler that doesn't call it skips it).
pub struct MailerEvent {
    pub message: crate::mail::Message,
    /// What the email is for: `verification`, `passwordReset`, `emailChange`, `otp`,
    /// `authAlert`, or empty for other emails (test emails, hooks).
    pub kind: String,
    /// The auth collection and record of an auth email.
    pub collection: Option<Arc<Collection>>,
    /// The record without its secrets (password hash, token key).
    pub record: Option<Map<String, Value>>,
}

/// A record that was created, updated or deleted (after its transaction committed).
pub struct RecordEvent {
    pub core: Arc<Core>,
    pub collection: Arc<Collection>,
    /// All values of the record, including hidden fields (but not password hashes and
    /// token keys).
    pub record: Map<String, Value>,
}

/// An event of the realtime, file, settings, backup and terminate hooks: the request (when
/// there is one) and the event's values by PocketBase's names in `data`, which handlers can
/// read and change. `next` runs the action, which uses `data`; a request handler that
/// doesn't call it answers with its own `response` (or an empty 200).
///
/// | Hook | `data` |
/// |---|---|
/// | `on_realtime_connect_request` | `client` (`id`, `subscriptions`, `auth`), `idleTimeout`, `maxTimeout` (nanoseconds, like Go's durations; 0 = none) |
/// | `on_realtime_subscribe_request` | `client`, `subscriptions` (the topics to set) |
/// | `on_realtime_message_send` | `client`, `message` (`name`, `data` as text) |
/// | `on_file_download_request` | `record`, `fileField`, `servedPath`, `servedName`, `thumbError` |
/// | `on_file_token_request` | `record` (the caller), `token` |
/// | `on_settings_list_request` | `settings` (secrets masked) |
/// | `on_settings_update_request` | `oldSettings`, `newSettings` (what's saved) |
/// | `on_settings_reload` | `settings` |
/// | `on_backup_create`, `on_backup_restore` | `name`, `exclude` |
/// | `on_terminate` | `isRestart` |
/// | `on_collections_list_request` | `collections` (the page), `result` (`page`, `perPage`, `totalItems`, `totalPages`, `items`: what's sent) |
/// | `on_collection_view_request`, `on_collection_delete_request` | `collection` |
/// | `on_collection_create_request`, `on_collection_update_request` | `collection` (what's saved; after `next`, as saved) |
/// | `on_collections_import_request` | `collectionsData`, `deleteMissing` |
/// | `on_batch_request` | `batch` (the requests: `method`, `url`, `headers`, `body`) |
pub struct DataEvent {
    pub core: Arc<Core>,
    /// The collection of file hooks (handlers limited to collections see only theirs).
    pub collection: Option<Arc<Collection>>,
    pub auth: RequestAuth,
    /// The request, for request hooks (realtime, files, settings).
    pub request: Option<Arc<RequestInfo>>,
    pub data: Map<String, Value>,
    pub response: Option<Response>,
}

impl DataEvent {
    /// An event without a request (settings reloads, backups, terminate).
    pub fn new(core: &Arc<Core>, data: Map<String, Value>) -> Self {
        Self {
            core: core.clone(),
            collection: None,
            auth: RequestAuth::default(),
            request: None,
            data,
            response: None,
        }
    }

    /// A string of `data` (empty when missing).
    pub fn str(&self, key: &str) -> &str {
        self.data
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// `data` from `(key, value)` pairs.
pub fn data<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
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
    /// Every sign-in (password, OTP, OAuth2), token refresh and impersonation, before the
    /// token and record are sent back (and before the MFA check).
    pub on_record_auth_request: Hook<AuthRequestEvent>,
    pub on_record_auth_with_password_request: Hook<AuthRequestEvent>,
    pub on_record_auth_with_oauth2_request: Hook<AuthRequestEvent>,
    pub on_record_auth_refresh_request: Hook<AuthRequestEvent>,
    pub on_record_request_otp_request: Hook<AuthRequestEvent>,
    pub on_record_auth_with_otp_request: Hook<AuthRequestEvent>,
    pub on_record_request_verification_request: Hook<AuthRequestEvent>,
    pub on_record_confirm_verification_request: Hook<AuthRequestEvent>,
    pub on_record_request_password_reset_request: Hook<AuthRequestEvent>,
    pub on_record_confirm_password_reset_request: Hook<AuthRequestEvent>,
    pub on_record_request_email_change_request: Hook<AuthRequestEvent>,
    pub on_record_confirm_email_change_request: Hook<AuthRequestEvent>,
    /// Model hooks: every create, update and delete (the API, batches and the record
    /// service), before it's written, inside its transaction. Changes to `record` are
    /// written; an error aborts the write.
    pub on_record_create: Hook<RecordEvent>,
    pub on_record_update: Hook<RecordEvent>,
    pub on_record_delete: Hook<RecordEvent>,
    /// Runs after a record's own validation passed, before it's written (create and
    /// update); an error rejects the write.
    pub on_record_validate: Hook<RecordEvent>,
    /// Collection changes through the API, before they're applied; an error cancels them.
    pub on_collection_create: Hook<CollectionEvent>,
    pub on_collection_update: Hook<CollectionEvent>,
    pub on_collection_delete: Hook<CollectionEvent>,
    /// After a collection change was applied (run in the background).
    pub on_collection_after_create_success: Hook<CollectionEvent>,
    pub on_collection_after_update_success: Hook<CollectionEvent>,
    pub on_collection_after_delete_success: Hook<CollectionEvent>,
    /// Every record sent to a client (see [`RecordEnrichEvent`]).
    pub on_record_enrich: Hook<RecordEnrichEvent>,
    /// Every email (handlers limited to collections see only auth emails of those).
    pub on_mailer_send: Hook<MailerEvent>,
    pub on_record_after_create_success: Hook<RecordEvent>,
    pub on_record_after_update_success: Hook<RecordEvent>,
    pub on_record_after_delete_success: Hook<RecordEvent>,
    /// A realtime connection being opened: `next` registers the client and opens the
    /// stream, and returns right away (in PocketBase it returns when the connection
    /// closes). A handler that doesn't call it refuses the connection.
    pub on_realtime_connect_request: Hook<DataEvent>,
    /// A client setting its subscriptions: `next` applies `subscriptions`.
    pub on_realtime_subscribe_request: Hook<DataEvent>,
    /// Every message sent to a realtime client, `PB_CONNECT` included: `next` sends
    /// `message`; a handler that doesn't call it skips the message, and an error closes
    /// the connection.
    pub on_realtime_message_send: Hook<DataEvent>,
    /// A file download (handlers can be limited to collections): `next` serves
    /// `servedPath` (a storage key) as `servedName`.
    pub on_file_download_request: Hook<DataEvent>,
    /// A file token request (handlers can be limited to the caller's collection): `next`
    /// answers with `token`.
    pub on_file_token_request: Hook<DataEvent>,
    pub on_settings_list_request: Hook<DataEvent>,
    /// `next` saves `newSettings` (validated like the request).
    pub on_settings_update_request: Hook<DataEvent>,
    /// After the settings changed: an update here or on another instance, a restore, and
    /// once at startup.
    pub on_settings_reload: Hook<DataEvent>,
    /// Every backup: the API, automatic backups and hooks. `next` creates it as `name`.
    pub on_backup_create: Hook<DataEvent>,
    /// Before a restore; `next` restores `name`.
    pub on_backup_restore: Hook<DataEvent>,
    /// When the server stops (after it stops taking requests).
    pub on_terminate: Hook<DataEvent>,
    /// The collections API (superusers): `next` answers the request.
    pub on_collections_list_request: Hook<DataEvent>,
    pub on_collection_view_request: Hook<DataEvent>,
    pub on_collection_create_request: Hook<DataEvent>,
    pub on_collection_update_request: Hook<DataEvent>,
    pub on_collection_delete_request: Hook<DataEvent>,
    pub on_collections_import_request: Hook<DataEvent>,
    /// A batch request, before its requests run: `next` runs `batch`.
    pub on_batch_request: Hook<DataEvent>,
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

/// Runs the [`Hooks::on_terminate`] handlers, for at most 5 seconds so a stuck one can't
/// hold up the stop.
pub(crate) async fn terminate(core: &Arc<Core>, is_restart: bool) {
    let hook = &core.hooks.on_terminate;
    if !hook.has_handlers(None) {
        return;
    }
    let e = DataEvent::new(core, data([("isRestart", is_restart.into())]));
    let run = hook.trigger(None, e, |e| async { Ok(e) });
    match tokio::time::timeout(std::time::Duration::from_secs(5), run).await {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => tracing::error!(
            error = %err.details.as_deref().unwrap_or(&err.message),
            "an onTerminate handler failed"
        ),
        Err(_) => tracing::warn!("the onTerminate handlers didn't finish within 5 seconds"),
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
