//! Backups: zip archives of the database and the local files, compatible with
//! PocketBase's backups API (`/api/backups`) and dashboard.
//!
//! slonbase can't rely on `pg_dump` (not in the `scratch` image, and it must match the
//! server's version), so it exports its own tables with `COPY ... TO STDOUT` (the text
//! format, portable across PostgreSQL versions) in one repeatable-read snapshot:
//! - the collections and the params (settings and the auth secret) system tables,
//! - every collection's table (records, including superusers and linked accounts),
//! - and the local storage files, when files aren't stored in S3.
//!
//! Short-lived state (realtime events, OTPs, MFA sessions, logs) isn't included. A restore
//! replaces the tables in one transaction (recreating them from the restored collections)
//! and the local files, then every instance reloads collections, settings and the auth
//! secret. Backups are stored in the backups directory, or in the S3 bucket of the
//! `backups.s3` settings.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path as UrlPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use deadpool_postgres::{GenericClient, Transaction};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::app::App;
use crate::auth::{RequestAuth, TokenType, authenticate_as};
use crate::collections::model::Collection;
use crate::collections::schema::{self, table_ref};
use crate::collections::store;
use crate::core::Core;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::filter::quote_ident;
use crate::migrations::{MIGRATIONS, SCHEMA_LOCK_KEY};
use crate::storage::Storage;

const MANIFEST: &str = "slonbase-backup.json";
const FORMAT: &str = "slonbase-backup";
const FORMAT_VERSION: u32 = 1;
/// Held (as a session lock) while a backup or restore runs on any instance.
const BACKUP_LOCK_KEY: i64 = SCHEMA_LOCK_KEY + 1;
/// System tables in backups; the others are rebuilt or short-lived.
const SYSTEM_TABLES: &[&str] = &["collections", "params"];
/// Names of automatic backups start with this.
pub const AUTO_PREFIX: &str = "auto_backup_";

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    /// The last applied system migration.
    migration: i32,
    created: String,
    slonbase: String,
    tables: Vec<TableEntry>,
    /// Whether `storage/` holds the local files.
    storage: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct TableEntry {
    schema: String,
    table: String,
    columns: Vec<String>,
    file: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("Try again later - another backup or restore is already in progress.")]
    Busy,
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<BackupError> for ApiError {
    fn from(err: BackupError) -> Self {
        match err {
            BackupError::Busy => ApiError::bad_request(err.to_string()),
            BackupError::Invalid(message) => ApiError::bad_request(message),
            BackupError::Other(err) => ApiError::internal(err),
        }
    }
}

/// `{name}.zip` with lowercase letters, digits, `_` and `-` (PocketBase's rule).
pub fn valid_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".zip") else {
        return false;
    };
    !stem.is_empty()
        && name.len() <= 150
        && stem
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Where backups are stored: the `backups.s3` bucket, or the backups directory.
pub fn storage(core: &Core) -> Storage {
    let settings = core.settings();
    if settings.backups.s3.enabled {
        Storage::s3(settings.backups.s3.config())
    } else {
        Storage::local(&core.backups_dir)
    }
}

/// A scratch directory next to the backups, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    async fn new(core: &Core) -> Result<Self> {
        let path = core
            .backups_dir
            .join(format!(".tmp-{}", crate::ids::random_id(12)));
        tokio::fs::create_dir_all(&path)
            .await
            .with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Holds the instance's and the database-wide backup locks.
struct Locks<'a> {
    _local: tokio::sync::MutexGuard<'a, ()>,
    client: deadpool_postgres::Client,
}

impl Locks<'_> {
    async fn release(self) {
        let _ = self
            .client
            .execute("SELECT pg_advisory_unlock($1)", &[&BACKUP_LOCK_KEY])
            .await;
    }
}

async fn lock(core: &Core) -> Result<Locks<'_>, BackupError> {
    let local = core.backup_lock.try_lock().map_err(|_| BackupError::Busy)?;
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| BackupError::Other(crate::db::describe_pool_error(e)))?;
    let locked: bool = client
        .query_one("SELECT pg_try_advisory_lock($1)", &[&BACKUP_LOCK_KEY])
        .await
        .map_err(|e| BackupError::Other(e.into()))?
        .get(0);
    if !locked {
        return Err(BackupError::Busy);
    }
    Ok(Locks {
        _local: local,
        client,
    })
}

// ---------------------------------------------------------------------------
// creating

async fn copy_out(tx: &Transaction<'_>, sql: &str, path: &Path) -> Result<()> {
    let stream = tx.copy_out(sql).await?;
    futures_util::pin_mut!(stream);
    let mut file = tokio::fs::File::create(path).await?;
    while let Some(chunk) = stream.next().await {
        file.write_all(&chunk?).await?;
    }
    file.flush().await?;
    Ok(())
}

fn record_columns(c: &Collection) -> Vec<String> {
    c.fields.iter().map(|f| f.name.clone()).collect()
}

/// Creates a backup named `name` (which must be valid).
pub async fn create(core: &Core, name: &str) -> Result<(), BackupError> {
    if !valid_name(name) {
        return Err(BackupError::Invalid("Invalid backup name.".into()));
    }
    let target = storage(core);
    if target.exists(name).await.map_err(BackupError::Other)? {
        return Err(BackupError::Invalid(format!(
            "A backup named {name:?} already exists."
        )));
    }
    let locks = lock(core).await?;
    let result = create_locked(core, name, &target).await;
    locks.release().await;
    result.map_err(BackupError::Other)
}

async fn create_locked(core: &Core, name: &str, target: &Storage) -> Result<()> {
    let started = std::time::Instant::now();
    let tmp = TempDir::new(core).await?;
    tokio::fs::create_dir_all(tmp.0.join("db")).await?;

    let mut client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    let tx = client
        .build_transaction()
        .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
        .read_only(true)
        .start()
        .await?;
    let migration: i32 = tx
        .query_one(
            "SELECT COALESCE(max(version), 0) FROM _slonbase.migrations",
            &[],
        )
        .await?
        .get(0);

    let mut tables = Vec::new();
    for table in SYSTEM_TABLES {
        let columns: Vec<String> = tx
            .query(
                "SELECT column_name::text FROM information_schema.columns
                 WHERE table_schema = '_slonbase' AND table_name = $1 ORDER BY ordinal_position",
                &[table],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        let file = format!("db/_slonbase.{table}.copy");
        let list = columns
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        copy_out(
            &tx,
            &format!("COPY (SELECT {list} FROM _slonbase.{table}) TO STDOUT"),
            &tmp.0.join(&file),
        )
        .await?;
        tables.push(TableEntry {
            schema: "_slonbase".into(),
            table: table.to_string(),
            columns,
            file,
        });
    }
    let registry = store::load(&tx).await?;
    for (i, c) in registry.all().iter().enumerate() {
        let columns = record_columns(c);
        let file = format!("db/collection-{i}.copy");
        let list = columns
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        copy_out(
            &tx,
            // in insertion order, so restored records keep it
            &format!(
                "COPY (SELECT {list} FROM {} ORDER BY _seq) TO STDOUT",
                table_ref(&c.name)
            ),
            &tmp.0.join(&file),
        )
        .await?;
        tables.push(TableEntry {
            schema: "slonbase".into(),
            table: c.name.clone(),
            columns,
            file,
        });
    }
    tx.commit().await?;
    drop(client);

    let local_files = match core.storage().as_ref() {
        Storage::Local(local) => Some(local.root().to_path_buf()),
        _ => None,
    };
    let manifest = Manifest {
        format: FORMAT.into(),
        version: FORMAT_VERSION,
        migration,
        created: crate::datetime::format(&chrono::Utc::now()),
        slonbase: env!("CARGO_PKG_VERSION").into(),
        tables,
        storage: local_files.is_some(),
    };
    let archive = tmp.0.join("archive.zip");
    let (dir, archive_path) = (tmp.0.clone(), archive.clone());
    tokio::task::spawn_blocking(move || {
        write_zip(&archive_path, &manifest, &dir, local_files.as_deref())
    })
    .await??;
    target.put_file(name, &archive).await?;
    tracing::info!(backup = %name, elapsed = ?started.elapsed(), "created a backup");
    Ok(())
}

fn write_zip(path: &Path, manifest: &Manifest, dir: &Path, files: Option<&Path>) -> Result<()> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    let mut zip = zip::ZipWriter::new(std::fs::File::create(path)?);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    zip.start_file(MANIFEST, options)?;
    zip.write_all(&serde_json::to_vec_pretty(manifest)?)?;
    for table in &manifest.tables {
        zip.start_file(&table.file, options)?;
        std::io::copy(&mut std::fs::File::open(dir.join(&table.file))?, &mut zip)?;
    }
    if let Some(root) = files {
        let mut stack = vec![root.to_path_buf()];
        while let Some(current) = stack.pop() {
            let entries = match std::fs::read_dir(&current) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with('.') {
                    continue; // temp files
                }
                if entry.file_type()?.is_dir() {
                    stack.push(path);
                    continue;
                }
                let relative = path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                // already compressed formats are stored as is
                let stored = matches!(
                    relative.rsplit('.').next(),
                    Some("jpg" | "jpeg" | "png" | "gif" | "webp" | "zip" | "gz" | "mp4" | "pdf")
                );
                let entry_options = if stored {
                    options.compression_method(zip::CompressionMethod::Stored)
                } else {
                    options
                };
                zip.start_file(format!("storage/{relative}"), entry_options)?;
                std::io::copy(&mut std::fs::File::open(&path)?, &mut zip)?;
            }
        }
    }
    zip.finish()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// restoring

/// Reads the manifest and checks the archive is a backup this version can restore.
fn read_manifest(path: &Path) -> Result<Manifest, BackupError> {
    let invalid = || BackupError::Invalid("The file isn't a slonbase backup.".into());
    let file = std::fs::File::open(path).map_err(|e| BackupError::Other(e.into()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| invalid())?;
    let mut entry = archive.by_name(MANIFEST).map_err(|_| invalid())?;
    let mut raw = String::new();
    std::io::Read::read_to_string(&mut entry, &mut raw).map_err(|_| invalid())?;
    let manifest: Manifest = serde_json::from_str(&raw).map_err(|_| invalid())?;
    if manifest.format != FORMAT {
        return Err(invalid());
    }
    let current = MIGRATIONS.iter().map(|m| m.version).max().unwrap_or(0);
    if manifest.version > FORMAT_VERSION || manifest.migration > current {
        return Err(BackupError::Invalid(format!(
            "The backup was created by a newer slonbase ({}); upgrade to restore it.",
            manifest.slonbase
        )));
    }
    Ok(manifest)
}

/// Extracts the database files and the local files (`storage/`) into `dir`.
fn extract(path: &Path, dir: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        // rejects absolute paths and `..`
        let Some(name) = entry.enclosed_name() else {
            bail!("invalid path in the backup: {:?}", entry.name());
        };
        if entry.is_dir() || !(name.starts_with("db") || name.starts_with("storage")) {
            continue;
        }
        let target = dir.join(&name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::io::copy(&mut entry, &mut std::fs::File::create(&target)?)?;
    }
    Ok(())
}

async fn copy_in(tx: &Transaction<'_>, sql: &str, path: &Path) -> Result<u64> {
    let sink = tx.copy_in(sql).await?;
    futures_util::pin_mut!(sink);
    let mut file = tokio::fs::File::open(path).await?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        sink.send(bytes::Bytes::copy_from_slice(&buf[..n])).await?;
    }
    Ok(sink.finish().await?)
}

/// Replaces the data with the backup `name`.
pub async fn restore(core: &Core, name: &str) -> Result<(), BackupError> {
    if !valid_name(name) {
        return Err(BackupError::Invalid("Invalid backup name.".into()));
    }
    let source = storage(core);
    let locks = lock(core).await?;
    let result = restore_locked(core, name, &source).await;
    locks.release().await;
    result
}

async fn restore_locked(core: &Core, name: &str, source: &Storage) -> Result<(), BackupError> {
    let started = std::time::Instant::now();
    let tmp = TempDir::new(core).await?;
    let archive = tmp.0.join("archive.zip");
    {
        let object = source
            .open(name)
            .await?
            .ok_or_else(|| BackupError::Invalid("The backup doesn't exist.".into()))?;
        let mut stream = object.stream;
        let mut file = tokio::fs::File::create(&archive)
            .await
            .map_err(anyhow::Error::from)?;
        while let Some(chunk) = stream.next().await {
            file.write_all(&chunk.map_err(anyhow::Error::from)?)
                .await
                .map_err(anyhow::Error::from)?;
        }
        file.flush().await.map_err(anyhow::Error::from)?;
    }
    let (path, dir) = (archive.clone(), tmp.0.clone());
    let manifest = tokio::task::spawn_blocking(move || -> Result<Manifest, BackupError> {
        let manifest = read_manifest(&path)?;
        extract(&path, &dir)?;
        Ok(manifest)
    })
    .await
    .map_err(|e| BackupError::Other(e.into()))??;

    restore_database(core, &manifest, &tmp.0)
        .await
        .map_err(BackupError::Other)?;
    if manifest.storage {
        replace_local_files(core, &tmp.0.join("storage"))
            .await
            .map_err(BackupError::Other)?;
    }
    // collections created after the backup's version (e.g. new system collections)
    crate::collections::system::ensure(&core.pool).await?;
    core.reload_after_restore().await?;
    tracing::info!(backup = %name, elapsed = ?started.elapsed(), "restored a backup");
    Ok(())
}

async fn restore_database(core: &Core, manifest: &Manifest, dir: &Path) -> Result<()> {
    let _guard = core.schema_lock.lock().await;
    let mut client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    let tx = client.transaction().await?;
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&SCHEMA_LOCK_KEY])
        .await?;

    let current = store::load(&tx).await?;
    for c in current.all() {
        for statement in schema::drop(c) {
            tx.batch_execute(&statement).await?;
        }
    }
    tx.batch_execute(
        "DELETE FROM _slonbase.collections; DELETE FROM _slonbase.params;
         TRUNCATE _slonbase.events, _slonbase.otps, _slonbase.mfas;",
    )
    .await?;

    let column_list = |columns: &[String]| {
        columns
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for table in manifest.tables.iter().filter(|t| t.schema == "_slonbase") {
        if !SYSTEM_TABLES.contains(&table.table.as_str()) {
            continue;
        }
        copy_in(
            &tx,
            &format!(
                "COPY _slonbase.{} ({}) FROM STDIN",
                table.table,
                column_list(&table.columns)
            ),
            &dir.join(&table.file),
        )
        .await
        .with_context(|| format!("failed to restore _slonbase.{}", table.table))?;
    }

    let restored = store::load(&tx).await?;
    for c in restored.all() {
        for statement in schema::create(c) {
            tx.batch_execute(&statement).await?;
        }
    }
    for table in manifest.tables.iter().filter(|t| t.schema == "slonbase") {
        if restored.get(&table.table).is_none() {
            continue;
        }
        copy_in(
            &tx,
            &format!(
                "COPY {} ({}) FROM STDIN",
                table_ref(&table.table),
                column_list(&table.columns)
            ),
            &dir.join(&table.file),
        )
        .await
        .with_context(|| format!("failed to restore the {} collection", table.table))?;
    }
    // other instances reload when this commits
    crate::realtime::notify(&tx, "restored").await?;
    tx.commit().await?;
    Ok(())
}

/// Swaps the local storage directory for the restored files.
async fn replace_local_files(core: &Core, restored: &Path) -> Result<()> {
    let target = core.local_storage_path().to_path_buf();
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if !tokio::fs::try_exists(restored).await? {
        tokio::fs::create_dir_all(restored).await?;
    }
    let old = target.with_file_name(format!(
        ".{}-before-restore-{}",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("storage"),
        crate::ids::random_id(8)
    ));
    let had_files = tokio::fs::try_exists(&target).await?;
    if had_files {
        tokio::fs::rename(&target, &old)
            .await
            .with_context(|| format!("failed to move {}", target.display()))?;
    }
    if let Err(err) = tokio::fs::rename(restored, &target).await {
        // e.g. another file system: put the old files back
        if had_files {
            let _ = tokio::fs::rename(&old, &target).await;
        }
        return Err(err).context("failed to move the restored files into place");
    }
    if had_files {
        let _ = tokio::fs::remove_dir_all(&old).await;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// automatic backups

/// Runs the automatic backup if the `backups.cron` schedule fires this minute. Only one
/// instance wins each minute (claimed in the params table).
pub async fn run_scheduled(core: &Arc<Core>) {
    let settings = core.settings();
    let Some(schedule) = crate::cron::Schedule::parse(&settings.backups.cron) else {
        return;
    };
    let now = chrono::Utc::now();
    if !schedule.matches(&now) {
        return;
    }
    let minute = now.format("%Y-%m-%dT%H:%M").to_string();
    let claimed = async {
        let client = core
            .pool
            .get()
            .await
            .map_err(crate::db::describe_pool_error)?;
        let rows = client
            .execute(
                "INSERT INTO _slonbase.params (key, value) VALUES ('backups_last_auto', to_jsonb($1::text))
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated = now()
                 WHERE _slonbase.params.value <> EXCLUDED.value",
                &[&minute],
            )
            .await?;
        anyhow::Ok(rows == 1)
    };
    match claimed.await {
        Ok(true) => {}
        Ok(false) => return,
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "failed to schedule the automatic backup");
            return;
        }
    }
    let core = core.clone();
    tokio::spawn(async move {
        if let Err(err) = run_auto_backup(&core).await {
            tracing::error!(error = %err, "the automatic backup failed");
        }
    });
}

/// Creates an automatic backup and deletes the oldest beyond `backups.cronMaxKeep`.
pub async fn run_auto_backup(core: &Core) -> Result<(), BackupError> {
    let name = format!(
        "{AUTO_PREFIX}{}.zip",
        chrono::Utc::now().format("%Y%m%d%H%M%S")
    );
    create(core, &name).await?;
    let keep = core.settings().backups.cron_max_keep.max(1) as usize;
    let target = storage(core);
    let mut auto: Vec<(String, u64, String)> = target
        .list_root()
        .await?
        .into_iter()
        .filter(|(key, _, _)| key.starts_with(AUTO_PREFIX))
        .collect();
    // names carry the creation time
    auto.sort_by(|a, b| b.0.cmp(&a.0));
    for (key, _, _) in auto.into_iter().skip(keep) {
        if let Err(err) = target.delete(&key).await {
            tracing::warn!(backup = %key, error = %format!("{err:#}"), "failed to delete an old automatic backup");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// the API (superusers)

/// `GET /api/backups`
pub async fn list(State(app): State<Arc<App>>, auth: RequestAuth) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let mut items: Vec<(String, u64, String)> = storage(&core)
        .list_root()
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|(key, _, _)| valid_name(key))
        .collect();
    items.sort_by(|a, b| b.2.cmp(&a.2));
    Ok(Json(Value::Array(
        items
            .into_iter()
            .map(|(key, size, modified)| json!({ "key": key, "size": size, "modified": modified }))
            .collect(),
    )))
}

/// `POST /api/backups` (`{"name"}`, optional)
pub async fn create_handler(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let name = match body.get("name").and_then(Value::as_str).unwrap_or_default() {
        "" => format!(
            "slonbase_backup_{}.zip",
            chrono::Utc::now().format("%Y%m%d%H%M%S")
        ),
        name => {
            if !valid_name(name) {
                let mut errors = crate::validation::Errors::new();
                errors.invalid(
                    "name",
                    "Must be a .zip name with only lowercase letters, digits, _ and -.",
                );
                return Err(ApiError::invalid_data(
                    "An error occurred while validating the submitted data.",
                    &errors,
                ));
            }
            name.to_string()
        }
    };
    create(&core, &name).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/backups/upload` (multipart, `file`): adds a backup created elsewhere.
pub async fn upload(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let tmp = TempDir::new(&core).await.map_err(ApiError::internal)?;
    let path = tmp.0.join("upload.zip");
    let mut name = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("Invalid upload: {e}.")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        name = field.file_name().map(str::to_string);
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|e| ApiError::internal(e.into()))?;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| ApiError::bad_request(format!("Invalid upload: {e}.")))?
        {
            file.write_all(&chunk)
                .await
                .map_err(|e| ApiError::internal(e.into()))?;
        }
        file.flush()
            .await
            .map_err(|e| ApiError::internal(e.into()))?;
    }
    let name = name.filter(|n| valid_name(n)).ok_or_else(|| {
        let mut errors = crate::validation::Errors::new();
        errors.invalid(
            "file",
            "Must be a .zip file with only lowercase letters, digits, _ and - in its name.",
        );
        ApiError::invalid_data("Failed to upload the backup.", &errors)
    })?;
    let check = path.clone();
    tokio::task::spawn_blocking(move || read_manifest(&check))
        .await
        .map_err(|e| ApiError::internal(e.into()))??;
    let target = storage(&core);
    if target.exists(&name).await.map_err(ApiError::internal)? {
        return Err(ApiError::bad_request(format!(
            "A backup named {name:?} already exists."
        )));
    }
    target
        .put_file(&name, &path)
        .await
        .map_err(ApiError::internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/backups/{key}?token=` (a superuser's file token, see `POST /api/files/token`)
pub async fn download(
    State(app): State<Arc<App>>,
    UrlPath(key): UrlPath<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    client_ip: Option<axum::Extension<crate::guard::ClientIp>>,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let denied = || ApiError::forbidden("Insufficient permissions to access the resource.");
    let token = query.get("token").map(String::as_str).unwrap_or_default();
    let auth = authenticate_as(&core, token, TokenType::File).await;
    let ip = client_ip.and_then(|e| e.0.0);
    if !auth.is_superuser()
        || !crate::guard::superuser_ip_allowed(&core.settings().superuser_ips, ip)
    {
        return Err(denied());
    }
    if !valid_name(&key) {
        return Err(ApiError::not_found());
    }
    let object = storage(&core)
        .open(&key)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CONTENT_LENGTH, object.size.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename={key:?}"),
            ),
            (header::CACHE_CONTROL, "private, no-store".to_string()),
        ],
        Body::from_stream(object.stream),
    )
        .into_response())
}

/// `DELETE /api/backups/{key}`
pub async fn delete(
    State(app): State<Arc<App>>,
    UrlPath(key): UrlPath<String>,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    if !valid_name(&key) {
        return Err(ApiError::not_found());
    }
    let target = storage(&core);
    if !target.exists(&key).await.map_err(ApiError::internal)? {
        return Err(ApiError::not_found());
    }
    target.delete(&key).await.map_err(ApiError::internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/backups/{key}/restore`
pub async fn restore_handler(
    State(app): State<Arc<App>>,
    UrlPath(key): UrlPath<String>,
    auth: RequestAuth,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    restore(&core, &key).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["a.zip", "slonbase_backup_20261013.zip", "my-backup_2.zip"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            ".zip",
            "a.tar",
            "A.zip",
            "a b.zip",
            "../a.zip",
            "a/b.zip",
            "@auto.zip",
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
