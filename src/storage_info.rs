//! `GET /api/settings/storage` (superusers): where uploaded files are kept, how much room is
//! left there, and how much they take; `POST /api/settings/storage/clear-thumbs` deletes the
//! generated thumbnails (they're made again when asked for).
//!
//! The usage is measured by walking the storage directory in the background, at most once
//! every five minutes per directory, never on the request's time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::app::App;
use crate::auth::RequestAuth;
use crate::error::ApiError;
use crate::storage::Storage;

const USAGE_TTL: Duration = Duration::from_secs(300);

#[derive(Clone, Default)]
struct Usage {
    files: u64,
    bytes: u64,
    thumb_files: u64,
    thumb_bytes: u64,
    /// Files per collection directory (its id), largest first when reported.
    collections: HashMap<String, (u64, u64)>,
    measured: Option<chrono::DateTime<chrono::Utc>>,
}

enum Slot {
    Measuring,
    Done(Instant, Usage),
}

fn cache() -> &'static Mutex<HashMap<PathBuf, Slot>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Slot>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn walk(root: &Path) -> Usage {
    let mut usage = Usage::default();
    let mut stack = vec![(root.to_path_buf(), None::<String>, false)];
    while let Some((dir, collection, thumbs)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if meta.is_dir() {
                let collection = collection.clone().or_else(|| Some(name.clone()));
                stack.push((
                    entry.path(),
                    collection,
                    thumbs || name.starts_with("thumbs_"),
                ));
            } else if meta.is_file() {
                let size = meta.len();
                usage.files += 1;
                usage.bytes += size;
                if thumbs {
                    usage.thumb_files += 1;
                    usage.thumb_bytes += size;
                }
                if let Some(c) = &collection {
                    let e = usage.collections.entry(c.clone()).or_default();
                    e.0 += 1;
                    e.1 += size;
                }
            }
        }
    }
    usage.measured = Some(chrono::Utc::now());
    usage
}

/// The last measured usage of `root`, measuring it again in the background when it's old.
fn usage(root: &Path, refresh: bool) -> Option<Usage> {
    let mut map = cache().lock().expect("usage lock");
    let stale = match map.get(root) {
        Some(Slot::Measuring) => false,
        Some(Slot::Done(at, _)) => refresh || at.elapsed() > USAGE_TTL,
        None => true,
    };
    let current = match map.get(root) {
        Some(Slot::Done(_, u)) => Some(u.clone()),
        _ => None,
    };
    if stale {
        map.insert(root.to_path_buf(), Slot::Measuring);
        let root = root.to_path_buf();
        let previous = current.clone();
        tokio::task::spawn_blocking(move || {
            let measured = walk(&root);
            let mut map = cache().lock().expect("usage lock");
            map.insert(root, Slot::Done(Instant::now(), measured));
            drop(previous);
        });
    }
    current
}

/// Free and total bytes of the file system holding `path`.
#[cfg(unix)]
fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `stat` a writable statvfs.
    if unsafe { libc::statvfs(c.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let block = stat.f_frsize as u64;
    Some((stat.f_bavail as u64 * block, stat.f_blocks as u64 * block))
}

#[cfg(not(unix))]
fn disk_space(_: &Path) -> Option<(u64, u64)> {
    None
}

/// `GET /api/settings/storage` (`?refresh=1` measures the usage again)
pub async fn info(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let storage = core.storage();
    let local = core.local_storage_path().to_path_buf();
    let names: HashMap<String, String> = core
        .registry()
        .all()
        .iter()
        .map(|c| (c.id.clone(), c.name.clone()))
        .collect();
    let (kind, path) = match storage.as_ref() {
        Storage::Local(l) => ("local", Some(l.root().to_path_buf())),
        Storage::S3(_) => ("s3", None),
        Storage::Unavailable(_) => ("unavailable", None),
    };
    let absolute = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let measured = path
        .as_deref()
        .and_then(|p| usage(p, q.get("refresh").is_some_and(|v| v == "1" || v == "true")));
    // the nearest directory that exists (storage directories are made with the first upload)
    let space = path
        .as_deref()
        .or(Some(local.as_path()))
        .and_then(|p| p.ancestors().find(|a| a.exists()).map(Path::to_path_buf))
        .and_then(|p| disk_space(&p));
    let usage_json = measured.map(|u| {
        let mut by_collection: Vec<(String, u64, u64)> = u
            .collections
            .iter()
            .map(|(id, (files, bytes))| (id.clone(), *files, *bytes))
            .collect();
        by_collection.sort_by_key(|c| std::cmp::Reverse(c.2));
        json!({
            "files": u.files,
            "bytes": u.bytes,
            "thumbFiles": u.thumb_files,
            "thumbBytes": u.thumb_bytes,
            "measured": u.measured.map(|m| crate::datetime::format(&m)),
            "collections": by_collection.into_iter().take(20).map(|(id, files, bytes)| json!({
                "id": id,
                "name": names.get(&id),
                "files": files,
                "bytes": bytes,
            })).collect::<Vec<_>>(),
        })
    });
    Ok(Json(json!({
        "kind": kind,
        "description": storage.describe(),
        "path": path.as_deref().map(absolute).map(|p| p.display().to_string()),
        "localPath": absolute(&local).display().to_string(),
        "disk": space.map(|(free, total)| json!({ "free": free, "total": total })),
        "usage": usage_json,
        "measuring": usage_json_pending(path.as_deref()),
    })))
}

fn usage_json_pending(path: Option<&Path>) -> bool {
    path.is_some_and(|p| {
        matches!(
            cache().lock().expect("usage lock").get(p),
            Some(Slot::Measuring)
        )
    })
}

/// `POST /api/settings/storage/clear-thumbs`: deletes the generated thumbnails (local storage).
pub async fn clear_thumbs(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let storage = core.storage();
    let Storage::Local(local) = storage.as_ref() else {
        return Err(ApiError::bad_request(
            "Thumbnails can only be cleared from local storage here.",
        ));
    };
    let root = local.root().to_path_buf();
    let (dirs, bytes) = tokio::task::spawn_blocking(move || {
        let mut removed = (0u64, 0u64);
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if !meta.is_dir() {
                    continue;
                }
                if entry.file_name().to_string_lossy().starts_with("thumbs_") {
                    let size = walk(&path).bytes;
                    if std::fs::remove_dir_all(&path).is_ok() {
                        removed.0 += 1;
                        removed.1 += size;
                    }
                } else {
                    stack.push(path);
                }
            }
        }
        removed
    })
    .await
    .map_err(|e| ApiError::internal(e.into()))?;
    // measure again
    usage(local.root(), true);
    tracing::info!(
        directories = dirs,
        bytes,
        "cleared the generated thumbnails"
    );
    Ok(Json(json!({ "directories": dirs, "bytes": bytes })))
}
