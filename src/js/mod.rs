//! JavaScript hooks: `*.pb.js` files in the hooks directory, with PocketBase's JSVM API
//! (`routerAdd`, `onRecordCreateRequest`, `cronAdd`, `$app`, `$http`, ...; see
//! `prelude.js`), run by QuickJS.
//!
//! A pool of threads each owns a runtime that loaded every hook file, so any of them can
//! run any handler. A handler runs on one thread from start to end; when it calls
//! `e.next()`, the thread sends the event back to the async side, which runs the rest of
//! the hook chain (and the action) and resumes the thread with the result. Native calls
//! (`$app.findRecordById`, `$http.send`, ...) block their thread on the async runtime.

pub mod migrate;
mod native;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, RwLock, Weak, mpsc};

use axum::extract::{FromRequestParts, RawPathParams, Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rquickjs::{CatchResultExt, Context, Function, Runtime};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::Extensions;
use crate::app::App;
use crate::auth::RequestAuth;
use crate::collections::api::to_json as collection_json;
use crate::collections::model::Collection;
use crate::config::HooksConfig;
use crate::core::Core;
use crate::error::ApiError;
use crate::filter::RequestInfo;
use crate::hooks::{
    AuthRequestEvent, BoxFuture, CollectionEvent, HookResult, MailerEvent, Next, RecordEvent,
    RecordRequestEvent, RecordsListEvent, RequestEvent,
};
use crate::records::files::Upload;

const PRELUDE: &str = include_str!("prelude.js");
/// Largest body a JavaScript route reads.
const MAX_ROUTE_BODY: usize = 32 << 20;

/// What the runtimes need outside JavaScript.
pub(crate) struct Shared {
    handle: tokio::runtime::Handle,
    core: RwLock<Weak<Core>>,
    dir: PathBuf,
}

impl Shared {
    fn core(&self) -> Result<Arc<Core>, ApiError> {
        self.core
            .read()
            .expect("core lock")
            .upgrade()
            .ok_or_else(|| ApiError::unavailable("slonbase isn't ready yet."))
    }
}

enum FromJs {
    Next(String),
    Done(String),
}

struct Job {
    index: usize,
    event: String,
    /// Files the handler can use (`e.findUploadedFiles()`), by token.
    uploads: Vec<(String, Upload)>,
    out: tokio::sync::mpsc::UnboundedSender<FromJs>,
    resume: mpsc::Receiver<String>,
}

type NextFn = Box<dyn FnOnce(Value) -> BoxFuture<Result<Value, ApiError>> + Send>;

#[derive(Clone)]
struct Pool {
    jobs: mpsc::Sender<Job>,
    workers: Arc<Workers>,
}

/// Most runtimes, the extra ones included.
const MAX_RUNTIMES: usize = 256;
/// How long an extra runtime waits for work before it stops.
const EXTRA_IDLE: std::time::Duration = std::time::Duration::from_secs(30);

/// The runtime threads. Handlers nest (a `routerUse` handler waits in `e.next()` while the
/// route's handler runs, which may save a record that runs a model hook...), so when every
/// runtime is busy or spoken for, an extra one starts for the job instead of it waiting on
/// a runtime that waits on it.
struct Workers {
    shared: Arc<Shared>,
    files: Arc<Vec<(String, String)>>,
    jobs: Arc<Mutex<mpsc::Receiver<Job>>>,
    /// Runtimes waiting for a job.
    idle: std::sync::atomic::AtomicUsize,
    /// Jobs sent and not yet picked up.
    queued: std::sync::atomic::AtomicUsize,
    /// Running runtimes.
    total: std::sync::atomic::AtomicUsize,
}

impl Workers {
    fn spawn(
        self: &Arc<Self>,
        name: String,
        report: Option<tokio::sync::oneshot::Sender<Result<String, String>>>,
        idle_timeout: Option<std::time::Duration>,
    ) -> std::io::Result<()> {
        use std::sync::atomic::Ordering;
        self.total.fetch_add(1, Ordering::SeqCst);
        let workers = self.clone();
        let spawned = std::thread::Builder::new()
            .name(name)
            .spawn(move || worker(workers, report, idle_timeout));
        if spawned.is_err() {
            self.total.fetch_sub(1, Ordering::SeqCst);
        }
        spawned.map(|_| ())
    }

    /// Starts an extra runtime when no idle one is left for the queued jobs.
    fn grow_if_needed(self: &Arc<Self>) {
        use std::sync::atomic::Ordering;
        if self.idle.load(Ordering::SeqCst) >= self.queued.load(Ordering::SeqCst)
            || self.total.load(Ordering::SeqCst) >= MAX_RUNTIMES
        {
            return;
        }
        let n = self.total.load(Ordering::SeqCst);
        if let Err(err) = self.spawn(format!("slonbase-js-extra-{n}"), None, Some(EXTRA_IDLE)) {
            tracing::warn!(error = %err, "failed to start an extra JavaScript runtime");
        }
    }
}

/// `{"ok": value}` or `{"err": {status, message, data}}`, as the prelude expects.
fn envelope(result: Result<Value, ApiError>) -> String {
    match result {
        Ok(value) => json!({ "ok": value }).to_string(),
        Err(err) => json!({ "err": {
            "status": err.status.as_u16(),
            "message": err.message,
            "data": err.data,
        } })
        .to_string(),
    }
}

fn from_envelope(raw: &str) -> Result<Value, ApiError> {
    let mut value: Value = serde_json::from_str(raw)
        .map_err(|e| ApiError::internal(anyhow::anyhow!("invalid hook result: {e}")))?;
    if let Some(ok) = value.get_mut("ok") {
        return Ok(ok.take());
    }
    let err = value.get("err").cloned().unwrap_or_default();
    let status = err["status"]
        .as_u64()
        .and_then(|s| StatusCode::from_u16(s as u16).ok())
        .filter(|s| s.is_client_error() || s.is_server_error())
        .unwrap_or(StatusCode::BAD_REQUEST);
    let mut error = ApiError::new(status, err["message"].as_str().unwrap_or_default());
    if let Value::Object(data) = &err["data"] {
        error.data = data.clone();
    }
    if let Some(details) = err["details"].as_str() {
        tracing::error!(error = %details, "a JavaScript hook failed");
        error.details = Some(Box::new(details.to_string()));
    }
    Err(error)
}

impl Pool {
    /// Runs handler `index` with the event; `next` runs when the handler calls `e.next()`.
    async fn run(
        &self,
        index: usize,
        event: Value,
        next: Option<NextFn>,
    ) -> Result<Value, ApiError> {
        self.run_with(index, event, next, Vec::new()).await
    }

    /// [`Pool::run`] with uploaded files for the handler.
    async fn run_with(
        &self,
        index: usize,
        event: Value,
        next: Option<NextFn>,
        uploads: Vec<(String, Upload)>,
    ) -> Result<Value, ApiError> {
        let (out, mut from_js) = tokio::sync::mpsc::unbounded_channel();
        let (resume_tx, resume) = mpsc::channel();
        self.workers
            .queued
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.jobs
            .send(Job {
                index,
                event: event.to_string(),
                uploads,
                out,
                resume,
            })
            .map_err(|_| ApiError::internal(anyhow::anyhow!("the JavaScript runtimes stopped")))?;
        self.workers.grow_if_needed();
        let mut next = next;
        loop {
            match from_js.recv().await {
                Some(FromJs::Next(state)) => {
                    let state = serde_json::from_str(&state).unwrap_or_default();
                    let result = match next.take() {
                        Some(next) => next(state).await,
                        None => Err(ApiError::internal(anyhow::anyhow!(
                            "e.next() was called more than once"
                        ))),
                    };
                    let _ = resume_tx.send(envelope(result));
                }
                Some(FromJs::Done(result)) => return from_envelope(&result),
                None => {
                    return Err(ApiError::internal(anyhow::anyhow!(
                        "a JavaScript runtime stopped"
                    )));
                }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct Registration {
    kind: String,
    #[serde(default)]
    index: usize,
    #[serde(default)]
    collections: Vec<String>,
    #[serde(default)]
    method: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    expression: String,
    /// Mail hooks: the kind of email (`verification`, ...), or empty for all.
    #[serde(default)]
    mail: String,
}

/// The running job's channels, used by `e.next()`.
type JobSlot = Rc<
    RefCell<
        Option<(
            tokio::sync::mpsc::UnboundedSender<FromJs>,
            mpsc::Receiver<String>,
        )>,
    >,
>;

/// Creates a runtime with the prelude and the hook files, returning the registrations.
fn start_runtime(
    shared: &Arc<Shared>,
    files: &[(String, String)],
    slot: &JobSlot,
) -> Result<(Runtime, Context, String), String> {
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_max_stack_size(4 << 20);
    let ctx = Context::full(&rt).map_err(|e| e.to_string())?;
    let registrations = ctx.with(|ctx| -> Result<String, String> {
        let globals = ctx.globals();
        let js = |e: rquickjs::Error| e.to_string();
        let call_shared = shared.clone();
        globals
            .set(
                "__native_call",
                Function::new(ctx.clone(), move |name: String, args: String| {
                    native::call(&call_shared, &name, &args)
                })
                .map_err(js)?,
            )
            .map_err(js)?;
        let next_slot = slot.clone();
        globals
            .set(
                "__native_next",
                Function::new(ctx.clone(), move |state: String| -> String {
                    let slot = next_slot.borrow();
                    let Some((out, resume)) = slot.as_ref() else {
                        return envelope(Err(ApiError::bad_request(
                            "e.next() isn't available here.",
                        )));
                    };
                    if out.send(FromJs::Next(state)).is_err() {
                        return envelope(Err(ApiError::bad_request("The request was cancelled.")));
                    }
                    resume.recv().unwrap_or_else(|_| {
                        envelope(Err(ApiError::bad_request("The request was cancelled.")))
                    })
                })
                .map_err(js)?,
            )
            .map_err(js)?;
        globals
            .set(
                "__native_log",
                Function::new(ctx.clone(), |level: String, message: String| {
                    match level.as_str() {
                        "error" => tracing::error!(target: "slonbase::js", "{message}"),
                        "warn" => tracing::warn!(target: "slonbase::js", "{message}"),
                        "debug" => tracing::debug!(target: "slonbase::js", "{message}"),
                        _ => tracing::info!(target: "slonbase::js", "{message}"),
                    }
                })
                .map_err(js)?,
            )
            .map_err(js)?;
        globals
            .set("__hooks", shared.dir.to_string_lossy().to_string())
            .map_err(js)?;
        ctx.eval::<(), _>(PRELUDE)
            .catch(&ctx)
            .map_err(|e| format!("prelude: {e}"))?;
        for (name, source) in files {
            let mut options = rquickjs::context::EvalOptions::default();
            options.global = true;
            options.strict = false;
            options.filename = Some(name.clone());
            ctx.eval_with_options::<(), _>(source.as_str(), options)
                .catch(&ctx)
                .map_err(|e| format!("{name}: {e}"))?;
        }
        ctx.eval::<String, _>("__registrations()")
            .catch(&ctx)
            .map_err(|e| e.to_string())
    })?;
    Ok((rt, ctx, registrations))
}

fn worker(
    workers: Arc<Workers>,
    report: Option<tokio::sync::oneshot::Sender<Result<String, String>>>,
    idle_timeout: Option<std::time::Duration>,
) {
    use std::sync::atomic::Ordering;
    struct Running<'a>(&'a Workers);
    impl Drop for Running<'_> {
        fn drop(&mut self) {
            self.0.total.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let _running = Running(&workers);
    let (shared, files) = (&workers.shared, &workers.files);
    let slot: JobSlot = Rc::new(RefCell::new(None));
    let (rt, ctx) = match start_runtime(shared, files, &slot) {
        Ok((rt, ctx, registrations)) => {
            if let Some(report) = report {
                let _ = report.send(Ok(registrations));
            }
            (rt, ctx)
        }
        Err(err) => {
            match report {
                Some(report) => {
                    let _ = report.send(Err(err));
                }
                None => tracing::error!(error = %err, "failed to start a JavaScript runtime"),
            }
            return;
        }
    };
    loop {
        workers.idle.fetch_add(1, Ordering::SeqCst);
        let received = {
            let jobs = workers.jobs.lock().expect("jobs lock");
            match idle_timeout {
                Some(timeout) => jobs.recv_timeout(timeout).ok(),
                None => jobs.recv().ok(),
            }
        };
        workers.idle.fetch_sub(1, Ordering::SeqCst);
        let Some(job) = received else {
            return; // the pool is gone, or an extra runtime had nothing to do
        };
        workers.queued.fetch_sub(1, Ordering::SeqCst);
        *slot.borrow_mut() = Some((job.out.clone(), job.resume));
        native::set_uploads(job.uploads);
        let result = ctx.with(|ctx| {
            let run = || -> rquickjs::Result<String> {
                let dispatch: Function = ctx.globals().get("__dispatch")?;
                dispatch.call((job.index as u32, job.event))
            };
            run().catch(&ctx).unwrap_or_else(|e| {
                envelope(Err(ApiError::internal(anyhow::anyhow!(
                    "JavaScript hook: {e}"
                ))))
            })
        });
        // promises created by the handler
        while rt.is_job_pending() {
            if rt.execute_pending_job().is_err() {
                break;
            }
        }
        slot.borrow_mut().take();
        native::set_uploads(Vec::new());
        let _ = job.out.send(FromJs::Done(result));
    }
}

/// Loads the hook files of `config.dir` (if any) and registers their handlers.
pub async fn load(config: &HooksConfig, extensions: &mut Extensions) -> anyhow::Result<()> {
    let dir = &config.dir;
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".pb.js") && entry.path().is_file() {
                let source = std::fs::read_to_string(entry.path()).map_err(|e| {
                    anyhow::anyhow!("failed to read {}: {e}", entry.path().display())
                })?;
                files.push((name, source));
            }
        }
    }
    if files.is_empty() {
        return Ok(());
    }
    files.sort();
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    tracing::info!(dir = %dir.display(), files = ?names, "loading JavaScript hooks");

    let shared = Arc::new(Shared {
        handle: tokio::runtime::Handle::current(),
        core: RwLock::new(Weak::new()),
        dir: std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()),
    });
    let (jobs_tx, jobs_rx) = mpsc::channel();
    let workers = Arc::new(Workers {
        shared: shared.clone(),
        files: Arc::new(files),
        jobs: Arc::new(Mutex::new(jobs_rx)),
        idle: Default::default(),
        queued: Default::default(),
        total: Default::default(),
    });
    let (report_tx, report_rx) = tokio::sync::oneshot::channel();
    let mut report = Some(report_tx);
    for i in 0..config.pool_size.clamp(1, MAX_RUNTIMES) {
        workers.spawn(format!("slonbase-js-{i}"), report.take(), None)?;
    }
    let registrations = report_rx
        .await
        .map_err(|_| anyhow::anyhow!("the JavaScript runtime stopped"))?
        .map_err(|e| anyhow::anyhow!("failed to load the JavaScript hooks: {e}"))?;
    let registrations: Vec<Registration> = serde_json::from_str(&registrations)?;
    register(
        Pool {
            jobs: jobs_tx,
            workers,
        },
        shared,
        registrations,
        extensions,
    )
}

fn register(
    pool: Pool,
    shared: Arc<Shared>,
    registrations: Vec<Registration>,
    extensions: &mut Extensions,
) -> anyhow::Result<()> {
    let hooks = &mut extensions.hooks;
    // the natives need the core; set before any JavaScript `onBootstrap` handler runs
    hooks.on_ready(move |core| {
        *shared.core.write().expect("core lock") = Arc::downgrade(&core);
        async {}
    });

    let mut routes: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    for reg in registrations {
        let (index, pool) = (reg.index, pool.clone());
        let request_hook = match reg.kind.as_str() {
            "recordCreateRequest" => Some((&mut hooks.on_record_create_request, false)),
            "recordUpdateRequest" => Some((&mut hooks.on_record_update_request, true)),
            "recordDeleteRequest" => Some((&mut hooks.on_record_delete_request, true)),
            "recordViewRequest" => Some((&mut hooks.on_record_view_request, true)),
            _ => None,
        };
        if let Some((hook, stored)) = request_hook {
            hook.add_boxed(
                reg.collections,
                Arc::new(move |e, next| {
                    Box::pin(record_request(pool.clone(), index, e, next, stored))
                }),
            );
            continue;
        }
        let model_hook = match reg.kind.as_str() {
            "recordCreate" => Some((&mut hooks.on_record_create, false)),
            "recordUpdate" => Some((&mut hooks.on_record_update, true)),
            "recordDelete" => Some((&mut hooks.on_record_delete, true)),
            "recordValidate" => Some((&mut hooks.on_record_validate, true)),
            _ => None,
        };
        if let Some((hook, stored)) = model_hook {
            hook.add_boxed(
                reg.collections,
                Arc::new(move |e, next| {
                    Box::pin(model_event(pool.clone(), index, e, next, stored))
                }),
            );
            continue;
        }
        let collection_hook = match reg.kind.as_str() {
            "collectionCreate" => Some(&mut hooks.on_collection_create),
            "collectionUpdate" => Some(&mut hooks.on_collection_update),
            "collectionDelete" => Some(&mut hooks.on_collection_delete),
            "collectionAfterCreateSuccess" => Some(&mut hooks.on_collection_after_create_success),
            "collectionAfterUpdateSuccess" => Some(&mut hooks.on_collection_after_update_success),
            "collectionAfterDeleteSuccess" => Some(&mut hooks.on_collection_after_delete_success),
            _ => None,
        };
        if let Some(hook) = collection_hook {
            hook.add_boxed(
                reg.collections,
                Arc::new(move |e, next| Box::pin(collection_event(pool.clone(), index, e, next))),
            );
            continue;
        }
        let auth_hook = match reg.kind.as_str() {
            "recordAuthRequest" => Some(&mut hooks.on_record_auth_request),
            "recordAuthWithPasswordRequest" => {
                Some(&mut hooks.on_record_auth_with_password_request)
            }
            "recordAuthWithOAuth2Request" => Some(&mut hooks.on_record_auth_with_oauth2_request),
            "recordAuthRefreshRequest" => Some(&mut hooks.on_record_auth_refresh_request),
            "recordRequestOTPRequest" => Some(&mut hooks.on_record_request_otp_request),
            "recordAuthWithOTPRequest" => Some(&mut hooks.on_record_auth_with_otp_request),
            "recordRequestVerificationRequest" => {
                Some(&mut hooks.on_record_request_verification_request)
            }
            "recordConfirmVerificationRequest" => {
                Some(&mut hooks.on_record_confirm_verification_request)
            }
            "recordRequestPasswordResetRequest" => {
                Some(&mut hooks.on_record_request_password_reset_request)
            }
            "recordConfirmPasswordResetRequest" => {
                Some(&mut hooks.on_record_confirm_password_reset_request)
            }
            "recordRequestEmailChangeRequest" => {
                Some(&mut hooks.on_record_request_email_change_request)
            }
            "recordConfirmEmailChangeRequest" => {
                Some(&mut hooks.on_record_confirm_email_change_request)
            }
            _ => None,
        };
        if let Some(hook) = auth_hook {
            hook.add_boxed(
                reg.collections,
                Arc::new(move |e, next| Box::pin(auth_request(pool.clone(), index, e, next))),
            );
            continue;
        }
        if reg.kind == "mailerSend" {
            let mail = reg.mail.clone();
            hooks.on_mailer_send.add_boxed(
                reg.collections,
                Arc::new(move |e, next| {
                    Box::pin(mailer_event(pool.clone(), index, mail.clone(), e, next))
                }),
            );
            continue;
        }
        let after_hook = match reg.kind.as_str() {
            "recordAfterCreateSuccess" => Some(&mut hooks.on_record_after_create_success),
            "recordAfterUpdateSuccess" => Some(&mut hooks.on_record_after_update_success),
            "recordAfterDeleteSuccess" => Some(&mut hooks.on_record_after_delete_success),
            _ => None,
        };
        if let Some(hook) = after_hook {
            hook.add_boxed(
                reg.collections,
                Arc::new(move |e, next| Box::pin(record_event(pool.clone(), index, e, next))),
            );
            continue;
        }
        match reg.kind.as_str() {
            "recordsListRequest" => hooks.on_records_list_request.add_boxed(
                reg.collections,
                Arc::new(move |e, next| Box::pin(list_request(pool.clone(), index, e, next))),
            ),
            "routerUse" => hooks.on_request.add_boxed(
                Vec::new(),
                Arc::new(move |e, next| Box::pin(middleware(pool.clone(), index, e, next))),
            ),
            "route" => routes
                .entry(reg.path.clone())
                .or_default()
                .push((reg.method.clone(), index)),
            "cron" => hooks.add_cron(
                &reg.id,
                &reg.expression,
                Arc::new(move |_| {
                    let pool = pool.clone();
                    Box::pin(async move { pool.run(index, json!({}), None).await.map(|_| ()) })
                }),
            )?,
            "cronRemove" => hooks.remove_cron(&reg.id),
            "ready" => hooks.on_ready(move |_| {
                let pool = pool.clone();
                async move {
                    if let Err(err) = pool.run(index, json!({}), None).await {
                        tracing::error!(error = %err.message, "a JavaScript onBootstrap handler failed");
                    }
                }
            }),
            other => anyhow::bail!("unknown hook registration {other:?}"),
        }
    }

    for (path, handlers) in routes {
        let mut router: Option<axum::routing::MethodRouter<Arc<App>>> = None;
        for (method, index) in handlers {
            let pool = pool.clone();
            let handler = move |State(app): State<Arc<App>>, req: Request| {
                let pool = pool.clone();
                async move { route(pool, index, app, req).await }
            };
            let next = if method == "*" || method == "ANY" {
                axum::routing::any(handler)
            } else {
                let parsed = axum::http::Method::from_bytes(method.as_bytes())
                    .map_err(|_| anyhow::anyhow!("routerAdd: invalid method {method:?}"))?;
                let filter = axum::routing::MethodFilter::try_from(parsed)
                    .map_err(|_| anyhow::anyhow!("routerAdd: unsupported method {method:?}"))?;
                axum::routing::on(filter, handler)
            };
            router = Some(match router {
                None => next,
                Some(r) => r.merge(next),
            });
        }
        if let Some(router) = router {
            extensions.route(&axum_path(&path)?, router);
        }
    }
    Ok(())
}

/// PocketBase's route syntax (`/a/{id}`, `/files/{path...}`) in axum's (`/files/{*path}`).
fn axum_path(path: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        path.starts_with('/'),
        "routerAdd: the path must start with / ({path:?})"
    );
    let path = path.split_whitespace().next().unwrap_or(path);
    Ok(path
        .split('/')
        .map(|segment| {
            match segment
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix("...}"))
            {
                Some(name) => format!("{{*{name}}}"),
                None => segment.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("/"))
}

// ---------------------------------------------------------------------------
// events

/// The caller as a record (secrets left out), or null.
fn auth_json(auth: &RequestAuth) -> Value {
    match auth.superuser.as_ref().or(auth.record.as_ref()) {
        Some(record) => record_json(&record.collection, record.values.clone()),
        None => Value::Null,
    }
}

fn record_json(c: &Collection, mut values: Map<String, Value>) -> Value {
    for field in c.fields.iter().filter(|f| f.is_secret()) {
        values.remove(&field.name);
    }
    crate::records::store::record_json(c, values)
}

fn request_info_json(request: &RequestInfo) -> Value {
    json!({
        "method": request.method,
        "query": request.query,
        "headers": request.headers,
        "body": request.body,
    })
}

fn response_from(value: &Value) -> Option<Response> {
    if value.is_null() {
        return None;
    }
    let status = value["status"]
        .as_u64()
        .and_then(|s| StatusCode::from_u16(s as u16).ok())
        .unwrap_or(StatusCode::OK);
    let mut response = if value["hasJson"].as_bool() == Some(true) {
        (status, axum::Json(value["json"].clone())).into_response()
    } else {
        let body = value["body"].as_str().unwrap_or_default().to_string();
        let mut response = (status, body).into_response();
        if let Some(Ok(content_type)) = value["contentType"].as_str().map(HeaderValue::from_str) {
            response
                .headers_mut()
                .insert(axum::http::header::CONTENT_TYPE, content_type);
        }
        response
    };
    if let Value::Object(headers) = &value["headers"] {
        for (name, v) in headers {
            if let (Ok(name), Some(Ok(v))) = (
                HeaderName::from_bytes(name.as_bytes()),
                v.as_str().map(HeaderValue::from_str),
            ) {
                response.headers_mut().insert(name, v);
            }
        }
    }
    Some(response)
}

/// Keys of record states that aren't fields.
const NON_FIELDS: [&str; 3] = ["collectionId", "collectionName", "expand"];

async fn record_request(
    pool: Pool,
    index: usize,
    e: RecordRequestEvent,
    next: Next<RecordRequestEvent>,
    stored: bool,
) -> HookResult<RecordRequestEvent> {
    let event = json!({
        "collection": collection_json(&e.collection),
        "record": e.record,
        "stored": stored,
        "auth": auth_json(&e.auth),
        "requestInfo": request_info_json(&e.request),
    });
    let rebuild = (
        e.core.clone(),
        e.collection.clone(),
        e.auth.clone(),
        e.request.clone(),
    );
    let slot = Arc::new(Mutex::new(Some(e)));
    let next_fn: NextFn = {
        let slot = slot.clone();
        Box::new(move |state: Value| {
            Box::pin(async move {
                let mut e = slot.lock().expect("event lock").take().expect("event");
                if let Some(Value::Object(mut record)) = state.get("record").cloned() {
                    for key in NON_FIELDS {
                        record.remove(key);
                    }
                    e.record = record;
                }
                let e = next.run(e).await?;
                let out = json!({ "record": e.record });
                *slot.lock().expect("event lock") = Some(e);
                Ok(out)
            })
        })
    };
    let state = pool.run(index, event, Some(next_fn)).await?;
    let mut e = slot.lock().expect("event lock").take().unwrap_or_else(|| {
        let (core, collection, auth, request) = rebuild;
        RecordRequestEvent {
            core,
            collection,
            record: Map::new(),
            auth,
            request,
            response: None,
        }
    });
    if let Value::Object(record) = &state["record"] {
        // changes made after `next` reach the response
        for (key, value) in record {
            e.record.insert(key.clone(), value.clone());
        }
    }
    if let Some(response) = response_from(&state["response"]) {
        e.response = Some(response);
    }
    Ok(e)
}

/// An auth request hook (`onRecordAuthRequest`, `onRecordAuthWithPasswordRequest`, ...):
/// changes to the record and the hook's data are used by the action.
async fn auth_request(
    pool: Pool,
    index: usize,
    e: AuthRequestEvent,
    next: Next<AuthRequestEvent>,
) -> HookResult<AuthRequestEvent> {
    let record_of = |e: &AuthRequestEvent| match &e.record {
        Some(r) => record_json(&r.collection, r.values.clone()),
        None => Value::Null,
    };
    let event = json!({
        "collection": collection_json(&e.collection),
        "record": record_of(&e),
        "data": e.data,
        "auth": auth_json(&e.auth),
        "requestInfo": request_info_json(&e.request),
    });
    let rebuild = (
        e.core.clone(),
        e.collection.clone(),
        e.auth.clone(),
        e.request.clone(),
    );
    let slot = Arc::new(Mutex::new(Some(e)));
    let next_fn: NextFn = {
        let slot = slot.clone();
        Box::new(move |state: Value| {
            Box::pin(async move {
                let mut e = slot.lock().expect("event lock").take().expect("event");
                apply_auth_state(&mut e, &state).await?;
                let e = next.run(e).await?;
                let out = json!({ "record": record_of(&e), "data": e.data });
                *slot.lock().expect("event lock") = Some(e);
                Ok(out)
            })
        })
    };
    let state = pool.run(index, event, Some(next_fn)).await?;
    let mut e = slot.lock().expect("event lock").take().unwrap_or_else(|| {
        let (core, collection, auth, request) = rebuild;
        AuthRequestEvent {
            core,
            collection,
            record: None,
            data: Map::new(),
            auth,
            request,
            response: None,
        }
    });
    apply_auth_state(&mut e, &state).await?;
    if let Some(response) = response_from(&state["response"]) {
        e.response = Some(response);
    }
    Ok(e)
}

/// Applies a handler's changes to an auth request event. The record comes back without
/// its secrets: changes to the same record are applied to its values, and another record
/// (found by the handler) is loaded with them.
async fn apply_auth_state(e: &mut AuthRequestEvent, state: &Value) -> Result<(), ApiError> {
    if let Value::Object(data) = &state["data"] {
        for (key, value) in data {
            e.data.insert(key.clone(), value.clone());
        }
    }
    let record = match state.get("record") {
        None => return Ok(()),
        Some(Value::Object(record)) => record,
        Some(_) => {
            e.record = None;
            return Ok(());
        }
    };
    let id = record.get("id").and_then(Value::as_str).unwrap_or_default();
    if let Some(current) = e.record.as_mut().filter(|r| r.id() == id) {
        for field in e.collection.fields.iter().filter(|f| !f.is_secret()) {
            if let Some(value) = record.get(&field.name) {
                current.values.insert(field.name.clone(), value.clone());
            }
        }
        return Ok(());
    }
    let client = e
        .core
        .pool
        .get()
        .await
        .map_err(|err| ApiError::internal(crate::db::describe_pool_error(err)))?;
    let values = crate::records::store::find_by_id(&client, &e.collection, id)
        .await
        .map_err(|err| ApiError::internal(err.into()))?
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "The record {id:?} isn't in the {} collection.",
                e.collection.name
            ))
        })?;
    e.record = Some(crate::auth::AuthRecord {
        collection: e.collection.clone(),
        values,
        refreshable: true,
    });
    Ok(())
}

/// A model hook (`onRecordCreate`, ...): the handler's record changes are kept.
async fn model_event(
    pool: Pool,
    index: usize,
    e: RecordEvent,
    next: Next<RecordEvent>,
    stored: bool,
) -> HookResult<RecordEvent> {
    let event = json!({
        "collection": collection_json(&e.collection),
        "record": e.record,
        "stored": stored,
    });
    let (core, collection) = (e.core.clone(), e.collection.clone());
    let slot = Arc::new(Mutex::new(Some(e)));
    let take_record = |state: &Value| match state.get("record") {
        Some(Value::Object(record)) => {
            let mut record = record.clone();
            for key in NON_FIELDS {
                record.remove(key);
            }
            Some(record)
        }
        _ => None,
    };
    let next_fn: NextFn = {
        let slot = slot.clone();
        Box::new(move |state: Value| {
            Box::pin(async move {
                let mut e = slot.lock().expect("event lock").take().expect("event");
                if let Some(record) = take_record(&state) {
                    e.record = record;
                }
                let e = next.run(e).await?;
                let out = json!({ "record": e.record });
                *slot.lock().expect("event lock") = Some(e);
                Ok(out)
            })
        })
    };
    let state = pool.run(index, event, Some(next_fn)).await?;
    let mut e = slot
        .lock()
        .expect("event lock")
        .take()
        .unwrap_or(RecordEvent {
            core,
            collection,
            record: Map::new(),
        });
    if let Some(record) = take_record(&state) {
        e.record = record;
    }
    Ok(e)
}

/// A collection hook: changes to the collection are kept (create, update).
async fn collection_event(
    pool: Pool,
    index: usize,
    e: CollectionEvent,
    next: Next<CollectionEvent>,
) -> HookResult<CollectionEvent> {
    let event = json!({ "collection": e.collection });
    let core = e.core.clone();
    let slot = Arc::new(Mutex::new(Some(e)));
    let next_fn: NextFn = {
        let slot = slot.clone();
        Box::new(move |state: Value| {
            Box::pin(async move {
                let mut e = slot.lock().expect("event lock").take().expect("event");
                if let Some(Value::Object(c)) = state.get("collection") {
                    e.collection = c.clone();
                }
                let e = next.run(e).await?;
                let out = json!({ "collection": e.collection });
                *slot.lock().expect("event lock") = Some(e);
                Ok(out)
            })
        })
    };
    let state = pool.run(index, event, Some(next_fn)).await?;
    let mut e = slot
        .lock()
        .expect("event lock")
        .take()
        .unwrap_or(CollectionEvent {
            core,
            collection: Map::new(),
        });
    if let Some(Value::Object(c)) = state.get("collection") {
        e.collection = c.clone();
    }
    Ok(e)
}

/// A mail hook (`onMailerSend`, `onMailerRecordVerificationSend`, ...); `mail` limits it
/// to one kind of email. Not calling `e.next()` skips the email.
async fn mailer_event(
    pool: Pool,
    index: usize,
    mail: String,
    e: MailerEvent,
    next: Next<MailerEvent>,
) -> HookResult<MailerEvent> {
    if !mail.is_empty() && mail != e.kind {
        return next.run(e).await;
    }
    let message_json = |m: &crate::mail::Message| json!({ "to": [{ "address": m.to }], "subject": m.subject, "html": m.html });
    let event = json!({
        "message": message_json(&e.message),
        "kind": e.kind,
        "collection": e.collection.as_deref().map(collection_json),
        "record": e.record,
    });
    let apply = |e: &mut MailerEvent, state: &Value| {
        let m = &state["message"];
        if let Some(to) = m["to"][0]["address"].as_str().or(m["to"].as_str()) {
            e.message.to = to.to_string();
        }
        if let Some(subject) = m["subject"].as_str() {
            e.message.subject = subject.to_string();
        }
        if let Some(html) = m["html"].as_str() {
            e.message.html = html.to_string();
        }
    };
    let slot = Arc::new(Mutex::new(Some(e)));
    let next_fn: NextFn = {
        let slot = slot.clone();
        Box::new(move |state: Value| {
            Box::pin(async move {
                let mut e = slot.lock().expect("event lock").take().expect("event");
                apply(&mut e, &state);
                let e = next.run(e).await?;
                let out = json!({ "message": message_json(&e.message) });
                *slot.lock().expect("event lock") = Some(e);
                Ok(out)
            })
        })
    };
    pool.run(index, event, Some(next_fn)).await?;
    slot.lock()
        .expect("event lock")
        .take()
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("the email was lost")))
}

async fn list_request(
    pool: Pool,
    index: usize,
    e: RecordsListEvent,
    next: Next<RecordsListEvent>,
) -> HookResult<RecordsListEvent> {
    let event = json!({
        "collection": collection_json(&e.collection),
        "records": e.records,
        "page": e.page,
        "perPage": e.per_page,
        "totalItems": e.total_items,
        "totalPages": e.total_pages,
        "auth": auth_json(&e.auth),
        "requestInfo": request_info_json(&e.request),
    });
    let apply = |e: &mut RecordsListEvent, state: &Value| {
        if let Value::Array(records) = &state["records"] {
            e.records = records.clone();
        }
        for (key, target) in [
            ("page", &mut e.page),
            ("perPage", &mut e.per_page),
            ("totalItems", &mut e.total_items),
            ("totalPages", &mut e.total_pages),
        ] {
            if let Some(v) = state[key].as_i64() {
                *target = v;
            }
        }
    };
    let rebuild = (
        e.core.clone(),
        e.collection.clone(),
        e.auth.clone(),
        e.request.clone(),
    );
    let slot = Arc::new(Mutex::new(Some(e)));
    let next_fn: NextFn = {
        let slot = slot.clone();
        Box::new(move |state: Value| {
            Box::pin(async move {
                let mut e = slot.lock().expect("event lock").take().expect("event");
                apply(&mut e, &state);
                let e = next.run(e).await?;
                let out = json!({
                    "records": e.records,
                    "page": e.page,
                    "perPage": e.per_page,
                    "totalItems": e.total_items,
                    "totalPages": e.total_pages,
                });
                *slot.lock().expect("event lock") = Some(e);
                Ok(out)
            })
        })
    };
    let state = pool.run(index, event, Some(next_fn)).await?;
    let mut e = slot.lock().expect("event lock").take().unwrap_or_else(|| {
        let (core, collection, auth, request) = rebuild;
        RecordsListEvent {
            core,
            collection,
            records: Vec::new(),
            page: 1,
            per_page: 0,
            total_items: 0,
            total_pages: 0,
            auth,
            request,
            response: None,
        }
    });
    apply(&mut e, &state);
    if let Some(response) = response_from(&state["response"]) {
        e.response = Some(response);
    }
    Ok(e)
}

async fn record_event(
    pool: Pool,
    index: usize,
    e: RecordEvent,
    next: Next<RecordEvent>,
) -> HookResult<RecordEvent> {
    let event = json!({
        "collection": collection_json(&e.collection),
        "record": record_json(&e.collection, e.record.clone()),
    });
    let rebuild = (e.core.clone(), e.collection.clone(), e.record.clone());
    let slot = Arc::new(Mutex::new(Some(e)));
    let next_fn: NextFn = {
        let slot = slot.clone();
        Box::new(move |_| {
            Box::pin(async move {
                let e = slot.lock().expect("event lock").take().expect("event");
                let e = next.run(e).await?;
                *slot.lock().expect("event lock") = Some(e);
                Ok(json!({}))
            })
        })
    };
    pool.run(index, event, Some(next_fn)).await?;
    let e = slot.lock().expect("event lock").take();
    Ok(e.unwrap_or_else(|| {
        let (core, collection, record) = rebuild;
        RecordEvent {
            core,
            collection,
            record,
        }
    }))
}

async fn route(pool: Pool, index: usize, app: Arc<App>, req: Request) -> Response {
    if let Err(err) = app.core() {
        return err.into_response();
    }
    let (mut parts, body) = req.into_parts();
    let auth = match RequestAuth::from_request_parts(&mut parts, &app).await {
        Ok(auth) => auth,
        Err(err) => return err.into_response(),
    };
    let mut path_params = Map::new();
    if let Ok(params) = RawPathParams::from_request_parts(&mut parts, &app).await {
        for (key, value) in &params {
            path_params.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
    let bytes = match axum::body::to_bytes(body, MAX_ROUTE_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "Request entity too large.")
                .into_response();
        }
    };
    let (body, uploads) = match read_body(&parts, bytes).await {
        Ok(read) => read,
        Err(err) => return err.into_response(),
    };
    let mut files: Map<String, Value> = Map::new();
    let mut tokens = Vec::with_capacity(uploads.len());
    for upload in uploads {
        let token = crate::ids::random_id(24);
        let list = files
            .entry(upload.key.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(list) = list {
            list.push(json!({
                "_upload": token,
                "name": upload.original_name,
                "originalName": upload.original_name,
                "size": upload.data.len(),
            }));
        }
        tokens.push((token, upload));
    }
    let mut event = request_event_json(&parts, &auth, body, path_params);
    event["requestInfo"]["files"] = Value::Object(files);
    match pool.run_with(index, event, None, tokens).await {
        Ok(state) => {
            response_from(&state["response"]).unwrap_or_else(|| StatusCode::OK.into_response())
        }
        Err(err) => err.into_response(),
    }
}

/// A request's body for a handler: JSON, a form, or multipart (values and uploads).
async fn read_body(
    parts: &axum::http::request::Parts,
    bytes: axum::body::Bytes,
) -> Result<(Value, Vec<Upload>), ApiError> {
    let content_type = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if bytes.is_empty() {
        return Ok((Value::Object(Map::new()), Vec::new()));
    }
    if content_type.starts_with("application/json") {
        return serde_json::from_slice(&bytes)
            .map(|body| (body, Vec::new()))
            .map_err(|_| ApiError::bad_request("Invalid JSON body."));
    }
    if content_type.starts_with("application/x-www-form-urlencoded") {
        let body = form_urlencoded::parse(&bytes)
            .map(|(k, v)| (k.into_owned(), Value::String(v.into_owned())))
            .collect();
        return Ok((Value::Object(body), Vec::new()));
    }
    if content_type.starts_with("multipart/form-data") {
        let mut req = Request::new(axum::body::Body::from(bytes));
        *req.headers_mut() = parts.headers.clone();
        let body =
            <crate::records::files::RecordBody as axum::extract::FromRequest<()>>::from_request(
                req,
                &(),
            )
            .await?;
        return Ok((Value::Object(body.data), body.uploads));
    }
    Ok((Value::Object(Map::new()), Vec::new()))
}

/// The event of a request handler (`routerAdd`, `routerUse`).
fn request_event_json(
    parts: &axum::http::request::Parts,
    auth: &RequestAuth,
    body: Value,
    path_params: Map<String, Value>,
) -> Value {
    let real_ip = parts
        .extensions
        .get::<crate::guard::ClientIp>()
        .and_then(|ip| ip.0)
        .map(|ip| ip.to_string())
        .unwrap_or_default();
    let raw_query = parts.uri.query().unwrap_or_default().to_string();
    let query: Map<String, Value> = form_urlencoded::parse(raw_query.as_bytes())
        .map(|(k, v)| (k.into_owned(), Value::String(v.into_owned())))
        .collect();
    let headers: Map<String, Value> = parts
        .headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str().ok().map(|v| {
                (
                    k.as_str().to_ascii_lowercase().replace('-', "_"),
                    Value::String(v.to_string()),
                )
            })
        })
        .collect();
    json!({
        "auth": auth_json(auth),
        "requestInfo": {
            "method": parts.method.as_str(),
            "path": parts.uri.path(),
            "rawQuery": raw_query,
            "query": query,
            "headers": headers,
            "body": body,
            "pathParams": path_params,
            "realIP": real_ip,
            "files": {},
        },
    })
}

/// A `routerUse` handler around the rest of the request. JSON and form bodies (up to the
/// routes' limit) are read for `e.requestInfo()` and passed on; other bodies (uploads)
/// stream through unread.
async fn middleware(
    pool: Pool,
    index: usize,
    mut e: RequestEvent,
    next: Next<RequestEvent>,
) -> HookResult<RequestEvent> {
    let Some(req) = e.request.take() else {
        return next.run(e).await;
    };
    let (parts, body) = req.into_parts();
    let content_type = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let readable = content_type.starts_with("application/json")
        || content_type.starts_with("application/x-www-form-urlencoded");
    let (body_json, body) = if readable {
        let bytes = axum::body::to_bytes(body, MAX_ROUTE_BODY)
            .await
            .map_err(|_| {
                ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "Request entity too large.")
            })?;
        let json = match read_body(&parts, bytes.clone()).await {
            Ok((json, _)) => json,
            Err(_) => Value::Object(Map::new()), // the route reports invalid bodies
        };
        (json, axum::body::Body::from(bytes))
    } else {
        (Value::Object(Map::new()), body)
    };
    let event = request_event_json(&parts, &e.auth, body_json, Map::new());
    e.request = Some(Request::from_parts(parts, body));

    let slot = Arc::new(Mutex::new(Some(e)));
    let next_fn: NextFn = {
        let slot = slot.clone();
        Box::new(move |_| {
            Box::pin(async move {
                let e = slot.lock().expect("event lock").take().expect("event");
                let e = next.run(e).await?;
                *slot.lock().expect("event lock") = Some(e);
                Ok(json!({}))
            })
        })
    };
    let state = pool.run(index, event, Some(next_fn)).await?;
    let mut e = slot
        .lock()
        .expect("event lock")
        .take()
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("the request was lost")))?;
    // the handler's own response wins; else the rest of the chain's, with its headers
    if let Some(response) = response_from(&state["response"]) {
        e.response = Some(response);
    } else if let (Some(response), Value::Object(headers)) = (&mut e.response, &state["headers"]) {
        for (name, v) in headers {
            if let (Ok(name), Some(Ok(v))) = (
                HeaderName::from_bytes(name.as_bytes()),
                v.as_str().map(HeaderValue::from_str),
            ) {
                response.headers_mut().insert(name, v);
            }
        }
    }
    e.request = None;
    Ok(e)
}

/// How often the hooks directory is checked for changes.
const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// Most entries looked at in the hooks directory (a guard against huge trees).
const WATCH_MAX_ENTRIES: usize = 10_000;

/// A fingerprint of the hooks directory: every file's path, size and modification time
/// (`node_modules` and hidden entries left out). Empty when the directory doesn't exist.
fn fingerprint(dir: &Path) -> Vec<(PathBuf, u64, Option<std::time::SystemTime>)> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            if out.len() >= WATCH_MAX_ENTRIES {
                return out;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                out.push((entry.path(), meta.len(), meta.modified().ok()));
            }
        }
    }
    out.sort();
    out
}

/// Resolves once the files in `dir` change (and have stopped changing for a moment).
pub async fn changed(dir: PathBuf) {
    let read = |dir: PathBuf| async move {
        tokio::task::spawn_blocking(move || fingerprint(&dir))
            .await
            .unwrap_or_default()
    };
    let initial = read(dir.clone()).await;
    loop {
        tokio::time::sleep(WATCH_INTERVAL).await;
        let mut current = read(dir.clone()).await;
        if current == initial {
            continue;
        }
        // editors write in several steps: wait until it settles
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let next = read(dir.clone()).await;
            if next == current {
                return;
            }
            current = next;
        }
    }
}

/// Whether `path` is inside the hooks directory.
fn inside(dir: &Path, path: &Path) -> bool {
    path.starts_with(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(axum_path("/hello/{name}").unwrap(), "/hello/{name}");
        assert_eq!(axum_path("/files/{path...}").unwrap(), "/files/{*path}");
        assert!(axum_path("hello").is_err());
    }

    #[test]
    fn envelopes() {
        assert_eq!(from_envelope(&envelope(Ok(json!(1)))).unwrap(), json!(1));
        let err =
            from_envelope(r#"{"err":{"status":404,"message":"no","data":{"a":1}}}"#).unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert_eq!(err.data["a"], 1);
        // a status that isn't an error becomes a 400
        let err = from_envelope(r#"{"err":{"status":200,"message":"x"}}"#).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }
}
