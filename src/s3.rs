//! A small S3 client: the object operations slonbase needs, signed with AWS Signature
//! Version 4. Works with AWS S3 and S3-compatible services (MinIO, Cloudflare R2,
//! Backblaze B2, Wasabi, DigitalOcean Spaces, ...).

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use bytes::Bytes;
use futures_util::StreamExt;
use hmac::{Hmac, KeyInit, Mac};
use md5::Md5;
use reqwest::{Method, StatusCode, Url};
use sha2::{Digest, Sha256};

use crate::storage::{ByteStream, Object};

type HmacSha256 = Hmac<Sha256>;

/// Total time allowed for requests other than downloads (uploads can be large).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// Files larger than this are uploaded in parts (`put_file`).
const MULTIPART_THRESHOLD: u64 = 64 << 20;
/// The size of each part (S3 wants at least 5 MiB, except for the last one).
const PART_SIZE: u64 = 16 << 20;
/// Parts per upload (the S3 limit): larger files get larger parts.
const MAX_PARTS: u64 = 10_000;
/// Objects per `DeleteObjects` request (the S3 limit).
const DELETE_BATCH: usize = 1000;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret: String,
    /// `{endpoint}/{bucket}/{key}` instead of `{bucket}.{endpoint host}/{key}`.
    pub force_path_style: bool,
}

/// A request body.
enum Payload<'a> {
    Empty,
    Bytes(Bytes),
    /// A file streamed from disk (with its size).
    File(&'a std::path::Path, u64),
}

/// An object in a listing.
#[derive(Debug, Clone)]
pub struct ObjectInfo {
    pub key: String,
    pub size: u64,
    /// As returned by S3 (RFC 3339).
    pub modified: String,
}

pub struct Client {
    config: Config,
    /// The endpoint with the bucket applied (as a host or a path prefix).
    base: Url,
    /// The path before object keys (`/` or `/{bucket}/`), already encoded.
    prefix: String,
    /// Keys per list request.
    page_size: usize,
    /// See [`MULTIPART_THRESHOLD`] and [`PART_SIZE`] (smaller in tests).
    multipart_threshold: u64,
    part_size: u64,
}

impl Client {
    pub fn new(config: Config) -> Result<Self> {
        let mut base = Url::parse(config.endpoint.trim_end_matches('/'))
            .with_context(|| format!("invalid S3 endpoint {:?}", config.endpoint))?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            bail!("the S3 endpoint must be an http(s) URL");
        }
        if config.bucket.is_empty() || config.bucket.contains('/') {
            bail!("invalid S3 bucket {:?}", config.bucket);
        }
        let path = base.path().trim_end_matches('/').to_string();
        let prefix = if config.force_path_style {
            format!("{path}/{}/", uri_encode(&config.bucket, true))
        } else {
            let host = format!("{}.{}", config.bucket, base.host_str().expect("checked"));
            base.set_host(Some(&host))
                .with_context(|| format!("invalid S3 bucket {:?}", config.bucket))?;
            format!("{path}/")
        };
        base.set_path("");
        Ok(Self {
            config,
            base,
            prefix,
            page_size: 1000,
            multipart_threshold: MULTIPART_THRESHOLD,
            part_size: PART_SIZE,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    fn host(&self) -> String {
        let host = self.base.host_str().expect("checked in new");
        match self.base.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        }
    }

    /// Sends a signed request for `key` (empty for bucket-level requests).
    async fn send(
        &self,
        method: Method,
        key: &str,
        query: &[(&str, &str)],
        body: Payload<'_>,
        extra_headers: &[(&str, String)],
        timeout: Option<Duration>,
    ) -> Result<reqwest::Response> {
        let path = format!("{}{}", self.prefix, uri_encode(key, false));
        let mut query: Vec<(String, String)> = query
            .iter()
            .map(|(k, v)| (uri_encode(k, true), uri_encode(v, true)))
            .collect();
        query.sort();
        let query_string = query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let url = if query_string.is_empty() {
            format!("{}{}", self.base.as_str().trim_end_matches('/'), path)
        } else {
            format!(
                "{}{}?{query_string}",
                self.base.as_str().trim_end_matches('/'),
                path
            )
        };

        let payload_hash = match &body {
            // hashing a large upload takes a while, so not on the async threads
            Payload::Bytes(data) if data.len() > 1 << 20 => {
                let data = data.clone();
                tokio::task::spawn_blocking(move || hex::encode(Sha256::digest(&data))).await?
            }
            Payload::Bytes(data) => hex::encode(Sha256::digest(data)),
            Payload::File(path, _) => {
                let path = path.to_path_buf();
                tokio::task::spawn_blocking(move || -> Result<String> {
                    use std::io::Read;
                    let mut file = std::fs::File::open(&path)?;
                    let mut hasher = Sha256::new();
                    let mut buf = vec![0u8; 256 * 1024];
                    loop {
                        let n = file.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        hasher.update(&buf[..n]);
                    }
                    Ok(hex::encode(hasher.finalize()))
                })
                .await??
            }
            Payload::Empty => EMPTY_SHA256.to_string(),
        };
        let now = chrono::Utc::now();
        let mut headers = vec![
            ("host".to_string(), self.host()),
            ("x-amz-content-sha256".to_string(), payload_hash.clone()),
            (
                "x-amz-date".to_string(),
                now.format("%Y%m%dT%H%M%SZ").to_string(),
            ),
        ];
        for (name, value) in extra_headers {
            headers.push((name.to_ascii_lowercase(), value.clone()));
        }
        let authorization = sign(
            &self.config,
            method.as_str(),
            &path,
            &query_string,
            &mut headers,
            &payload_hash,
            &now,
        );

        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut request = crate::http::client()
                .request(method.clone(), &url)
                .header("authorization", &authorization);
            for (name, value) in &headers {
                if name != "host" {
                    request = request.header(name.as_str(), value.as_str());
                }
            }
            if let Some(timeout) = timeout {
                request = request.timeout(timeout);
            }
            match &body {
                Payload::Empty => {}
                Payload::Bytes(data) => request = request.body(data.clone()),
                Payload::File(path, size) => {
                    let file = tokio::fs::File::open(path).await?;
                    request = request.header("content-length", size.to_string()).body(
                        reqwest::Body::wrap_stream(crate::storage::file_chunks(file)),
                    );
                }
            }
            match request.send().await {
                // every request here is idempotent, so a failed attempt can be repeated
                Ok(response) if response.status().is_server_error() && attempt < 2 => {}
                Ok(response) => return Ok(response),
                Err(err) if attempt < 2 && (err.is_connect() || err.is_timeout()) => {}
                Err(err) => return Err(anyhow!(err).context("S3 request failed")),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// A presigned URL that GETs `key` (SigV4 query authentication), signed at `now` and
    /// valid for `expires` seconds (S3 allows up to 7 days). `params` are signed with it,
    /// such as `response-content-type` and `response-content-disposition`, which make S3
    /// answer with those headers.
    pub fn presign_get(
        &self,
        key: &str,
        now: &chrono::DateTime<chrono::Utc>,
        expires: u64,
        params: &[(&str, &str)],
    ) -> String {
        let path = format!("{}{}", self.prefix, uri_encode(key, false));
        let credential = format!("{}/{}", self.config.access_key, scope(&self.config, now));
        let date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let expires = expires.to_string();
        let mut query: Vec<(String, String)> = [
            ("X-Amz-Algorithm", "AWS4-HMAC-SHA256"),
            ("X-Amz-Credential", credential.as_str()),
            ("X-Amz-Date", date.as_str()),
            ("X-Amz-Expires", expires.as_str()),
            ("X-Amz-SignedHeaders", "host"),
        ]
        .iter()
        .chain(params)
        .map(|(k, v)| (uri_encode(k, true), uri_encode(v, true)))
        .collect();
        query.sort();
        let query = query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let canonical_request = format!(
            "GET\n{path}\n{query}\nhost:{}\n\nhost\nUNSIGNED-PAYLOAD",
            self.host()
        );
        let signature = signature(&self.config, &canonical_request, now);
        format!(
            "{}{path}?{query}&X-Amz-Signature={signature}",
            self.base.as_str().trim_end_matches('/')
        )
    }

    pub async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        let response = self
            .send(
                Method::PUT,
                key,
                &[],
                Payload::Bytes(data),
                &[],
                Some(REQUEST_TIMEOUT),
            )
            .await?;
        ensure_success(response, "upload").await.map(|_| ())
    }

    /// Opens an object for streaming, or `None` when it doesn't exist.
    pub async fn open(&self, key: &str) -> Result<Option<Object>> {
        let response = self
            .send(Method::GET, key, &[], Payload::Empty, &[], None)
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            ensure_success(response, "download").await?;
            bail!("unreachable: the response was an error");
        }
        let size = response.content_length().unwrap_or_default();
        let stream: ByteStream = Box::pin(
            response
                .bytes_stream()
                .map(|chunk| chunk.map_err(std::io::Error::other)),
        );
        Ok(Some(Object { size, stream }))
    }

    /// The size of an object, or `None` when it doesn't exist.
    pub async fn size(&self, key: &str) -> Result<Option<u64>> {
        let response = self
            .send(
                Method::HEAD,
                key,
                &[],
                Payload::Empty,
                &[],
                Some(REQUEST_TIMEOUT),
            )
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        // a HEAD response has no body: its length is the object's
        let size = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or_default();
        ensure_success(response, "check").await?;
        Ok(Some(size))
    }

    /// Opens bytes `start..=end` of an object for streaming.
    pub async fn open_range(&self, key: &str, start: u64, end: u64) -> Result<Option<Object>> {
        let response = self
            .send(
                Method::GET,
                key,
                &[],
                Payload::Empty,
                &[("range", format!("bytes={start}-{end}"))],
                None,
            )
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            ensure_success(response, "download").await?;
            bail!("unreachable: the response was an error");
        }
        let size = response.content_length().unwrap_or_default();
        let stream: ByteStream = Box::pin(
            response
                .bytes_stream()
                .map(|chunk| chunk.map_err(std::io::Error::other)),
        );
        Ok(Some(Object { size, stream }))
    }

    pub async fn exists(&self, key: &str) -> Result<bool> {
        let response = self
            .send(
                Method::HEAD,
                key,
                &[],
                Payload::Empty,
                &[],
                Some(REQUEST_TIMEOUT),
            )
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        ensure_success(response, "check").await.map(|_| true)
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let response = self
            .send(
                Method::DELETE,
                key,
                &[],
                Payload::Empty,
                &[],
                Some(REQUEST_TIMEOUT),
            )
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        ensure_success(response, "delete").await.map(|_| ())
    }

    /// Lists the keys starting with `prefix`.
    pub async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .list_objects(prefix)
            .await?
            .into_iter()
            .map(|o| o.key)
            .collect())
    }

    /// Lists the objects whose keys start with `prefix`.
    pub async fn list_objects(&self, prefix: &str) -> Result<Vec<ObjectInfo>> {
        let mut objects = Vec::new();
        let mut token = String::new();
        loop {
            let page_size = self.page_size.to_string();
            let mut query = vec![
                ("list-type", "2"),
                ("max-keys", page_size.as_str()),
                ("prefix", prefix),
            ];
            if !token.is_empty() {
                query.push(("continuation-token", &token));
            }
            let response = self
                .send(
                    Method::GET,
                    "",
                    &query,
                    Payload::Empty,
                    &[],
                    Some(REQUEST_TIMEOUT),
                )
                .await?;
            let body = ensure_success(response, "list").await?;
            for contents in xml_blocks(&body, "Contents") {
                let first = |tag| {
                    xml_values(contents, tag)
                        .into_iter()
                        .next()
                        .unwrap_or_default()
                };
                objects.push(ObjectInfo {
                    key: first("Key"),
                    size: first("Size").parse().unwrap_or_default(),
                    modified: first("LastModified"),
                });
            }
            let truncated =
                xml_values(&body, "IsTruncated").first().map(String::as_str) == Some("true");
            token = xml_values(&body, "NextContinuationToken")
                .into_iter()
                .next()
                .unwrap_or_default();
            if !truncated || token.is_empty() {
                return Ok(objects);
            }
        }
    }

    /// Uploads a file from disk without reading it into memory: in one request, or in parts
    /// when it's large (each part is retried on its own, and objects can pass the 5 GiB a
    /// single upload allows).
    pub async fn put_file(&self, key: &str, path: &std::path::Path) -> Result<()> {
        let size = tokio::fs::metadata(path).await?.len();
        if size > self.multipart_threshold {
            return self.put_file_in_parts(key, path, size).await;
        }
        let response = self
            .send(Method::PUT, key, &[], Payload::File(path, size), &[], None)
            .await?;
        ensure_success(response, "upload").await.map(|_| ())
    }

    /// A multipart upload: `CreateMultipartUpload`, an `UploadPart` per part, then
    /// `CompleteMultipartUpload`; on failure the upload is aborted, so no parts are left
    /// behind (and billed).
    async fn put_file_in_parts(&self, key: &str, path: &std::path::Path, size: u64) -> Result<()> {
        let response = self
            .send(
                Method::POST,
                key,
                &[("uploads", "")],
                Payload::Empty,
                &[],
                Some(REQUEST_TIMEOUT),
            )
            .await?;
        let body = ensure_success(response, "multipart upload creation").await?;
        let upload_id = xml_values(&body, "UploadId")
            .into_iter()
            .next()
            .filter(|id| !id.is_empty())
            .context("S3 returned no multipart upload id")?;
        let result = self.upload_parts(key, path, size, &upload_id).await;
        if result.is_err() {
            let aborted = self
                .send(
                    Method::DELETE,
                    key,
                    &[("uploadId", upload_id.as_str())],
                    Payload::Empty,
                    &[],
                    Some(REQUEST_TIMEOUT),
                )
                .await;
            if let Err(err) = aborted {
                tracing::warn!(key, error = %format!("{err:#}"), "failed to abort a multipart upload");
            }
        }
        result
    }

    async fn upload_parts(
        &self,
        key: &str,
        path: &std::path::Path,
        size: u64,
        upload_id: &str,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;
        let part_size = self.part_size.max(size.div_ceil(MAX_PARTS));
        let mut file = tokio::fs::File::open(path).await?;
        let mut etags = Vec::new();
        let mut sent = 0u64;
        while sent < size {
            let len = part_size.min(size - sent) as usize;
            let mut part = vec![0u8; len];
            file.read_exact(&mut part)
                .await
                .context("reading the file to upload")?;
            let number = (etags.len() + 1).to_string();
            let response = self
                .send(
                    Method::PUT,
                    key,
                    &[("partNumber", number.as_str()), ("uploadId", upload_id)],
                    Payload::Bytes(Bytes::from(part)),
                    &[],
                    Some(REQUEST_TIMEOUT),
                )
                .await?;
            let etag = response
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            ensure_success(response, "part upload").await?;
            if etag.is_empty() {
                bail!("S3 returned no ETag for part {number}");
            }
            etags.push(etag);
            sent += len as u64;
        }

        let mut xml = String::from("<CompleteMultipartUpload>");
        for (i, etag) in etags.iter().enumerate() {
            xml.push_str(&format!(
                "<Part><PartNumber>{}</PartNumber><ETag>{}</ETag></Part>",
                i + 1,
                xml_escape(etag)
            ));
        }
        xml.push_str("</CompleteMultipartUpload>");
        let response = self
            .send(
                Method::POST,
                key,
                &[("uploadId", upload_id)],
                Payload::Bytes(Bytes::from(xml)),
                &[("content-type", "application/xml".to_string())],
                Some(REQUEST_TIMEOUT),
            )
            .await?;
        // S3 can answer 200 with an error in the body
        let body = ensure_success(response, "multipart upload completion").await?;
        if body.contains("<Error>") {
            let codes = xml_values(&body, "Code");
            bail!("S3 failed to complete the upload: {}", codes.join(", "));
        }
        Ok(())
    }

    /// Creates the bucket (used by tests; an existing bucket of the same owner is fine).
    pub async fn create_bucket(&self) -> Result<()> {
        let response = self
            .send(
                Method::PUT,
                "",
                &[],
                Payload::Empty,
                &[],
                Some(REQUEST_TIMEOUT),
            )
            .await?;
        if response.status() == StatusCode::CONFLICT {
            return Ok(());
        }
        ensure_success(response, "bucket creation")
            .await
            .map(|_| ())
    }

    /// Deletes several objects (missing ones are ignored).
    pub async fn delete_many(&self, keys: &[String]) -> Result<()> {
        for batch in keys.chunks(DELETE_BATCH) {
            let mut xml = String::from("<Delete><Quiet>true</Quiet>");
            for key in batch {
                xml.push_str("<Object><Key>");
                xml.push_str(&xml_escape(key));
                xml.push_str("</Key></Object>");
            }
            xml.push_str("</Delete>");
            let body = Bytes::from(xml);
            let md5 = B64.encode(Md5::digest(&body));
            let response = self
                .send(
                    Method::POST,
                    "",
                    &[("delete", "")],
                    Payload::Bytes(body),
                    &[
                        ("content-md5", md5),
                        ("content-type", "application/xml".to_string()),
                    ],
                    Some(REQUEST_TIMEOUT),
                )
                .await?;
            let body = ensure_success(response, "delete").await?;
            if body.contains("<Error>") {
                let codes = xml_values(&body, "Code");
                bail!("S3 failed to delete some objects: {}", codes.join(", "));
            }
        }
        Ok(())
    }
}

/// Returns the body of a successful response, or the S3 error.
async fn ensure_success(response: reqwest::Response, action: &str) -> Result<String> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status.is_success() {
        return Ok(body);
    }
    let code = xml_values(&body, "Code").into_iter().next();
    let message = xml_values(&body, "Message").into_iter().next();
    match (code, message) {
        (Some(code), Some(message)) => bail!("S3 {action} failed ({status}): {code}: {message}"),
        (Some(code), None) => bail!("S3 {action} failed ({status}): {code}"),
        _ => bail!("S3 {action} failed ({status})"),
    }
}

/// Signs a request (AWS Signature Version 4) and returns the `Authorization` header.
/// `headers` must include `host`, `x-amz-content-sha256` and `x-amz-date`; they're
/// sorted in place. `path` and `query` must already be URI-encoded.
fn sign(
    config: &Config,
    method: &str,
    path: &str,
    query: &str,
    headers: &mut [(String, String)],
    payload_hash: &str,
    now: &chrono::DateTime<chrono::Utc>,
) -> String {
    headers.sort_by(|a, b| a.0.cmp(&b.0));
    let canonical_headers: String = headers
        .iter()
        .map(|(k, v)| format!("{k}:{}\n", v.trim()))
        .collect();
    let signed_headers = headers
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_request =
        format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
    let signature = signature(config, &canonical_request, now);
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={signed_headers}, Signature={signature}",
        config.access_key,
        scope(config, now)
    )
}

/// The credential scope of a signature made at `now`.
fn scope(config: &Config, now: &chrono::DateTime<chrono::Utc>) -> String {
    format!("{}/{}/s3/aws4_request", now.format("%Y%m%d"), config.region)
}

/// The SigV4 signature of a canonical request made at `now`.
fn signature(
    config: &Config,
    canonical_request: &str,
    now: &chrono::DateTime<chrono::Utc>,
) -> String {
    let date = now.format("%Y%m%d").to_string();
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        now.format("%Y%m%dT%H%M%SZ"),
        scope(config, now),
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );
    let mut key = hmac(format!("AWS4{}", config.secret).as_bytes(), date.as_bytes());
    for part in [config.region.as_str(), "s3", "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    hex::encode(hmac(&key, string_to_sign.as_bytes()))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// URI-encodes like SigV4 expects: unreserved characters stay, `/` stays in paths.
fn uri_encode(s: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' if !encode_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The raw inner XML of every `<tag>...</tag>` element.
fn xml_blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        let Some(end) = rest.find(&close) else { break };
        out.push(&rest[..end]);
        rest = &rest[end + close.len()..];
    }
    out
}

/// The text of every `<tag>...</tag>` element (S3's responses are simple and flat
/// enough that this beats pulling in an XML parser).
fn xml_values(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        let Some(end) = rest.find(&close) else { break };
        out.push(xml_unescape(&rest[..end]));
        rest = &rest[end + close.len()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn example_config() -> Config {
        Config {
            endpoint: "https://s3.amazonaws.com".into(),
            bucket: "examplebucket".into(),
            region: "us-east-1".into(),
            access_key: "AKIAIOSFODNN7EXAMPLE".into(),
            secret: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            force_path_style: false,
        }
    }

    fn example_time() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap()
    }

    // the example from AWS's "Authenticating Requests: Using Query Parameters" docs
    #[test]
    fn presigns_get_object() {
        let client = Client::new(example_config()).unwrap();
        let url = client.presign_get("test.txt", &example_time(), 86400, &[]);
        assert_eq!(
            url,
            "https://examplebucket.s3.amazonaws.com/test.txt\
             ?X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request\
             &X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host\
             &X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
        // response headers are signed along, in order
        let url = client.presign_get(
            "a b.txt",
            &example_time(),
            60,
            &[(
                "response-content-disposition",
                "attachment; filename=\"a b.txt\"",
            )],
        );
        assert!(
            url.starts_with("https://examplebucket.s3.amazonaws.com/a%20b.txt?"),
            "{url}"
        );
        assert!(
            url.contains("&X-Amz-SignedHeaders=host&response-content-disposition=attachment%3B%20filename%3D%22a%20b.txt%22&X-Amz-Signature="),
            "{url}"
        );
    }

    // the examples from AWS's "Signature Calculations for the Authorization Header" docs
    #[test]
    fn signs_get_object() {
        let mut headers = vec![
            ("host".into(), "examplebucket.s3.amazonaws.com".into()),
            ("range".into(), "bytes=0-9".into()),
            ("x-amz-content-sha256".into(), EMPTY_SHA256.into()),
            ("x-amz-date".into(), "20130524T000000Z".into()),
        ];
        let auth = sign(
            &example_config(),
            "GET",
            "/test.txt",
            "",
            &mut headers,
            EMPTY_SHA256,
            &example_time(),
        );
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn signs_put_object() {
        let payload = "Welcome to Amazon S3.";
        let hash = hex::encode(Sha256::digest(payload));
        let mut headers = vec![
            ("host".into(), "examplebucket.s3.amazonaws.com".into()),
            ("date".into(), "Fri, 24 May 2013 00:00:00 GMT".into()),
            ("x-amz-storage-class".into(), "REDUCED_REDUNDANCY".into()),
            ("x-amz-content-sha256".into(), hash.clone()),
            ("x-amz-date".into(), "20130524T000000Z".into()),
        ];
        let auth = sign(
            &example_config(),
            "PUT",
            &format!("/{}", uri_encode("test$file.text", false)),
            "",
            &mut headers,
            &hash,
            &example_time(),
        );
        assert!(
            auth.ends_with(
                "Signature=98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
            ),
            "{auth}"
        );
    }

    #[test]
    fn signs_list_objects() {
        let mut headers = vec![
            ("host".into(), "examplebucket.s3.amazonaws.com".into()),
            ("x-amz-content-sha256".into(), EMPTY_SHA256.into()),
            ("x-amz-date".into(), "20130524T000000Z".into()),
        ];
        let auth = sign(
            &example_config(),
            "GET",
            "/",
            "max-keys=2&prefix=J",
            &mut headers,
            EMPTY_SHA256,
            &example_time(),
        );
        assert!(
            auth.ends_with(
                "Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
            ),
            "{auth}"
        );
    }

    #[test]
    fn urls_by_style() {
        let virtual_hosted = Client::new(example_config()).unwrap();
        assert_eq!(virtual_hosted.host(), "examplebucket.s3.amazonaws.com");
        assert_eq!(virtual_hosted.prefix, "/");

        let path_style = Client::new(Config {
            endpoint: "http://localhost:9000/".into(),
            force_path_style: true,
            ..example_config()
        })
        .unwrap();
        assert_eq!(path_style.host(), "localhost:9000");
        assert_eq!(path_style.prefix, "/examplebucket/");

        assert!(
            Client::new(Config {
                endpoint: "ftp://x".into(),
                ..example_config()
            })
            .is_err()
        );
        assert!(
            Client::new(Config {
                bucket: "a/b".into(),
                ..example_config()
            })
            .is_err()
        );
    }

    /// Runs against an S3-compatible server when `SLONBASE_TEST_S3_ENDPOINT` is set
    /// (with `SLONBASE_TEST_S3_ACCESS_KEY` and `SLONBASE_TEST_S3_SECRET`).
    pub(crate) fn test_config(bucket: &str) -> Option<Config> {
        let endpoint = std::env::var("SLONBASE_TEST_S3_ENDPOINT").ok()?;
        Some(Config {
            endpoint,
            bucket: bucket.into(),
            region: std::env::var("SLONBASE_TEST_S3_REGION").unwrap_or_else(|_| "us-east-1".into()),
            access_key: std::env::var("SLONBASE_TEST_S3_ACCESS_KEY").unwrap_or_default(),
            secret: std::env::var("SLONBASE_TEST_S3_SECRET").unwrap_or_default(),
            force_path_style: true,
        })
    }

    #[tokio::test]
    async fn against_a_server() {
        let bucket = format!("slonbase-unit-{}", crate::ids::random_id(8));
        let Some(config) = test_config(&bucket) else {
            return;
        };
        let mut client = Client::new(config.clone()).unwrap();
        client.create_bucket().await.unwrap();

        client
            .put("a/b/one.txt", Bytes::from_static(b"hello"))
            .await
            .unwrap();
        let object = client.open("a/b/one.txt").await.unwrap().unwrap();
        assert_eq!(object.size, 5);
        let data: Vec<Bytes> = object.stream.map(|c| c.unwrap()).collect().await;
        assert_eq!(data.concat(), b"hello");
        assert!(client.exists("a/b/one.txt").await.unwrap());
        assert!(!client.exists("a/b/missing.txt").await.unwrap());
        assert!(client.open("a/b/missing.txt").await.unwrap().is_none());
        client.delete("a/b/missing.txt").await.unwrap();

        // listing pages through all keys; batch deletes
        client.page_size = 7;
        let keys: Vec<String> = (0..23).map(|i| format!("p/k{i:02}")).collect();
        for key in &keys {
            client.put(key, Bytes::from_static(b"x")).await.unwrap();
        }
        client
            .put("pq/other", Bytes::from_static(b"x"))
            .await
            .unwrap();
        let mut listed = client.list("p/").await.unwrap();
        listed.sort();
        assert_eq!(listed, keys);
        client.delete_many(&listed).await.unwrap();
        assert!(client.list("p/").await.unwrap().is_empty());
        assert_eq!(client.list("pq/").await.unwrap(), vec!["pq/other"]);

        // a file in parts (5 MiB is the smallest part S3 takes)
        client.multipart_threshold = 5 << 20;
        client.part_size = 5 << 20;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        let data: Vec<u8> = (0..(11 << 20) + 123).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        client.put_file("big/file.bin", &path).await.unwrap();
        let object = client.open("big/file.bin").await.unwrap().unwrap();
        assert_eq!(object.size, data.len() as u64);
        let read: Vec<Bytes> = object.stream.map(|c| c.unwrap()).collect().await;
        assert!(read.concat() == data, "the parts are put together in order");
        client.delete("big/file.bin").await.unwrap();

        // a wrong secret is rejected by the server
        let wrong = Client::new(Config {
            secret: "wrong".into(),
            ..config
        })
        .unwrap();
        let err = wrong
            .put("a/x", Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("403"), "{err:#}");
    }

    /// A mock S3 server for multipart uploads: uploads' parts, completed objects, and aborted
    /// upload ids. `fail_part` answers that part number with an error.
    #[derive(Clone, Default)]
    struct MockS3 {
        parts: Arc<Mutex<std::collections::BTreeMap<u32, Vec<u8>>>>,
        objects: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        aborted: Arc<Mutex<Vec<String>>>,
        fail_part: Option<u32>,
    }

    async fn mock_s3(
        axum::extract::State(mock): axum::extract::State<MockS3>,
        method: axum::http::Method,
        uri: axum::http::Uri,
        body: Bytes,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        let query: HashMap<String, String> =
            form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
                .into_owned()
                .collect();
        let key = uri.path().to_string();
        match (method.as_str(), query.get("uploadId"), query.get("partNumber")) {
            ("POST", None, _) if query.contains_key("uploads") => {
                "<InitiateMultipartUploadResult><UploadId>up-1</UploadId></InitiateMultipartUploadResult>"
                    .into_response()
            }
            ("PUT", Some(id), Some(n)) if id == "up-1" => {
                let n: u32 = n.parse().unwrap();
                if mock.fail_part == Some(n) {
                    return (axum::http::StatusCode::BAD_REQUEST, "<Error><Code>Nope</Code></Error>")
                        .into_response();
                }
                mock.parts.lock().unwrap().insert(n, body.to_vec());
                ([("etag", format!("\"etag-{n}\""))], "").into_response()
            }
            ("POST", Some(id), None) if id == "up-1" => {
                let xml = String::from_utf8(body.to_vec()).unwrap();
                let parts = mock.parts.lock().unwrap();
                let numbers = xml_values(&xml, "PartNumber");
                let etags = xml_values(&xml, "ETag");
                assert_eq!(numbers.len(), parts.len());
                let mut object = Vec::new();
                for (n, etag) in numbers.iter().zip(&etags) {
                    let n: u32 = n.parse().unwrap();
                    assert_eq!(etag, &format!("\"etag-{n}\""));
                    object.extend_from_slice(&parts[&n]);
                }
                mock.objects.lock().unwrap().insert(key, object);
                "<CompleteMultipartUploadResult></CompleteMultipartUploadResult>".into_response()
            }
            ("DELETE", Some(id), None) => {
                mock.aborted.lock().unwrap().push(id.clone());
                axum::http::StatusCode::NO_CONTENT.into_response()
            }
            _ => axum::http::StatusCode::NOT_IMPLEMENTED.into_response(),
        }
    }

    async fn start_mock(mock: MockS3) -> Client {
        let router = axum::Router::new().fallback(mock_s3).with_state(mock);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut client = Client::new(Config {
            endpoint: format!("http://{addr}"),
            bucket: "b".into(),
            region: "us-east-1".into(),
            access_key: "k".into(),
            secret: "s".into(),
            force_path_style: true,
        })
        .unwrap();
        client.multipart_threshold = 1000;
        client.part_size = 400;
        client
    }

    #[tokio::test]
    async fn multipart_uploads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        let data: Vec<u8> = (0..1100u32).map(|i| (i % 256) as u8).collect();
        std::fs::write(&path, &data).unwrap();

        // 400 + 400 + 300 bytes, put together in order
        let mock = MockS3::default();
        let client = start_mock(mock.clone()).await;
        client.put_file("dir/f.bin", &path).await.unwrap();
        assert_eq!(mock.parts.lock().unwrap().len(), 3);
        assert_eq!(mock.objects.lock().unwrap()["/b/dir/f.bin"], data);
        assert!(mock.aborted.lock().unwrap().is_empty());

        // a failed part aborts the upload
        let mock = MockS3 {
            fail_part: Some(2),
            ..Default::default()
        };
        let client = start_mock(mock.clone()).await;
        let err = client.put_file("dir/f.bin", &path).await.unwrap_err();
        assert!(format!("{err:#}").contains("Nope"), "{err:#}");
        assert_eq!(*mock.aborted.lock().unwrap(), vec!["up-1".to_string()]);
        assert!(mock.objects.lock().unwrap().is_empty());

        // small files are a single upload
        std::fs::write(&path, b"small").unwrap();
        let mock = MockS3::default();
        let client = start_mock(mock.clone()).await;
        assert!(
            client.put_file("dir/f.bin", &path).await.is_err(),
            "the mock has no plain PUT"
        );
        assert!(mock.parts.lock().unwrap().is_empty());
    }

    #[test]
    fn parses_xml() {
        let xml = "<ListBucketResult><IsTruncated>true</IsTruncated>\
                   <Contents><Key>a/b&amp;c.txt</Key></Contents><Contents><Key>a/d</Key></Contents>\
                   <NextContinuationToken>tok</NextContinuationToken></ListBucketResult>";
        assert_eq!(xml_values(xml, "Key"), vec!["a/b&c.txt", "a/d"]);
        assert_eq!(xml_values(xml, "NextContinuationToken"), vec!["tok"]);
        assert_eq!(uri_encode("a b/c~", false), "a%20b/c~");
        assert_eq!(uri_encode("a/b", true), "a%2Fb");
    }
}
