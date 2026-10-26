//! Imports a PocketBase installation into slonbase: a `pb_data` directory (with `data.db`) or a
//! PocketBase backup zip. Collections keep their ids, rules, indexes and options; records keep
//! their ids, timestamps and password hashes (both use bcrypt, so users sign in as before);
//! superusers, OAuth2 links, files and settings come along.
//!
//! The schema goes through the same path as `PUT /api/collections/import`; records are written
//! directly (no hooks, no realtime events), one transaction per collection. View collections
//! whose queries don't run on PostgreSQL are skipped and reported.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use rusqlite::types::ValueRef;
use serde_json::{Map, Value, json};

use crate::collections::model::{Collection, FieldKind, SUPERUSERS_COLLECTION_ID, Storage};
use crate::collections::schema::table_ref;
use crate::collections::system::EXTERNAL_AUTHS_COLLECTION_ID;
use crate::core::Core;
use crate::filter::{Params, quote_ident};
use crate::records::values;

/// System collections whose records are imported (the rest are PocketBase's own state).
const SYSTEM_WITH_RECORDS: [&str; 2] = [SUPERUSERS_COLLECTION_ID, EXTERNAL_AUTHS_COLLECTION_ID];
/// Rows read and written at a time.
const CHUNK: usize = 500;
/// Settings sections carried over.
const SETTINGS_SECTIONS: [&str; 8] = [
    "meta",
    "smtp",
    "s3",
    "backups",
    "batch",
    "rateLimits",
    "trustedProxy",
    "logs",
];

/// What an import did.
#[derive(Debug, Default)]
pub struct Report {
    pub collections: Vec<String>,
    pub records: Vec<(String, usize)>,
    pub superusers: usize,
    pub superusers_skipped: usize,
    pub external_auths: usize,
    pub files: usize,
    pub file_bytes: u64,
    pub settings: bool,
    /// PocketBase's applied JavaScript migrations, now marked applied here too.
    pub migrations: usize,
    pub warnings: Vec<String>,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Imported {} collection(s):", self.collections.len())?;
        for (name, n) in &self.records {
            writeln!(f, "  {name}: {n} record(s)")?;
        }
        writeln!(
            f,
            "Superusers: {} imported, {} skipped (already present)",
            self.superusers, self.superusers_skipped
        )?;
        writeln!(f, "OAuth2 links: {}", self.external_auths)?;
        writeln!(f, "Files: {} ({} bytes)", self.files, self.file_bytes)?;
        writeln!(
            f,
            "Settings: {}",
            if self.settings {
                "imported"
            } else {
                "not imported"
            }
        )?;
        if self.migrations > 0 {
            writeln!(
                f,
                "Migrations: {} marked as applied (copy pb_migrations to the migrations directory)",
                self.migrations
            )?;
        }
        for w in &self.warnings {
            writeln!(f, "warning: {w}")?;
        }
        Ok(())
    }
}

/// The PocketBase data to import from: its directory and an open `data.db`.
struct Source {
    dir: PathBuf,
    db: Arc<Mutex<rusqlite::Connection>>,
    /// Keeps an extracted backup alive.
    _extracted: Option<tempfile::TempDir>,
}

fn open_source(path: &Path) -> Result<Source> {
    let (dir, extracted) = if path.is_file() {
        let tmp = tempfile::tempdir().context("failed to create a temporary directory")?;
        let file = std::fs::File::open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        let mut zip = zip::ZipArchive::new(file).with_context(|| {
            format!("{} isn't a zip file (a PocketBase backup)", path.display())
        })?;
        zip.extract(tmp.path())
            .context("failed to extract the backup")?;
        (tmp.path().to_path_buf(), Some(tmp))
    } else {
        (path.to_path_buf(), None)
    };
    let db_path = dir.join("data.db");
    if !db_path.is_file() {
        bail!(
            "{} has no data.db: pass a pb_data directory or a PocketBase backup zip",
            dir.display()
        );
    }
    let db = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", db_path.display()))?;
    Ok(Source {
        dir,
        db: Arc::new(Mutex::new(db)),
        _extracted: extracted,
    })
}

/// Runs a closure on the SQLite connection, off the async threads.
async fn sqlite<T: Send + 'static>(
    db: &Arc<Mutex<rusqlite::Connection>>,
    f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T> + Send + 'static,
) -> Result<T> {
    let db = db.clone();
    Ok(tokio::task::spawn_blocking(move || f(&db.lock().expect("sqlite lock"))).await??)
}

/// A collection row of PocketBase's `_collections` as its API JSON.
fn collection_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<(bool, Map<String, Value>)> {
    let text = |i: usize| -> rusqlite::Result<Option<String>> { row.get(i) };
    let json_of = |i: usize| -> rusqlite::Result<Value> {
        Ok(text(i)?
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null))
    };
    let system: bool = row.get(1)?;
    let mut out = Map::new();
    // options first, so the columns win
    if let Value::Object(options) = json_of(11)? {
        out.extend(options);
    }
    out.insert("id".into(), json!(text(0)?));
    out.insert("system".into(), json!(system));
    out.insert("type".into(), json!(text(2)?));
    out.insert("name".into(), json!(text(3)?));
    out.insert("fields".into(), json_of(4)?);
    out.insert("indexes".into(), json_of(5)?);
    for (i, rule) in [
        "listRule",
        "viewRule",
        "createRule",
        "updateRule",
        "deleteRule",
    ]
    .iter()
    .enumerate()
    {
        out.insert((*rule).into(), json!(text(6 + i)?));
    }
    Ok((system, out))
}

/// A SQLite value as the JSON value slonbase stores for the field.
fn field_value(field: &crate::collections::model::Field, raw: ValueRef<'_>) -> Value {
    let text = || match raw {
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        ValueRef::Integer(n) => n.to_string(),
        ValueRef::Real(n) => n.to_string(),
        ValueRef::Blob(b) => String::from_utf8_lossy(b).into_owned(),
        ValueRef::Null => String::new(),
    };
    let parsed = || serde_json::from_str::<Value>(&text()).unwrap_or(Value::Null);
    if matches!(field.kind, FieldKind::GeoPoint(_)) {
        return match parsed() {
            Value::Null => values::default_value(field),
            v => v,
        };
    }
    match field.storage() {
        Storage::Text | Storage::Date => Value::String(text()),
        Storage::Number => match raw {
            ValueRef::Integer(n) => json!(n),
            ValueRef::Real(n) => json!(n),
            _ => json!(text().parse::<f64>().unwrap_or(0.0)),
        },
        Storage::Bool => Value::Bool(match raw {
            ValueRef::Integer(n) => n != 0,
            _ => matches!(text().as_str(), "true" | "1"),
        }),
        Storage::Multi => match raw {
            ValueRef::Null => Value::Array(vec![]),
            _ => match parsed() {
                Value::Array(items) => Value::Array(items),
                // a single value stored as plain text
                Value::Null if !text().is_empty() => Value::Array(vec![Value::String(text())]),
                Value::String(s) => Value::Array(vec![Value::String(s)]),
                _ => Value::Array(vec![]),
            },
        },
        Storage::Json => match raw {
            ValueRef::Null => Value::Null,
            _ => parsed(),
        },
    }
}

/// Copies the records of `c` from SQLite table `table`; returns (written, skipped as already
/// present).
async fn copy_records(
    core: &Core,
    source: &Source,
    c: &Collection,
    table: &str,
    skip_existing: bool,
) -> Result<(usize, usize)> {
    let fields: Vec<_> = c.fields.clone();
    // the columns PocketBase has for these fields
    let table_name = table.to_string();
    let available: Vec<String> = sqlite(&source.db, move |db| {
        let mut stmt = db.prepare(&format!("PRAGMA table_info({})", quote_ident(&table_name)))?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
        rows.collect()
    })
    .await?;
    let present: Vec<_> = fields
        .iter()
        .filter(|f| available.contains(&f.name))
        .cloned()
        .collect();
    if present.is_empty() {
        return Ok((0, 0));
    }

    let mut client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    let tx = client.transaction().await?;
    // a bulk load: no realtime events for the imported rows
    tx.batch_execute("SET LOCAL slonbase.realtime = off")
        .await?;
    let columns: Vec<String> = present.iter().map(|f| quote_ident(&f.name)).collect();
    let select = format!(
        "SELECT rowid, {} FROM {} WHERE rowid > ?1 ORDER BY rowid LIMIT {CHUNK}",
        columns.join(", "),
        quote_ident(table)
    );
    let rows_per_insert = (30_000 / present.len()).clamp(1, CHUNK);
    let mut last = 0i64;
    let (mut written, mut skipped) = (0usize, 0usize);
    loop {
        let (select, present_fields) = (select.clone(), present.clone());
        let chunk: Vec<(i64, Vec<Value>)> = sqlite(&source.db, move |db| {
            let mut stmt = db.prepare(&select)?;
            let rows = stmt.query_map([last], |r| {
                let rowid: i64 = r.get(0)?;
                let mut vals = Vec::with_capacity(present_fields.len());
                for (i, f) in present_fields.iter().enumerate() {
                    vals.push(field_value(f, r.get_ref(i + 1)?));
                }
                Ok((rowid, vals))
            })?;
            rows.collect()
        })
        .await?;
        let Some((last_rowid, _)) = chunk.last() else {
            break;
        };
        last = *last_rowid;
        for rows in chunk.chunks(rows_per_insert) {
            let mut params = Params::default();
            let mut tuples = Vec::with_capacity(rows.len());
            for (_, vals) in rows {
                let placeholders: Vec<String> = present
                    .iter()
                    .zip(vals)
                    .map(|(f, v)| {
                        let p = match values::to_param(f, v) {
                            Some(p) => params.push(p),
                            None => params.push_null(),
                        };
                        format!("({p}){}", values::sql_cast(f))
                    })
                    .collect();
                tuples.push(format!("({})", placeholders.join(", ")));
            }
            let sql = format!(
                "INSERT INTO {} ({}) VALUES {}{}",
                table_ref(&c.name),
                columns.join(", "),
                tuples.join(", "),
                if skip_existing {
                    " ON CONFLICT DO NOTHING"
                } else {
                    ""
                }
            );
            let n = tx
                .execute(&sql, &params.as_refs())
                .await
                .with_context(|| format!("failed to write records of {}", c.name))?
                as usize;
            written += n;
            skipped += rows.len() - n;
        }
    }
    tx.commit().await?;
    Ok((written, skipped))
}

/// Copies the files of the imported collections (thumbnails are left out: they're made again
/// on request). Returns (files, bytes).
async fn copy_files(
    core: &Core,
    storage_dir: &Path,
    collection_ids: &[String],
) -> Result<(usize, u64)> {
    let storage = core.storage();
    let (mut files, mut bytes) = (0usize, 0u64);
    for collection_id in collection_ids {
        let dir = storage_dir.join(collection_id);
        let Ok(records) = std::fs::read_dir(&dir) else {
            continue;
        };
        for record in records.flatten() {
            if !record.path().is_dir() {
                continue;
            }
            let record_id = record.file_name().to_string_lossy().to_string();
            for file in std::fs::read_dir(record.path())?.flatten() {
                let path = file.path();
                let name = file.file_name().to_string_lossy().to_string();
                // `thumbs_*` directories and PocketBase's `*.attrs` metadata files
                if !path.is_file() || name.ends_with(".attrs") {
                    continue;
                }
                let key = format!("{collection_id}/{record_id}/{name}");
                // copied, never moved: the PocketBase data stays as it was
                let data = tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("failed to read {}", path.display()))?;
                bytes += data.len() as u64;
                storage
                    .put(&key, data.into())
                    .await
                    .with_context(|| format!("failed to copy {key}"))?;
                files += 1;
            }
        }
    }
    Ok((files, bytes))
}

/// An API error with its field errors, for the report.
fn error_detail(e: &crate::error::ApiError) -> String {
    let mut out = e.message.clone();
    fn leaves(v: &Value, out: &mut Vec<String>) {
        if let Value::Object(m) = v {
            if let Some(Value::String(msg)) = m.get("message") {
                out.push(msg.clone());
            } else {
                for v in m.values() {
                    leaves(v, out);
                }
            }
        }
    }
    let mut messages = Vec::new();
    leaves(&Value::Object(e.data.clone()), &mut messages);
    if !messages.is_empty() {
        out.push(' ');
        out.push_str(&messages.join("; "));
    }
    out
}

/// Imports the PocketBase data at `path` (a `pb_data` directory or a backup zip). The
/// collections must not exist in slonbase yet.
pub async fn run(core: &Arc<Core>, path: &Path, with_settings: bool) -> Result<Report> {
    let source = open_source(path)?;
    let mut report = Report::default();

    // collections
    let rows: Vec<(bool, Map<String, Value>)> = sqlite(&source.db, |db| {
        let mut stmt = db.prepare(
            "SELECT id, system, type, name, fields, indexes, listRule, viewRule, createRule, \
             updateRule, deleteRule, options FROM _collections ORDER BY created, rowid",
        )?;
        let rows = stmt.query_map([], collection_json)?;
        rows.collect()
    })
    .await
    .context("failed to read PocketBase's collections (is this a PocketBase 0.23+ data.db?)")?;
    let registry = core.registry();
    let user: Vec<Map<String, Value>> = rows
        .into_iter()
        .filter(|(system, _)| !system)
        .map(|(_, c)| c)
        .collect();
    let taken: Vec<String> = user
        .iter()
        .filter_map(|c| {
            let name = c.get("name")?.as_str()?;
            let id = c.get("id")?.as_str()?;
            (registry.get(name).is_some() || registry.by_id(id).is_some()).then(|| name.to_string())
        })
        .collect();
    if !taken.is_empty() {
        bail!(
            "slonbase already has collections named like these: {}; import into an installation \
             without them",
            taken.join(", ")
        );
    }
    let is_view = |c: &Map<String, Value>| c.get("type").and_then(Value::as_str) == Some("view");
    let (views, tables): (Vec<_>, Vec<_>) = user.into_iter().partition(is_view);
    if !tables.is_empty() {
        crate::collections::api::import_definitions(
            core,
            tables.iter().cloned().map(Value::Object).collect(),
            false,
        )
        .await
        .map_err(|e| anyhow::anyhow!("failed to create the collections: {}", error_detail(&e)))?;
    }
    // views one by one: their queries are SQLite SQL, which may not run on PostgreSQL
    let mut imported_views = Vec::new();
    for view in &views {
        let name = view
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match crate::collections::api::import_definitions(
            core,
            vec![Value::Object(view.clone())],
            false,
        )
        .await
        {
            Ok(_) => imported_views.push(name),
            Err(e) => report.warnings.push(format!(
                "view collection {name:?} wasn't imported: {}; its query may use SQLite-only SQL, \
                 recreate it with a PostgreSQL query",
                error_detail(&e)
            )),
        }
    }

    // records of the base and auth collections
    let registry = core.registry();
    let mut collection_ids = Vec::new();
    for c in &tables {
        let name = c.get("name").and_then(Value::as_str).unwrap_or_default();
        let collection = registry
            .get(name)
            .cloned()
            .with_context(|| format!("collection {name} is missing after the import"))?;
        let (n, _) = copy_records(core, &source, &collection, &collection.name, false).await?;
        report.records.push((collection.name.clone(), n));
        report.collections.push(collection.name.clone());
        collection_ids.push(collection.id.clone());
    }
    report.collections.extend(imported_views);

    // superusers (those already in slonbase stay) and OAuth2 links
    for id in SYSTEM_WITH_RECORDS {
        let Some(collection) = registry.by_id(id).cloned() else {
            continue;
        };
        let (n, skipped) = copy_records(core, &source, &collection, &collection.name, true).await?;
        if id == SUPERUSERS_COLLECTION_ID {
            report.superusers = n;
            report.superusers_skipped = skipped;
        } else {
            report.external_auths = n;
        }
    }

    // files
    let storage_dir = source.dir.join("storage");
    if storage_dir.is_dir() {
        let (files, bytes) = copy_files(core, &storage_dir, &collection_ids).await?;
        report.files = files;
        report.file_bytes = bytes;
    } else if collection_ids.iter().any(|id| {
        registry.by_id(id).is_some_and(|c| {
            c.fields
                .iter()
                .any(|f| matches!(f.kind, FieldKind::File(_)))
        })
    }) {
        report.warnings.push(
            "no storage directory next to data.db (PocketBase may have kept files in S3): copy \
             the files to slonbase's storage yourself, under the same {collectionId}/{recordId}/ \
             keys"
                .into(),
        );
    }

    // settings
    if with_settings {
        // stored as text or as a blob
        let raw: Option<String> = sqlite(&source.db, |db| {
            let mut stmt = db.prepare("SELECT value FROM _params WHERE id = 'settings'")?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => Ok(match row.get_ref(0)? {
                    ValueRef::Text(t) | ValueRef::Blob(t) => {
                        Some(String::from_utf8_lossy(t).into_owned())
                    }
                    _ => None,
                }),
                None => Ok(None),
            }
        })
        .await
        .context("failed to read PocketBase's settings")?;
        match raw.as_deref().map(serde_json::from_str::<Value>) {
            Some(Ok(Value::Object(all))) => {
                let mut patch = Map::new();
                for section in SETTINGS_SECTIONS {
                    if let Some(v @ Value::Object(_)) = all.get(section) {
                        patch.insert(section.to_string(), v.clone());
                    }
                }
                match crate::settings::apply_patch(core, patch).await {
                    Ok(_) => report.settings = true,
                    Err(e) => report.warnings.push(format!(
                        "the settings weren't imported ({}); set them in the dashboard",
                        e.message
                    )),
                }
            }
            Some(_) => report.warnings.push(
                "PocketBase's settings are encrypted (PB_ENCRYPTION) and weren't imported; set \
                 them in the dashboard"
                    .into(),
            ),
            None => {}
        }
    }

    // other instances reload the collections
    if let Ok(client) = core.pool.get().await {
        let _ = crate::realtime::notify(&client, "collections").await;
    }
    report.migrations = import_migration_history(core, &source).await?;
    Ok(report)
}

/// Marks PocketBase's applied JavaScript migrations (`*.js` in its `_migrations` table) as
/// applied, so its `pb_migrations` don't run again on the imported collections.
async fn import_migration_history(core: &Core, source: &Source) -> Result<usize> {
    let files: Vec<String> = sqlite(&source.db, |db| {
        let exists: bool = db.query_row(
            "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = '_migrations'",
            [],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(Vec::new());
        }
        let mut stmt = db.prepare(
            "SELECT file FROM _migrations WHERE file LIKE '%.js' ORDER BY applied, file",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect()
    })
    .await
    .context("failed to read PocketBase's migrations")?;
    if files.is_empty() {
        return Ok(0);
    }
    let client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    let n = client
        .execute(
            "INSERT INTO _slonbase.app_migrations (file) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING",
            &[&files],
        )
        .await?;
    Ok(n as usize)
}
