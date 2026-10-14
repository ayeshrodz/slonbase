//! The native functions behind the prelude's `$app`, `$http`, `$security`, `$os` and
//! `require` (called as `__native_call(name, argsJson)`).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::{Shared, envelope, inside};
use crate::auth::AuthRecord;
use crate::collections::api::to_json as collection_json;
use crate::collections::model::FIELD_PASSWORD;
use crate::core::Core;
use crate::error::ApiError;
use crate::records::files::Upload;

thread_local! {
    /// The files of the job running on this thread, by token: the request's uploads and
    /// those made with `$filesystem.fileFromBytes()`.
    static UPLOADS: std::cell::RefCell<std::collections::HashMap<String, Upload>> =
        Default::default();
}

/// Sets the files of the job about to run on this thread (and drops the previous ones).
pub(super) fn set_uploads(uploads: Vec<(String, Upload)>) {
    UPLOADS.with(|u| *u.borrow_mut() = uploads.into_iter().collect());
}

/// Largest file made with `$filesystem.fileFromBytes()`.
const MAX_FILE_BYTES: usize = 32 << 20;

/// `$filesystem.fileFromBytes(data, name)`: `data` is a string or an array of bytes.
fn file_from_bytes(data: &Value, name: &str) -> Result<Value, ApiError> {
    let bytes: Vec<u8> = match data {
        Value::String(s) => s.as_bytes().to_vec(),
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_u64().filter(|b| *b <= 255).map(|b| b as u8))
            .collect::<Option<_>>()
            .ok_or_else(|| bad("fileFromBytes: the data must be a string or an array of bytes."))?,
        _ => {
            return Err(bad(
                "fileFromBytes: the data must be a string or an array of bytes.",
            ));
        }
    };
    keep_file("fileFromBytes", bytes, name)
}

/// `$filesystem.fileFromPath(path)`: a file on the server's disk.
fn file_from_path(path: &str) -> Result<Value, ApiError> {
    let path = std::path::Path::new(path);
    let meta = std::fs::metadata(path)
        .map_err(|e| bad(format!("fileFromPath: can't read {}: {e}", path.display())))?;
    if !meta.is_file() {
        return Err(bad(format!(
            "fileFromPath: {} isn't a file.",
            path.display()
        )));
    }
    if meta.len() > MAX_FILE_BYTES as u64 {
        return Err(bad("fileFromPath: the file is too large."));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| bad(format!("fileFromPath: can't read {}: {e}", path.display())))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    keep_file("fileFromPath", bytes, &name)
}

/// Longest download of `$filesystem.fileFromURL` (seconds).
const FILE_URL_TIMEOUT: u64 = 60;

/// `$filesystem.fileFromURL(url)`: downloads a file (http or https, up to the size limit).
async fn file_from_url(url: &str) -> Result<Value, ApiError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| bad("fileFromURL: invalid URL."))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(bad("fileFromURL: only http and https URLs are supported."));
    }
    let mut response = crate::http::client()
        .get(parsed.clone())
        .timeout(Duration::from_secs(FILE_URL_TIMEOUT))
        .send()
        .await
        .map_err(|e| bad(format!("fileFromURL: the request failed: {e}")))?;
    if !response.status().is_success() {
        return Err(bad(format!(
            "fileFromURL: the server answered {}.",
            response.status().as_u16()
        )));
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_FILE_BYTES as u64)
    {
        return Err(bad("fileFromURL: the file is too large."));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| bad(format!("fileFromURL: failed to read the response: {e}")))?
    {
        if bytes.len() + chunk.len() > MAX_FILE_BYTES {
            return Err(bad("fileFromURL: the file is too large."));
        }
        bytes.extend_from_slice(&chunk);
    }
    let name = parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back().map(str::to_string))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "file".into());
    keep_file("fileFromURL", bytes, &name)
}

/// Keeps a file for the running job and returns its JavaScript value.
fn keep_file(function: &str, bytes: Vec<u8>, name: &str) -> Result<Value, ApiError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(bad(format!("{function}: the file is too large.")));
    }
    if name.trim().is_empty() {
        return Err(bad(format!("{function}: the file needs a name.")));
    }
    let token = crate::ids::random_id(24);
    let size = bytes.len();
    UPLOADS.with(|u| {
        u.borrow_mut().insert(
            token.clone(),
            Upload {
                key: String::new(),
                original_name: name.to_string(),
                data: bytes.into(),
            },
        )
    });
    Ok(json!({ "_upload": token, "name": name, "originalName": name, "size": size }))
}

/// The upload behind a file value of a record (`{"_upload": token, ...}`).
fn upload_of(value: &Value) -> Option<Result<Upload, ApiError>> {
    let token = value.get("_upload")?.as_str()?;
    Some(
        UPLOADS
            .with(|u| u.borrow().get(token).cloned())
            .ok_or_else(|| {
                bad("The file is no longer available (files live for one handler run).")
            }),
    )
}

/// Moves the files in record data to uploads: a field set to files (or to a list mixing
/// kept file names and files) keeps those names and appends the files.
fn take_files(data: &mut Map<String, Value>) -> Result<Vec<Upload>, ApiError> {
    let mut uploads = Vec::new();
    for (key, value) in data.iter_mut() {
        let items: Vec<Value> = match value {
            Value::Array(items) => items.clone(),
            Value::Object(_) => vec![value.clone()],
            _ => continue,
        };
        if !items.iter().any(|v| v.get("_upload").is_some()) {
            continue;
        }
        let mut kept = Vec::new();
        for item in items {
            match upload_of(&item) {
                Some(upload) => {
                    let mut upload = upload?;
                    upload.key = format!("{key}+");
                    uploads.push(upload);
                }
                None => kept.push(item),
            }
        }
        *value = Value::Array(kept);
    }
    Ok(uploads)
}

/// Longest `sleep()`.
const MAX_SLEEP_MS: u64 = 60_000;
/// Longest `$http.send` timeout (seconds).
const MAX_HTTP_TIMEOUT: u64 = 300;

fn bad(message: impl Into<String>) -> ApiError {
    ApiError::bad_request(message)
}

fn internal(err: impl Into<anyhow::Error>) -> ApiError {
    ApiError::internal(err.into())
}

pub(super) fn call(shared: &Arc<Shared>, name: &str, args: &str) -> String {
    let args: Vec<Value> = serde_json::from_str(args).unwrap_or_default();
    envelope(run(shared, name, &args))
}

struct Args<'a>(&'a [Value]);

impl Args<'_> {
    fn value(&self, i: usize) -> Value {
        self.0.get(i).cloned().unwrap_or(Value::Null)
    }
    fn str(&self, i: usize) -> String {
        match self.0.get(i) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        }
    }
    fn int(&self, i: usize) -> i64 {
        self.0.get(i).and_then(Value::as_f64).unwrap_or(0.0) as i64
    }
    fn object(&self, i: usize) -> Map<String, Value> {
        match self.0.get(i) {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        }
    }
}

fn run(shared: &Arc<Shared>, name: &str, args: &[Value]) -> Result<Value, ApiError> {
    let a = Args(args);
    let handle = &shared.handle;
    // calls that don't need the app
    match name {
        "isReady" => return Ok(Value::Bool(shared.core().is_ok())),
        "randomString" => return Ok(Value::String(random_string(a.int(0), &a.str(1))?)),
        "hash" => return Ok(Value::String(hash(&a.str(0), &a.str(1))?)),
        "hmac" => {
            use hmac::{KeyInit, Mac};
            let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(a.str(1).as_bytes())
                .expect("HMAC accepts any key length");
            mac.update(a.str(0).as_bytes());
            return Ok(Value::String(hex::encode(mac.finalize().into_bytes())));
        }
        "equal" => {
            return Ok(Value::Bool(crate::ids::constant_time_eq(
                &a.str(0),
                &a.str(1),
            )));
        }
        "env" => return Ok(Value::String(std::env::var(a.str(0)).unwrap_or_default())),
        "sleep" => {
            std::thread::sleep(Duration::from_millis(
                (a.int(0).max(0) as u64).min(MAX_SLEEP_MS),
            ));
            return Ok(Value::Null);
        }
        "resolveModule" => return resolve_module(shared, &a.str(0)),
        "fileFromBytes" => return file_from_bytes(&a.value(0), &a.str(1)),
        "fileFromPath" => return file_from_path(&a.str(0)),
        "fileFromURL" => return handle.block_on(file_from_url(&a.str(0))),
        "http" => return handle.block_on(http(&a)),
        _ => {}
    }

    let core = shared.core()?;
    match name {
        "collection" => Ok(collection_json(core.collection(&a.str(0))?.as_ref())),
        "collections" => Ok(Value::Array(
            core.registry()
                .all()
                .iter()
                .map(|c| collection_json(c))
                .collect(),
        )),
        "findRecordById" => Ok(handle
            .block_on(core.find_record_by_id(&a.str(0), &a.str(1)))?
            .map(Value::Object)
            .unwrap_or(Value::Null)),
        "findRecordsByFilter" => {
            let records = handle.block_on(core.find_records_by_filter(
                &a.str(0),
                &a.str(1),
                &a.str(2),
                a.int(3),
                a.int(4),
                &a.object(5),
            ))?;
            Ok(Value::Array(
                records.into_iter().map(Value::Object).collect(),
            ))
        }
        "countRecords" => Ok(json!(handle.block_on(core.count_records(
            &a.str(0),
            &a.str(1),
            &a.object(2)
        ))?)),
        "save" => {
            let mut data = a.object(1);
            let uploads = take_files(&mut data)?;
            Ok(Value::Object(handle.block_on(
                core.save_record_with_files(&a.str(0), data, uploads),
            )?))
        }
        "delete" => {
            handle.block_on(core.delete_record(&a.str(0), &a.str(1)))?;
            Ok(Value::Null)
        }
        "validatePassword" => {
            let values = handle.block_on(stored_values(&core, &a.str(0), &a.str(1)))?;
            let hash = values
                .get(FIELD_PASSWORD)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if hash.is_empty() {
                return Ok(Value::Bool(false));
            }
            Ok(Value::Bool(handle.block_on(
                crate::records::values::verify_password(a.str(2), hash),
            )))
        }
        "authToken" => {
            let collection = core.collection(&a.str(0))?;
            if !collection.is_auth() {
                return Err(bad("The record isn't from an auth collection."));
            }
            let values = handle.block_on(stored_values(&core, &a.str(0), &a.str(1)))?;
            let record = AuthRecord {
                collection,
                values,
                refreshable: true,
            };
            Ok(Value::String(record.issue_auth_token(&core, None, true)))
        }
        "dbQuery" => handle.block_on(db_query(&core, &a.str(0), &a.object(1), &a.str(2))),
        "settings" => serde_json::to_value(&*core.settings()).map_err(internal),
        "sendMail" => {
            let to: Vec<String> = match a.value(0) {
                Value::Array(items) => items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
                _ => Vec::new(),
            };
            if to.is_empty() {
                return Err(bad("The message has no recipients."));
            }
            for address in to {
                handle
                    .block_on(core.send_mail_now(crate::mail::Message {
                        to: address,
                        subject: a.str(1),
                        html: a.str(2),
                    }))
                    .map_err(internal)?;
            }
            Ok(Value::Null)
        }
        other => Err(bad(format!("Unknown native function {other:?}."))),
    }
}

/// All values of a record, secrets included.
async fn stored_values(
    core: &Core,
    collection: &str,
    id: &str,
) -> Result<Map<String, Value>, ApiError> {
    let c = core.collection(collection)?;
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    crate::records::store::find_by_id(&client, &c, id)
        .await
        .map_err(internal)?
        .ok_or_else(ApiError::not_found)
}

fn random_string(len: i64, alphabet: &str) -> Result<String, ApiError> {
    if !(0..=10_000).contains(&len) {
        return Err(bad("Invalid random string length."));
    }
    if alphabet.is_empty() {
        let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
            .chars()
            .collect();
        return Ok(pick(&chars, len as usize));
    }
    let chars: Vec<char> = alphabet.chars().collect();
    Ok(pick(&chars, len as usize))
}

/// Uniformly random characters (rejection sampling over 32-bit values).
fn pick(chars: &[char], len: usize) -> String {
    let n = chars.len() as u64;
    let limit = (u32::MAX as u64 + 1) / n * n;
    let mut out = String::with_capacity(len);
    while out.chars().count() < len {
        let mut buf = [0u8; 4];
        rand::fill(&mut buf);
        let v = u32::from_le_bytes(buf) as u64;
        if v < limit {
            out.push(chars[(v % n) as usize]);
        }
    }
    out
}

fn hash(algorithm: &str, text: &str) -> Result<String, ApiError> {
    use sha2::Digest;
    Ok(match algorithm {
        "sha256" => hex::encode(sha2::Sha256::digest(text.as_bytes())),
        "sha512" => hex::encode(sha2::Sha512::digest(text.as_bytes())),
        "md5" => hex::encode(md5::Md5::digest(text.as_bytes())),
        other => return Err(bad(format!("Unknown hash {other:?}."))),
    })
}

fn resolve_module(shared: &Shared, path: &str) -> Result<Value, ApiError> {
    let candidate = std::path::Path::new(path);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        shared.dir.join(candidate)
    };
    let resolved = std::fs::canonicalize(&candidate)
        .map_err(|_| bad(format!("Cannot find module {path:?}.")))?;
    if !inside(&shared.dir, &resolved) {
        return Err(bad("Modules must be inside the hooks directory."));
    }
    let source = std::fs::read_to_string(&resolved)
        .map_err(|_| bad(format!("Cannot read module {path:?}.")))?;
    Ok(json!({ "path": resolved.to_string_lossy(), "source": source }))
}

async fn http(a: &Args<'_>) -> Result<Value, ApiError> {
    let url = a.str(0);
    let method = reqwest::Method::from_bytes(a.str(1).as_bytes())
        .map_err(|_| bad("Invalid HTTP method."))?;
    let timeout = (a.int(4).max(1) as u64).min(MAX_HTTP_TIMEOUT);
    let mut request = crate::http::client()
        .request(method, &url)
        .timeout(Duration::from_secs(timeout));
    for (name, value) in a.object(2) {
        if let Some(value) = value.as_str() {
            request = request.header(name, value);
        }
    }
    let body = a.str(3);
    if !body.is_empty() {
        request = request.body(body);
    }
    let response = request
        .send()
        .await
        .map_err(|e| bad(format!("The HTTP request failed: {e}")))?;
    let status = response.status().as_u16();
    let mut headers = Map::new();
    for (name, value) in response.headers() {
        if let Ok(value) = value.to_str() {
            headers.insert(name.to_string(), Value::String(value.to_string()));
        }
    }
    let raw = response
        .text()
        .await
        .map_err(|e| bad(format!("Failed to read the HTTP response: {e}")))?;
    Ok(json!({ "statusCode": status, "headers": headers, "raw": raw }))
}

/// A SQL literal for a `{:name}` placeholder. Text is dollar-quoted with a tag it doesn't
/// contain, so no escaping mode of the server can end it early.
fn literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => dollar_quote(s),
        other => dollar_quote(&other.to_string()),
    }
}

fn dollar_quote(text: &str) -> String {
    let mut tag = String::from("$sb$");
    while text.contains(&tag) {
        tag = format!("$sb{}$", crate::ids::random_id(8));
    }
    format!("{tag}{text}{tag}")
}

fn bind(sql: &str, params: &Map<String, Value>) -> Result<String, ApiError> {
    let re = regex::Regex::new(r"\{:(\w+)\}").expect("valid regex");
    let mut missing = None;
    let bound = re.replace_all(sql, |caps: &regex::Captures| match params.get(&caps[1]) {
        Some(value) => literal(value),
        None => {
            missing = Some(caps[1].to_string());
            String::new()
        }
    });
    if let Some(name) = missing {
        return Err(bad(format!("Missing query parameter {name:?}.")));
    }
    Ok(bound.into_owned())
}

async fn db_query(
    core: &Core,
    sql: &str,
    params: &Map<String, Value>,
    mode: &str,
) -> Result<Value, ApiError> {
    let sql = bind(sql.trim().trim_end_matches(';'), params)?;
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let query_error = |e: tokio_postgres::Error| {
        let message = e
            .as_db_error()
            .map(|db| db.message().to_string())
            .unwrap_or_else(|| e.to_string());
        bad(format!("The query failed: {message}"))
    };
    if mode == "execute" {
        let affected = client
            .execute(sql.as_str(), &[])
            .await
            .map_err(query_error)?;
        return Ok(json!(affected));
    }
    // data-modifying statements work in WITH, so this also covers INSERT ... RETURNING
    let wrapped =
        format!("WITH _q AS ({sql}) SELECT coalesce(json_agg(_q), '[]'::json)::text FROM _q");
    let row = client
        .query_one(wrapped.as_str(), &[])
        .await
        .map_err(query_error)?;
    serde_json::from_str(&row.get::<_, String>(0)).map_err(internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_literals() {
        let params = json!({ "s": "it's $sb$ done", "n": 2, "b": true, "z": null })
            .as_object()
            .cloned()
            .unwrap();
        let sql = bind("SELECT {:s}, {:n}, {:b}, {:z}", &params).unwrap();
        assert!(sql.starts_with("SELECT $sb"));
        assert!(
            !sql.starts_with("SELECT $sb$it"),
            "the tag changes when the text contains it"
        );
        assert!(sql.ends_with(", 2, TRUE, NULL"));
        assert!(bind("SELECT {:missing}", &params).is_err());
    }

    #[test]
    fn random_strings() {
        assert_eq!(random_string(20, "").unwrap().len(), 20);
        assert!(
            random_string(50, "ab")
                .unwrap()
                .chars()
                .all(|c| c == 'a' || c == 'b')
        );
        assert!(random_string(-1, "").is_err());
    }
}
