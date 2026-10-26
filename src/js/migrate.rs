//! JavaScript migrations, compatible with PocketBase's `pb_migrations`: `*.js` files that
//! call `migrate((app) => { ... }, (app) => { ... })`, applied in file name order.
//!
//! Each migration runs in a transaction of its own, so it applies completely or not at all.
//! The database calls it makes (`app.save()`, `app.findRecordById()`, `app.db()`, ...) run
//! in that transaction: on a dedicated JavaScript runtime, the native calls are routed to
//! the transaction's task. Every call runs in a savepoint, so a failed call that the
//! migration catches leaves the transaction usable. Applied files are kept in
//! `_slonbase.app_migrations`; an advisory lock makes several instances apply each once.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context as _, Result, bail};
use deadpool_postgres::Transaction;
use rquickjs::{CatchResultExt, Context, Function, Runtime};
use serde_json::{Value, json};

use super::native::{Args, bad, internal, take_files};
use super::{PRELUDE, Shared};
use crate::collections::api::to_json as collection_json;
use crate::collections::model::Collection;
use crate::collections::store::{self, Registry};
use crate::core::Core;
use crate::error::ApiError;
use crate::records::api::FileEffects;
use crate::records::service;
use crate::records::txtask::{BoxFuture, TxHandle};
use crate::settings::Settings;

/// Advisory lock key for applying migrations ("slonmigr" in ASCII).
const MIGRATIONS_LOCK_KEY: i64 = 0x736c_6f6e_6d69_6772;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Up,
    Down,
}

/// What the migration running on this thread sees and changed.
#[derive(Clone)]
pub(super) struct TxContext {
    core: Arc<Core>,
    tx: TxHandle,
    state: Arc<Mutex<TxState>>,
}

struct TxState {
    /// The collections as the transaction sees them.
    registry: Arc<Registry>,
    collections_changed: bool,
    /// The settings, once the migration saved them.
    settings: Option<Settings>,
    /// Collections deleted, whose files go once the migration commits.
    deleted: Vec<Arc<Collection>>,
}

thread_local! {
    static CURRENT: RefCell<Option<TxContext>> = const { RefCell::new(None) };
}

/// The migration running on this thread, if any.
pub(super) fn current() -> Option<TxContext> {
    CURRENT.with(|c| c.borrow().clone())
}

fn db_error(e: tokio_postgres::Error) -> ApiError {
    let message = e
        .as_db_error()
        .map(|db| db.message().to_string())
        .unwrap_or_else(|| e.to_string());
    ApiError::bad_request(message)
}

impl TxContext {
    fn registry(&self) -> Arc<Registry> {
        self.state.lock().expect("state lock").registry.clone()
    }

    fn collection(&self, id_or_name: &str) -> Result<Arc<Collection>, ApiError> {
        self.registry().get(id_or_name).cloned().ok_or_else(|| {
            ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "Missing collection context.",
            )
        })
    }

    fn settings(&self) -> Settings {
        let state = self.state.lock().expect("state lock");
        match &state.settings {
            Some(s) => s.clone(),
            None => self.core.settings().as_ref().clone(),
        }
    }

    /// Runs `f` in the migration's transaction, in a savepoint that's rolled back if it fails.
    fn run<T, F>(&self, handle: &tokio::runtime::Handle, f: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: for<'a> FnOnce(
                &'a Transaction<'a>,
                &'a mut FileEffects,
            ) -> BoxFuture<'a, Result<T, ApiError>>
            + Send
            + 'static,
    {
        handle.block_on(self.tx.run(move |tx, effects| {
            Box::pin(async move {
                tx.batch_execute("SAVEPOINT slonbase_call")
                    .await
                    .map_err(db_error)?;
                let result = f(tx, effects).await;
                let end = if result.is_ok() {
                    "RELEASE SAVEPOINT slonbase_call"
                } else {
                    "ROLLBACK TO SAVEPOINT slonbase_call"
                };
                tx.batch_execute(end).await.map_err(db_error)?;
                result
            })
        }))
    }

    fn collections_saved(&self, registry: Registry, deleted: Vec<Arc<Collection>>) {
        let mut state = self.state.lock().expect("state lock");
        state.registry = Arc::new(registry);
        state.collections_changed = true;
        state.deleted.extend(deleted);
    }
}

/// The native calls that run in the migration's transaction (`None` for the others).
pub(super) fn call_in_tx(
    ctx: &TxContext,
    handle: &tokio::runtime::Handle,
    name: &str,
    a: &Args,
) -> Option<Result<Value, ApiError>> {
    Some(match name {
        "collection" => ctx.collection(&a.str(0)).map(|c| collection_json(&c)),
        "collections" => Ok(Value::Array(
            ctx.registry()
                .all()
                .iter()
                .map(|c| collection_json(c))
                .collect(),
        )),
        "findRecordById" => (|| {
            let c = ctx.collection(&a.str(0))?;
            let id = a.str(1);
            ctx.run(handle, move |tx, _| {
                Box::pin(async move {
                    Ok(service::find_by_id_in(tx, &c, &id)
                        .await?
                        .map(Value::Object)
                        .unwrap_or(Value::Null))
                })
            })
        })(),
        "findRecordsByFilter" => (|| {
            let c = ctx.collection(&a.str(0))?;
            let registry = ctx.registry();
            let (filter, sort, limit, offset, params) =
                (a.str(1), a.str(2), a.int(3), a.int(4), a.object(5));
            ctx.run(handle, move |tx, _| {
                Box::pin(async move {
                    let records = service::find_by_filter_in(
                        tx, registry, &c, &filter, &sort, limit, offset, &params,
                    )
                    .await?;
                    Ok(Value::Array(
                        records.into_iter().map(Value::Object).collect(),
                    ))
                })
            })
        })(),
        "countRecords" => (|| {
            let c = ctx.collection(&a.str(0))?;
            let registry = ctx.registry();
            let (filter, params) = (a.str(1), a.object(2));
            ctx.run(handle, move |tx, _| {
                Box::pin(async move {
                    Ok(json!(
                        service::count_in(tx, registry, &c, &filter, &params).await?
                    ))
                })
            })
        })(),
        "save" => (|| {
            let c = ctx.collection(&a.str(0))?;
            let registry = ctx.registry();
            let mut data = a.object(1);
            let uploads = take_files(&mut data)?;
            let core = ctx.core.clone();
            ctx.run(handle, move |tx, effects| {
                Box::pin(async move {
                    let saved =
                        service::save_in(&core, &registry, tx, &c, data, uploads, effects).await?;
                    Ok(Value::Object(saved))
                })
            })
        })(),
        "delete" => (|| {
            let c = ctx.collection(&a.str(0))?;
            let registry = ctx.registry();
            let id = a.str(1);
            let core = ctx.core.clone();
            ctx.run(handle, move |tx, effects| {
                Box::pin(async move {
                    service::delete_in_tx(&core, &registry, tx, &c, &id, effects).await?;
                    Ok(Value::Null)
                })
            })
        })(),
        "dbQuery" => (|| {
            let (sql, params, mode) = (a.str(0), a.object(1), a.str(2));
            let records = match a.str(3) {
                name if name.is_empty() => None,
                name => Some(ctx.collection(&name)?),
            };
            ctx.run(handle, move |tx, _| {
                Box::pin(async move {
                    super::native::db_query_in(tx, &sql, &params, &mode, records.as_deref()).await
                })
            })
        })(),
        "settings" => serde_json::to_value(ctx.settings()).map_err(internal),
        "saveSettings" => (|| {
            let settings = crate::settings::prepare_patch(&ctx.settings(), a.object(0))?;
            let saved = settings.clone();
            ctx.run(handle, move |tx, _| {
                Box::pin(async move {
                    saved.save(tx).await.map_err(ApiError::internal)?;
                    crate::realtime::notify(tx, "settings")
                        .await
                        .map_err(db_error)?;
                    Ok(())
                })
            })?;
            let value = serde_json::to_value(&settings).map_err(internal)?;
            ctx.state.lock().expect("state lock").settings = Some(settings);
            Ok(value)
        })(),
        "saveCollection" | "importCollections" => (|| {
            let (items, delete_missing) = if name == "saveCollection" {
                (vec![a.value(0)], false)
            } else {
                match a.value(0) {
                    Value::Array(items) => (items, a.value(1).as_bool().unwrap_or(false)),
                    _ => return Err(bad("importCollections expects a list of collections.")),
                }
            };
            let registry = ctx.registry();
            let single = name == "saveCollection";
            let imported = ctx.run(handle, move |tx, _| {
                Box::pin(async move {
                    crate::collections::tx::import(tx, &registry, items, delete_missing, single)
                        .await
                })
            })?;
            let saved = imported.saved.first().map(|c| collection_json(c));
            ctx.collections_saved(imported.registry, imported.deleted);
            Ok(saved.unwrap_or(Value::Null))
        })(),
        "deleteCollection" => (|| {
            let registry = ctx.registry();
            let id = a.str(0);
            let (registry, deleted) = ctx.run(handle, move |tx, _| {
                Box::pin(async move { crate::collections::tx::delete(tx, &registry, &id).await })
            })?;
            ctx.collections_saved(registry, vec![deleted]);
            Ok(Value::Null)
        })(),
        _ => return None,
    })
}

// ------------------------------------------------------------------ files and history

/// The migration files of `dir` (`*.js`, by name), with their sources. A missing
/// directory has none.
pub fn files(dir: &Path) -> Result<Vec<(String, String)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", dir.display())),
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".js") || name.ends_with(".d.ts") || !entry.path().is_file() {
            continue;
        }
        let source = std::fs::read_to_string(entry.path())
            .with_context(|| format!("failed to read {}", entry.path().display()))?;
        files.push((name, source));
    }
    files.sort();
    Ok(files)
}

/// The applied migrations, oldest first.
pub async fn applied(core: &Core) -> Result<Vec<String>> {
    let client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    Ok(client
        .query(
            "SELECT file FROM _slonbase.app_migrations ORDER BY applied, file",
            &[],
        )
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect())
}

/// Forgets applied migrations whose files are gone, and returns them.
pub async fn sync_history(core: &Core, dir: &Path) -> Result<Vec<String>> {
    let present: Vec<String> = files(dir)?.into_iter().map(|(name, _)| name).collect();
    let client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    Ok(client
        .query(
            "DELETE FROM _slonbase.app_migrations WHERE NOT (file = ANY($1)) RETURNING file",
            &[&present],
        )
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect())
}

// ------------------------------------------------------------------ running

/// Applies the pending migrations of `dir`, in order, and returns them. `hooks_dir` is
/// where `require()` finds modules.
pub async fn up(core: &Arc<Core>, dir: &Path, hooks_dir: &Path) -> Result<Vec<String>> {
    let pending: Vec<(String, String)> = {
        let files = files(dir)?;
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let done = applied(core).await?;
        files
            .into_iter()
            .filter(|(name, _)| !done.contains(name))
            .collect()
    };
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let runtime = MigrationRuntime::start(core, hooks_dir)?;
    let mut ran = Vec::new();
    for (name, source) in pending {
        if run_one(core, &runtime, &name, &source, Direction::Up).await? {
            tracing::info!(migration = %name, "applied migration");
            ran.push(name);
        }
    }
    Ok(ran)
}

/// Reverts the last `count` applied migrations (newest first) and returns them.
pub async fn down(
    core: &Arc<Core>,
    dir: &Path,
    hooks_dir: &Path,
    count: usize,
) -> Result<Vec<String>> {
    let files = files(dir)?;
    let mut done = applied(core).await?;
    done.reverse();
    done.truncate(count);
    if done.is_empty() {
        return Ok(Vec::new());
    }
    let runtime = MigrationRuntime::start(core, hooks_dir)?;
    let mut reverted = Vec::new();
    for name in done {
        let Some((_, source)) = files.iter().find(|(n, _)| *n == name) else {
            bail!(
                "the applied migration {name:?} isn't in {}; restore the file, or remove it \
                 from the history with `slonbase migrate history-sync`",
                dir.display()
            );
        };
        if run_one(core, &runtime, &name, source, Direction::Down).await? {
            tracing::info!(migration = %name, "reverted migration");
            reverted.push(name);
        }
    }
    Ok(reverted)
}

/// Runs one migration in its own transaction. Returns false when another instance already
/// applied (or reverted) it.
async fn run_one(
    core: &Arc<Core>,
    runtime: &MigrationRuntime,
    name: &str,
    source: &str,
    direction: Direction,
) -> Result<bool> {
    let (tx, effects, worker) = TxHandle::start(core);
    let file = name.to_string();
    let started = tx
        .run(move |tx, _| {
            Box::pin(async move {
                tx.execute("SELECT pg_advisory_xact_lock($1)", &[&MIGRATIONS_LOCK_KEY])
                    .await
                    .map_err(db_error)?;
                let applied = tx
                    .query_opt(
                        "SELECT 1 FROM _slonbase.app_migrations WHERE file = $1",
                        &[&file],
                    )
                    .await
                    .map_err(db_error)?
                    .is_some();
                let registry = store::load(tx).await.map_err(ApiError::internal)?;
                Ok((applied, registry))
            })
        })
        .await;
    let (applied, registry) = match started {
        Ok(started) => started,
        Err(err) => {
            drop(tx);
            let _ = worker.await;
            bail!("migration {name}: {}", err.message);
        }
    };
    if applied == (direction == Direction::Up) {
        drop(tx);
        let _ = worker.await;
        return Ok(false);
    }

    let ctx = TxContext {
        core: core.clone(),
        tx: tx.clone(),
        state: Arc::new(Mutex::new(TxState {
            registry: Arc::new(registry),
            collections_changed: false,
            settings: None,
            deleted: Vec::new(),
        })),
    };
    let outcome = match runtime.run(name, source, direction, ctx.clone()).await {
        Ok(()) => {
            let file = name.to_string();
            let recorded = tx
                .run(move |tx, _| {
                    Box::pin(async move {
                        let sql = match direction {
                            Direction::Up => {
                                "INSERT INTO _slonbase.app_migrations (file) VALUES ($1)"
                            }
                            Direction::Down => {
                                "DELETE FROM _slonbase.app_migrations WHERE file = $1"
                            }
                        };
                        tx.execute(sql, &[&file]).await.map_err(db_error)?;
                        Ok(())
                    })
                })
                .await;
            match recorded {
                Ok(()) => tx.commit().await,
                Err(err) => Err(err),
            }
        }
        Err(message) => Err(ApiError::bad_request(message)),
    };
    drop(tx);
    if outcome.is_err() {
        worker.abort();
    }
    let _ = worker.await;
    let effects = std::mem::take(&mut *effects.lock().await);
    if let Err(err) = effects.settle(core, outcome).await {
        bail!("migration {name} failed: {}", err.message);
    }

    let (registry, settings, deleted) = {
        let mut state = ctx.state.lock().expect("state lock");
        (
            state.collections_changed.then(|| state.registry.clone()),
            state.settings.take(),
            std::mem::take(&mut state.deleted),
        )
    };
    if let Some(registry) = registry {
        core.set_registry(registry.as_ref().clone());
    }
    if let Some(settings) = settings {
        core.set_settings(settings);
    }
    for c in deleted {
        if let Err(err) = core.storage().delete_prefix(&c.id).await {
            tracing::warn!(error = %format!("{err:#}"), "failed to delete the collection's files");
        }
    }
    Ok(true)
}

struct Job {
    name: String,
    source: String,
    direction: Direction,
    ctx: TxContext,
    reply: tokio::sync::oneshot::Sender<Result<(), String>>,
}

/// A JavaScript runtime on a thread of its own, running migration files one at a time.
struct MigrationRuntime {
    jobs: std::sync::mpsc::Sender<Job>,
}

impl MigrationRuntime {
    fn start(core: &Arc<Core>, hooks_dir: &Path) -> Result<Self> {
        let shared = Arc::new(Shared {
            handle: tokio::runtime::Handle::current(),
            core: RwLock::new(Arc::downgrade(core)),
            dir: std::fs::canonicalize(hooks_dir).unwrap_or_else(|_| hooks_dir.to_path_buf()),
        });
        let (jobs, receiver) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("slonbase-migrations".into())
            .spawn(move || {
                let started = new_runtime(&shared);
                for job in receiver {
                    let result = match &started {
                        Ok((rt, ctx)) => run_file(rt, ctx, &job),
                        Err(err) => Err(err.clone()),
                    };
                    let _ = job.reply.send(result);
                }
            })?;
        Ok(Self { jobs })
    }

    async fn run(
        &self,
        name: &str,
        source: &str,
        direction: Direction,
        ctx: TxContext,
    ) -> Result<(), String> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.jobs
            .send(Job {
                name: name.to_string(),
                source: source.to_string(),
                direction,
                ctx,
                reply,
            })
            .map_err(|_| "the migrations runtime stopped".to_string())?;
        result
            .await
            .map_err(|_| "the migrations runtime stopped".to_string())?
    }
}

/// A runtime with the prelude (and no hooks).
fn new_runtime(shared: &Arc<Shared>) -> Result<(Runtime, Context), String> {
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_max_stack_size(4 << 20);
    let ctx = Context::full(&rt).map_err(|e| e.to_string())?;
    ctx.with(|ctx| -> Result<(), String> {
        let globals = ctx.globals();
        let js = |e: rquickjs::Error| e.to_string();
        let call_shared = shared.clone();
        globals
            .set(
                "__native_call",
                Function::new(ctx.clone(), move |name: String, args: String| {
                    super::native::call(&call_shared, &name, &args)
                })
                .map_err(js)?,
            )
            .map_err(js)?;
        globals
            .set(
                "__native_next",
                Function::new(ctx.clone(), |_state: String| -> String {
                    super::envelope(Err(ApiError::bad_request(
                        "e.next() isn't available in migrations.",
                    )))
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
        Ok(())
    })?;
    Ok((rt, ctx))
}

/// Evaluates a migration file and runs its up or down function, with the native calls
/// routed to the migration's transaction.
fn run_file(rt: &Runtime, ctx: &Context, job: &Job) -> Result<(), String> {
    CURRENT.with(|c| *c.borrow_mut() = Some(job.ctx.clone()));
    let result = ctx.with(|ctx| -> Result<(), String> {
        let mut options = rquickjs::context::EvalOptions::default();
        options.global = true;
        options.strict = false;
        options.filename = Some(job.name.clone());
        ctx.eval_with_options::<(), _>(job.source.as_str(), options)
            .catch(&ctx)
            .map_err(|e| e.to_string())?;
        let run: Function = ctx
            .globals()
            .get("__runMigration")
            .map_err(|e| e.to_string())?;
        let direction = match job.direction {
            Direction::Up => "up",
            Direction::Down => "down",
        };
        let out: String = run
            .call((direction,))
            .catch(&ctx)
            .map_err(|e| e.to_string())?;
        let out: Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
        match out.get("err") {
            Some(Value::String(message)) => Err(message.clone()),
            Some(other) => Err(other.to_string()),
            None => Ok(()),
        }
    });
    while rt.is_job_pending() {
        if rt.execute_pending_job().is_err() {
            break;
        }
    }
    CURRENT.with(|c| c.borrow_mut().take());
    super::native::set_uploads(Vec::new());
    result
}

// ------------------------------------------------------------------ writing files

/// A file name for a new migration: `<unix seconds>_<name>.js`, later than every file
/// already in `dir`.
pub fn new_file_name(dir: &Path, name: &str) -> PathBuf {
    let slug: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let slug = slug.trim_matches('_');
    let mut stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // keep the order when several files are written within a second
    if let Ok(existing) = files(dir) {
        for (file, _) in existing {
            if let Some(n) = file.split('_').next().and_then(|n| n.parse::<u64>().ok()) {
                stamp = stamp.max(n + 1);
            }
        }
    }
    dir.join(format!("{stamp}_{slug}.js"))
}

/// Writes a migration file and returns its path.
pub fn write(dir: &Path, name: &str, up: &str, down: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = new_file_name(dir, name);
    let body = format!(
        "/// <reference path=\"../pb_data/types.d.ts\" />\nmigrate((app) => {{\n{up}\n}}, (app) => {{\n{down}\n}})\n"
    );
    std::fs::write(&path, body).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("serializes")
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A blank migration (`slonbase migrate create <name>`).
pub fn write_blank(dir: &Path, name: &str) -> Result<PathBuf> {
    write(
        dir,
        name,
        "  // add up queries...",
        "  // add down queries...",
    )
}

/// A migration that imports every collection as it is now (`slonbase migrate collections`).
pub fn write_snapshot(dir: &Path, registry: &Registry) -> Result<PathBuf> {
    let snapshot: Vec<Value> = registry
        .all()
        .iter()
        .filter(|c| !c.system)
        .map(|c| collection_json(c))
        .collect();
    let up = format!(
        "  const snapshot = {};\n\n  return app.importCollections(snapshot, false);",
        indent(&pretty(&Value::Array(snapshot))).trim_start()
    );
    write(dir, "collections_snapshot", &up, "  return null;")
}

/// What a collection change was, for [`write_change`].
pub enum Change<'a> {
    Created(&'a Collection),
    Updated {
        old: &'a Collection,
        new: &'a Collection,
    },
    Deleted(&'a Collection),
}

/// A migration that redoes (and undoes) a collection change made in the dashboard or with
/// the collections API (automigrate).
pub fn write_change(dir: &Path, change: Change<'_>) -> Result<PathBuf> {
    let definition = |c: &Collection| indent(&pretty(&collection_json(c)));
    let id = |c: &Collection| serde_json::to_string(&c.id).expect("serializes");
    match change {
        Change::Created(c) => write(
            dir,
            &format!("created_{}", c.name),
            &format!(
                "  const collection = new Collection({});\n\n  return app.save(collection);",
                definition(c).trim_start()
            ),
            &format!(
                "  const collection = app.findCollectionByNameOrId({});\n\n  return app.delete(collection);",
                id(c)
            ),
        ),
        Change::Updated { old, new } => write(
            dir,
            &format!("updated_{}", new.name),
            &format!(
                "  const collection = app.findCollectionByNameOrId({});\n\n  unmarshal({}, collection);\n\n  return app.save(collection);",
                id(new),
                definition(new).trim_start()
            ),
            &format!(
                "  const collection = app.findCollectionByNameOrId({});\n\n  unmarshal({}, collection);\n\n  return app.save(collection);",
                id(old),
                definition(old).trim_start()
            ),
        ),
        Change::Deleted(c) => write(
            dir,
            &format!("deleted_{}", c.name),
            &format!(
                "  const collection = app.findCollectionByNameOrId({});\n\n  return app.delete(collection);",
                id(c)
            ),
            &format!(
                "  const collection = new Collection({});\n\n  return app.save(collection);",
                definition(c).trim_start()
            ),
        ),
    }
}

/// Writes the automigrate files for collection changes, when automigrate is on, and marks
/// them applied (the changes already are). Failures are logged: the change happened.
pub async fn automigrate(core: &Core, changes: Vec<Change<'_>>) {
    let Some(dir) = core.automigrate_dir() else {
        return;
    };
    let mut written = Vec::new();
    for change in changes {
        match write_change(dir, change) {
            Ok(path) => {
                tracing::info!(file = %path.display(), "wrote migration");
                if let Some(name) = path.file_name() {
                    written.push(name.to_string_lossy().to_string());
                }
            }
            Err(err) => tracing::error!(error = %format!("{err:#}"), "failed to write a migration"),
        }
    }
    if written.is_empty() {
        return;
    }
    let recorded = async {
        let client = core
            .pool
            .get()
            .await
            .map_err(crate::db::describe_pool_error)?;
        client
            .execute(
                "INSERT INTO _slonbase.app_migrations (file) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING",
                &[&written],
            )
            .await?;
        anyhow::Ok(())
    };
    if let Err(err) = recorded.await {
        tracing::error!(error = %format!("{err:#}"), "failed to mark the written migrations applied");
    }
}
