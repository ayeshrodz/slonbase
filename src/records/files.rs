//! File fields: uploads (multipart), storage, serving and thumbnails, compatible with
//! PocketBase.
//!
//! A file field stores file names (`photo_ab12cd34ef.jpg`); the files live at
//! `{collectionId}/{recordId}/{name}` in storage, thumbnails at
//! `{collectionId}/{recordId}/thumbs_{name}/{size}_{name}`.
//!
//! Request bodies (JSON or multipart) change file fields like PocketBase:
//! - `field` with uploads replaces the files; with names (or `""`/`[]`) keeps only those;
//! - `field+` appends uploads, `+field` prepends them;
//! - `field-` removes the named files.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, Multipart, Path, Query, Request, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use super::access::rule_sql;
use super::store::{self, ALIAS};
use super::values::{self, Values};
use crate::app::App;
use crate::auth::{RequestAuth, TokenType, authenticate_as};
use crate::collections::model::{Collection, Field, FieldKind, RuleAction};
use crate::collections::schema::table_ref;
use crate::core::Core;
use crate::db::describe_pool_error;
use crate::error::ApiError;
use crate::filter::{Params, RequestInfo};
use crate::validation::Errors;

/// Default max size per file (same as PocketBase).
pub const DEFAULT_MAX_SIZE: i64 = 5 << 20;
/// Max size of a multipart request.
pub const MAX_MULTIPART_SIZE: usize = 128 << 20;
/// Max size of a JSON request body.
pub const MAX_JSON_SIZE: usize = 2 << 20;
/// Thumbnail size always allowed (used by admin dashboards).
const DEFAULT_THUMB: &str = "100x100";

const THUMBABLE: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
/// Types served inline; everything else is a download (same list as PocketBase).
const INLINE: &[&str] = &[
    "image/png",
    "image/jpg",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/x-icon",
    "image/bmp",
    "video/webm",
    "video/mp4",
    "video/3gpp",
    "video/quicktime",
    "video/x-ms-wmv",
    "audio/basic",
    "audio/aiff",
    "audio/mpeg",
    "audio/midi",
    "audio/mp3",
    "audio/wave",
    "audio/wav",
    "audio/x-wav",
    "audio/x-mpeg",
    "audio/x-m4a",
    "audio/aac",
    "application/pdf",
    "application/x-pdf",
];

pub fn record_prefix(collection_id: &str, record_id: &str) -> String {
    format!("{collection_id}/{record_id}")
}

fn file_key(collection_id: &str, record_id: &str, name: &str) -> String {
    format!("{collection_id}/{record_id}/{name}")
}

fn thumb_key(collection_id: &str, record_id: &str, name: &str, size: &str) -> String {
    format!("{collection_id}/{record_id}/thumbs_{name}/{size}_{name}")
}

fn thumbs_prefix(collection_id: &str, record_id: &str, name: &str) -> String {
    format!("{collection_id}/{record_id}/thumbs_{name}")
}

// ---------------------------------------------------------------------------
// names and types

/// A safe stored name: `{snake_case_name}_{random}{.ext}` (like PocketBase).
pub fn normalize_name(original: &str, data: &[u8]) -> String {
    let original = original.rsplit(['/', '\\']).next().unwrap_or_default();
    let (stem, ext) = match original.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, ext),
        _ => (original, ""),
    };
    let mut ext: String = ext
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(20)
        .collect::<String>()
        .to_ascii_lowercase();
    if ext.is_empty() {
        ext = infer::get(data)
            .map(|t| t.extension().to_string())
            .unwrap_or_default();
    }

    let mut name = String::new();
    let mut last_underscore = true;
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() && !last_underscore && !name.is_empty() {
                // camelCase -> snake_case
                name.push('_');
            }
            name.push(c.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore {
            name.push('_');
            last_underscore = true;
        }
    }
    let mut name = name.trim_matches('_').to_string();
    name.truncate(100);
    if name.len() < 3 {
        name.push_str(&crate::ids::random_id(10));
    }
    let random = crate::ids::random_id(10);
    if ext.is_empty() {
        format!("{name}_{random}")
    } else {
        format!("{name}_{random}.{ext}")
    }
}

/// The MIME type detected from the content. The file name is never trusted for binary
/// types: it only distinguishes between text formats (so a text file named `x.png` is
/// `text/plain`, not `image/png`).
pub fn detect_mime(data: &[u8], name: &str) -> String {
    if let Some(t) = infer::get(data) {
        return t.mime_type().to_string();
    }
    let sample = &data[..data.len().min(4096)];
    let text = match std::str::from_utf8(sample) {
        Ok(text) => Some(text),
        // a multi-byte character cut by the sample
        Err(e) if e.error_len().is_none() => std::str::from_utf8(&sample[..e.valid_up_to()]).ok(),
        Err(_) => None,
    };
    let Some(text) = text else {
        return "application/octet-stream".into();
    };
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with("<svg") || (trimmed.starts_with("<?xml") && trimmed.contains("<svg")) {
        return "image/svg+xml".into();
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "md" => "text/markdown",
        "xml" => "application/xml",
        _ => "text/plain",
    }
    .into()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

// ---------------------------------------------------------------------------
// request bodies

/// An uploaded file from a multipart request.
pub struct Upload {
    /// The form key: `field`, `field+` or `+field`.
    pub key: String,
    pub original_name: String,
    pub data: Bytes,
}

/// A record request body: JSON, or multipart form data with uploads.
pub struct RecordBody {
    pub data: Map<String, Value>,
    pub uploads: Vec<Upload>,
    /// Form values are strings; JSON fields are parsed from them.
    pub from_form: bool,
}

impl<S: Send + Sync> FromRequest<S> for RecordBody {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        if !content_type.starts_with("multipart/form-data") {
            let bytes = axum::body::to_bytes(req.into_body(), MAX_JSON_SIZE)
                .await
                .map_err(|_| ApiError::bad_request("The request body is too large."))?;
            if bytes.iter().all(u8::is_ascii_whitespace) {
                return Ok(Self {
                    data: Map::new(),
                    uploads: vec![],
                    from_form: false,
                });
            }
            if !content_type.is_empty() && !content_type.starts_with("application/json") {
                return Err(ApiError::bad_request(
                    "Unsupported Content-Type, use application/json or multipart/form-data.",
                ));
            }
            return match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(data)) => Ok(Self {
                    data,
                    uploads: vec![],
                    from_form: false,
                }),
                Ok(_) => Err(ApiError::bad_request(
                    "The request body must be a JSON object.",
                )),
                Err(e) => Err(ApiError::bad_request(format!("Invalid JSON body: {e}"))),
            };
        }

        let mut multipart = Multipart::from_request(req, state)
            .await
            .map_err(|e| ApiError::bad_request(format!("Invalid multipart body: {e}")))?;
        let mut values: Vec<(String, String)> = Vec::new();
        let mut data = Map::new();
        let mut uploads = Vec::new();
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| ApiError::bad_request(format!("Invalid multipart body: {e}")))?
        {
            let key = field.name().unwrap_or_default().to_string();
            match field.file_name().map(str::to_string) {
                Some(original_name) => {
                    let data = field.bytes().await.map_err(|e| {
                        ApiError::bad_request(format!("Failed to read an upload: {e}"))
                    })?;
                    uploads.push(Upload {
                        key,
                        original_name,
                        data,
                    });
                }
                None => {
                    let text = field
                        .text()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("Invalid form value: {e}")))?;
                    if key == "@jsonPayload" {
                        match serde_json::from_str::<Value>(&text) {
                            Ok(Value::Object(map)) => data.extend(map),
                            _ => {
                                return Err(ApiError::bad_request(
                                    "@jsonPayload must be a JSON object.",
                                ));
                            }
                        }
                    } else {
                        values.push((key, text));
                    }
                }
            }
        }
        // repeated keys become lists
        let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
        for (key, value) in values {
            match grouped.iter_mut().find(|(k, _)| *k == key) {
                Some((_, list)) => list.push(value),
                None => grouped.push((key, vec![value])),
            }
        }
        for (key, mut list) in grouped {
            let value = if list.len() == 1 {
                Value::String(list.remove(0))
            } else {
                Value::Array(list.into_iter().map(Value::String).collect())
            };
            data.entry(key).or_insert(value);
        }
        Ok(Self {
            data,
            uploads,
            from_form: true,
        })
    }
}

impl RecordBody {
    /// Form values for JSON fields are parsed as JSON when possible.
    pub fn normalize_form_values(&mut self, c: &Collection) {
        if !self.from_form {
            return;
        }
        for field in &c.fields {
            if let FieldKind::Json(_) = field.kind {
                if let Some(Value::String(s)) = self.data.get(&field.name) {
                    if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                        self.data.insert(field.name.clone(), parsed);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// applying changes

pub struct NewFile {
    pub key: String,
    pub data: Bytes,
}

/// Files to write before saving the record, and to delete after it's saved.
#[derive(Default)]
pub struct FilePlan {
    pub new: Vec<NewFile>,
    /// Names of files removed from the record (with their thumbnails).
    pub removed: Vec<String>,
}

/// Applies the body and uploads to the file fields of `values`. `current` holds the
/// stored values (empty when creating). The record id must already be in `values`.
pub fn apply(
    c: &Collection,
    values: &mut Values,
    current: &Values,
    body: &Map<String, Value>,
    uploads: Vec<Upload>,
    errors: &mut Errors,
) -> FilePlan {
    let mut plan = FilePlan::default();
    let record_id = values
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut uploads_by_key: HashMap<String, Vec<Upload>> = HashMap::new();
    for upload in uploads {
        uploads_by_key
            .entry(upload.key.clone())
            .or_default()
            .push(upload);
    }

    for field in &c.fields {
        let FieldKind::File(options) = &field.kind else {
            continue;
        };
        let name = &field.name;
        let existing = values::as_strings(current.get(name).unwrap_or(&Value::Null));
        let mut list = existing.clone();

        let mut accept = |uploads: Vec<Upload>, errors: &mut Errors| -> Vec<String> {
            let max_size = if options.max_size == 0 {
                DEFAULT_MAX_SIZE
            } else {
                options.max_size
            };
            let mut names = Vec::new();
            for upload in uploads {
                if upload.data.len() as i64 > max_size {
                    errors.invalid(
                        name,
                        format!(
                            "The file {:?} is larger than {max_size} bytes.",
                            upload.original_name
                        ),
                    );
                    continue;
                }
                let mime = detect_mime(&upload.data, &upload.original_name);
                if !options.mime_types.is_empty()
                    && !options
                        .mime_types
                        .iter()
                        .any(|m| m.eq_ignore_ascii_case(&mime))
                {
                    errors.invalid(name, format!("The file type {mime} is not allowed."));
                    continue;
                }
                let stored = normalize_name(&upload.original_name, &upload.data);
                plan.new.push(NewFile {
                    key: file_key(&c.id, &record_id, &stored),
                    data: upload.data,
                });
                names.push(stored);
            }
            names
        };

        // `field`: kept names and/or replacement uploads
        let set_uploads = uploads_by_key.remove(name.as_str()).unwrap_or_default();
        if let Some(value) = body.get(name.as_str()) {
            let keep: HashSet<String> = values::as_strings(value).into_iter().collect();
            list.retain(|n| keep.contains(n));
        } else if !set_uploads.is_empty() {
            list.clear();
        }
        let added = accept(set_uploads, errors);
        list.extend(added);

        if let Some(value) = body.get(&format!("{name}-")) {
            let remove: HashSet<String> = values::as_strings(value).into_iter().collect();
            list.retain(|n| !remove.contains(n));
        }
        let appended = accept(
            uploads_by_key
                .remove(&format!("{name}+"))
                .unwrap_or_default(),
            errors,
        );
        list.extend(appended);
        let prepended = accept(
            uploads_by_key
                .remove(&format!("+{name}"))
                .unwrap_or_default(),
            errors,
        );
        list.splice(0..0, prepended);

        plan.removed
            .extend(existing.iter().filter(|n| !list.contains(n)).cloned());
        let value = if field.is_multi() {
            Value::Array(list.into_iter().map(Value::String).collect())
        } else {
            Value::String(list.pop().unwrap_or_default())
        };
        values.insert(name.clone(), value);
    }
    plan
}

/// Writes new files; on failure, already written ones are removed.
pub async fn write(core: &Core, plan: &FilePlan) -> Result<(), ApiError> {
    for (i, file) in plan.new.iter().enumerate() {
        if let Err(err) = core.storage().put(&file.key, file.data.clone()).await {
            discard(core, &plan.new[..i]).await;
            return Err(ApiError::internal(err));
        }
    }
    Ok(())
}

/// Removes written files after a failed save.
pub async fn discard(core: &Core, files: &[NewFile]) {
    for file in files {
        if let Err(err) = core.storage().delete(&file.key).await {
            tracing::warn!(key = %file.key, error = %format!("{err:#}"), "failed to remove a file");
        }
    }
}

/// Deletes the files removed from a record, after it's saved.
pub async fn remove_replaced(core: &Core, collection_id: &str, record_id: &str, names: &[String]) {
    for name in names {
        let results = [
            core.storage()
                .delete(&file_key(collection_id, record_id, name))
                .await,
            core.storage()
                .delete_prefix(&thumbs_prefix(collection_id, record_id, name))
                .await,
        ];
        for result in results {
            if let Err(err) = result {
                tracing::warn!(error = %format!("{err:#}"), "failed to delete a replaced file");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// thumbnails

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Crop {
    Center,
    Top,
    Bottom,
    Fit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbSize {
    width: u32,
    height: u32,
    crop: Crop,
}

impl ThumbSize {
    pub fn parse(raw: &str) -> Option<Self> {
        let (dims, crop) = match raw.chars().last()? {
            't' => (&raw[..raw.len() - 1], Crop::Top),
            'b' => (&raw[..raw.len() - 1], Crop::Bottom),
            'f' => (&raw[..raw.len() - 1], Crop::Fit),
            _ => (raw, Crop::Center),
        };
        let (w, h) = dims.split_once('x')?;
        let (width, height): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
        if (width == 0 && height == 0) || width > 4000 || height > 4000 {
            return None;
        }
        if (width == 0 || height == 0) && crop != Crop::Center {
            return None;
        }
        Some(Self {
            width,
            height,
            crop,
        })
    }
}

fn make_thumb(data: &[u8], size: ThumbSize) -> anyhow::Result<Vec<u8>> {
    use image::{ImageFormat, ImageReader, Limits, imageops::FilterType};
    use std::io::Cursor;

    let mut reader = ImageReader::new(Cursor::new(data)).with_guessed_format()?;
    // protect against decompression bombs
    let mut limits = Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(256 << 20);
    reader.limits(limits);
    let format = reader.format().unwrap_or(ImageFormat::Png);
    let img = reader.decode()?;
    let (w, h) = (size.width, size.height);

    let thumb = match (w, h, size.crop) {
        (0, h, _) => img.resize(u32::MAX, h, FilterType::Lanczos3),
        (w, 0, _) => img.resize(w, u32::MAX, FilterType::Lanczos3),
        (w, h, Crop::Fit) => img.resize(w, h, FilterType::Lanczos3),
        (w, h, Crop::Center) => img.resize_to_fill(w, h, FilterType::Lanczos3),
        (w, h, crop) => {
            // cover the box, then crop from the top or the bottom
            let scale = f64::max(
                w as f64 / img.width() as f64,
                h as f64 / img.height() as f64,
            );
            let scaled = img.resize_exact(
                ((img.width() as f64 * scale).ceil() as u32).max(w),
                ((img.height() as f64 * scale).ceil() as u32).max(h),
                FilterType::Lanczos3,
            );
            let x = (scaled.width() - w) / 2;
            let y = if crop == Crop::Top {
                0
            } else {
                scaled.height() - h
            };
            scaled.crop_imm(x, y, w, h)
        }
    };

    let out_format = match format {
        ImageFormat::Jpeg | ImageFormat::Gif => format,
        _ => ImageFormat::Png,
    };
    let mut out = Cursor::new(Vec::new());
    thumb.write_to(&mut out, out_format)?;
    Ok(out.into_inner())
}

// ---------------------------------------------------------------------------
// serving

/// `GET /api/files/{collection}/{recordId}/{filename}` with optional `thumb`, `token`
/// and `download` query parameters.
pub async fn serve(
    State(app): State<Arc<App>>,
    Path((collection, record_id, filename)): Path<(String, String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let c = core
        .registry()
        .get(&collection)
        .cloned()
        .ok_or_else(ApiError::not_found)?;
    if !valid_name(&filename) {
        return Err(ApiError::not_found());
    }
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(describe_pool_error(e)))?;
    let record = store::find_by_id(&client, &c, &record_id)
        .await
        .map_err(|e| ApiError::internal(e.into()))?
        .ok_or_else(ApiError::not_found)?;
    let field = file_field_with(&c, &record, &filename).ok_or_else(ApiError::not_found)?;
    let protected = matches!(&field.kind, FieldKind::File(o) if o.protected);

    if protected {
        let token = query.get("token").map(String::as_str).unwrap_or_default();
        let auth = if token.is_empty() {
            RequestAuth::default()
        } else {
            authenticate_as(&core, token, TokenType::File).await
        };
        let mut request = RequestInfo::new("GET");
        request.auth = auth.info();
        request.registry = Some(core.registry());
        let mut params = Params::default();
        let id_param = params.push(record_id.clone());
        let allowed = match rule_sql(&c, RuleAction::View, &auth, &request, &mut params) {
            Err(_) => false,
            Ok(None) => true,
            Ok(Some(rule)) => client
                .query_opt(
                    &format!(
                        "SELECT 1 FROM {} {ALIAS} WHERE {ALIAS}.\"id\" = {id_param} AND {rule}",
                        table_ref(&c.name)
                    ),
                    &params.as_refs(),
                )
                .await
                .map_err(|e| ApiError::internal(e.into()))?
                .is_some(),
        };
        if !allowed {
            return Err(ApiError::not_found());
        }
    }
    drop(client);

    let original_key = file_key(&c.id, &record_id, &filename);
    let mut served_key = original_key.clone();
    let mut served_name = filename.clone();

    if let Some(thumb) = query.get("thumb").filter(|t| !t.is_empty()) {
        let allowed = matches!(&field.kind, FieldKind::File(o) if thumb == DEFAULT_THUMB || o.thumbs.contains(thumb));
        if let (true, Some(size)) = (allowed, ThumbSize::parse(thumb)) {
            if let Some(key) =
                ensure_thumb(&core, &c.id, &record_id, &filename, thumb, size).await?
            {
                served_key = key;
                served_name = format!("{thumb}_{filename}");
            }
        }
    }

    let object = core
        .storage()
        .open(&served_key)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let size = object.size;
    // fused: the head may already have read to the end
    let mut stream = object.stream.fuse();

    // detect the type from the first bytes, then stream the rest
    let mut head = Vec::new();
    while head.len() < 8192 {
        match stream.next().await {
            Some(chunk) => {
                head.extend_from_slice(&chunk.map_err(|e| ApiError::internal(e.into()))?)
            }
            None => break,
        }
    }
    let content_type = detect_mime(&head, &served_name);

    let download = query
        .get("download")
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "t"));
    let disposition = if !download && INLINE.contains(&content_type.as_str()) {
        "inline"
    } else {
        "attachment"
    };
    let stream =
        futures_util::stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(head)) })
            .chain(stream);

    let mut response = Body::from_stream(stream).into_response();
    let headers = response.headers_mut();
    let set = |headers: &mut axum::http::HeaderMap, name, value: String| {
        if let Ok(v) = HeaderValue::from_str(&value) {
            headers.insert(name, v);
        }
    };
    set(headers, header::CONTENT_TYPE, content_type);
    set(headers, header::CONTENT_LENGTH, size.to_string());
    set(
        headers,
        header::CONTENT_DISPOSITION,
        format!("{disposition}; filename={served_name:?}"),
    );
    set(
        headers,
        header::CONTENT_SECURITY_POLICY,
        "default-src 'none'; media-src 'self'; style-src 'unsafe-inline'; sandbox".into(),
    );
    set(headers, header::X_CONTENT_TYPE_OPTIONS, "nosniff".into());
    set(
        headers,
        header::CACHE_CONTROL,
        if protected {
            "private, no-store".into()
        } else {
            "max-age=2592000, stale-while-revalidate=86400".into()
        },
    );
    Ok(response)
}

use futures_util::StreamExt;

fn file_field_with<'c>(c: &'c Collection, record: &Values, filename: &str) -> Option<&'c Field> {
    c.fields.iter().find(|f| {
        matches!(f.kind, FieldKind::File(_))
            && values::as_strings(record.get(&f.name).unwrap_or(&Value::Null))
                .iter()
                .any(|n| n == filename)
    })
}

/// Returns the thumbnail's key, generating it on first use. `None` when the file
/// isn't an image that can be thumbnailed.
async fn ensure_thumb(
    core: &Core,
    collection_id: &str,
    record_id: &str,
    filename: &str,
    raw_size: &str,
    size: ThumbSize,
) -> Result<Option<String>, ApiError> {
    let key = thumb_key(collection_id, record_id, filename, raw_size);
    if core
        .storage()
        .exists(&key)
        .await
        .map_err(ApiError::internal)?
    {
        return Ok(Some(key));
    }
    let Some(original) = core
        .storage()
        .read(&file_key(collection_id, record_id, filename))
        .await
        .map_err(ApiError::internal)?
    else {
        return Ok(None);
    };
    if !THUMBABLE.contains(&detect_mime(&original, filename).as_str()) {
        return Ok(None);
    }
    let thumb = tokio::task::spawn_blocking(move || make_thumb(&original, size))
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    match thumb {
        Ok(data) => {
            core.storage()
                .put(&key, Bytes::from(data))
                .await
                .map_err(ApiError::internal)?;
            Ok(Some(key))
        }
        Err(err) => {
            // a corrupt or unsupported image: serve the original instead
            tracing::debug!(error = %format!("{err:#}"), "failed to create a thumbnail");
            Ok(None)
        }
    }
}

/// `POST /api/files/token`: a short-lived token for protected files.
pub async fn token(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let token = if let Some(record) = auth.any_record() {
        let duration = record
            .collection
            .auth_options()
            .map(|a| a.file_token.duration)
            .unwrap_or(180);
        record.issue(&core, TokenType::File, Map::new(), duration)
    } else {
        return Err(ApiError::unauthorized("Missing auth context."));
    };
    Ok(Json(json!({ "token": token })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let n = normalize_name("My Holiday Photo.JPG", b"");
        assert!(n.starts_with("my_holiday_photo_"), "{n}");
        assert!(n.ends_with(".jpg"), "{n}");
        assert!(valid_name(&n));

        let n = normalize_name("../../etc/passwd", b"");
        assert!(n.starts_with("passwd_") && valid_name(&n), "{n}");
        let n = normalize_name(".htaccess", b"");
        assert!(valid_name(&n), "{n}");
        let n = normalize_name("a.b", b"");
        assert!(
            n.len() > 10 && n.ends_with(".b"),
            "short names get a random part: {n}"
        );
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert!(normalize_name("noext", png).ends_with(".png"));
        assert!(normalize_name("camelCaseName.txt", b"").starts_with("camel_case_name_"));
    }

    #[test]
    fn mime_detection() {
        assert_eq!(
            detect_mime(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", "x.txt"),
            "image/png",
            "content wins"
        );
        assert_eq!(detect_mime(b"hello", "notes.txt"), "text/plain");
        assert_eq!(
            detect_mime(b"<svg></svg>", "x.txt"),
            "image/svg+xml",
            "svg by content"
        );
        assert_eq!(
            detect_mime(b"not an image", "fake.png"),
            "text/plain",
            "names don't decide"
        );
        assert_eq!(detect_mime(b"<h1>x</h1>", "page.html"), "text/html");
        assert_eq!(
            detect_mime(&[0xff, 0xfe, 0x00, 0x01], "x.png"),
            "application/octet-stream"
        );
    }

    #[test]
    fn thumb_sizes() {
        assert_eq!(
            ThumbSize::parse("100x50"),
            Some(ThumbSize {
                width: 100,
                height: 50,
                crop: Crop::Center
            })
        );
        assert_eq!(ThumbSize::parse("100x50t").unwrap().crop, Crop::Top);
        assert_eq!(ThumbSize::parse("0x300").unwrap().width, 0);
        for bad in [
            "", "x", "100", "0x0", "0x100f", "axb", "100x100z", "99999x1",
        ] {
            assert!(ThumbSize::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn thumbnails() {
        let mut img = image::RgbImage::new(400, 200);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x % 255) as u8, 10, 20]);
        }
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();

        for (raw, expected) in [
            ("100x100", (100, 100)),
            ("100x100t", (100, 100)),
            ("100x100f", (100, 50)),
            ("0x50", (100, 50)),
            ("200x0", (200, 100)),
        ] {
            let out = make_thumb(&png, ThumbSize::parse(raw).unwrap()).unwrap();
            let decoded = image::load_from_memory(&out).unwrap();
            assert_eq!((decoded.width(), decoded.height()), expected, "{raw}");
        }
        assert!(make_thumb(b"not an image", ThumbSize::parse("10x10").unwrap()).is_err());
    }
}
