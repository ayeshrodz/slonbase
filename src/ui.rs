//! The admin dashboard (`ui/app`, see `ui/README.md`), embedded at build time and served
//! at `/_/`. Files are revalidated with ETags (`no-cache`), so a new build shows up at once.

use std::io::Read;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};

use crate::app::{App, Mode};

/// Only the dashboard's own scripts run; images may be data: (provider logos) and blob:
/// (upload previews) URLs.
const CSP: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'; \
    img-src 'self' data: blob: https://tile.openstreetmap.org; connect-src 'self'; \
    script-src 'self'; frame-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'";

mod files {
    include!(concat!(env!("OUT_DIR"), "/ui_files.rs"));
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json" | "map") => "application/json",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|e| {
            let mut parts = e.trim().split(';');
            parts.next() == Some("gzip") && !parts.any(|p| p.trim() == "q=0")
        })
}

/// For working on the dashboard: `SLONBASE_DEV_UI_DIR=ui/app` serves the files from disk.
fn dev_file(path: &str) -> Option<Response> {
    let dir = std::env::var_os("SLONBASE_DEV_UI_DIR")?;
    if path.split('/').any(|part| part == ".." || part.is_empty()) {
        return Some(StatusCode::NOT_FOUND.into_response());
    }
    let data = std::fs::read(std::path::Path::new(&dir).join(path)).ok();
    Some(match data {
        Some(data) => (
            [
                (header::CONTENT_TYPE, content_type(path)),
                (header::CACHE_CONTROL, "no-store"),
                (header::CONTENT_SECURITY_POLICY, CSP),
            ],
            data,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    })
}

fn file(path: &str, headers: &HeaderMap) -> Response {
    if let Some(response) = dev_file(path) {
        return response;
    }
    let Some((_, data, gzipped, etag)) = files::FILES.iter().find(|(p, _, _, _)| *p == path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::ETAG, HeaderValue::from_static(etag));
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    let matches = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == *etag));
    if matches {
        return (StatusCode::NOT_MODIFIED, response_headers).into_response();
    }
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    response_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response_headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    if !*gzipped {
        return (response_headers, *data).into_response();
    }
    response_headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if accepts_gzip(headers) {
        response_headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        return (response_headers, *data).into_response();
    }
    let mut plain = Vec::new();
    match flate2::read::GzDecoder::new(*data).read_to_end(&mut plain) {
        Ok(_) => (response_headers, plain).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `GET /_/`
pub async fn index(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Mode::Setup(_) = app.mode() {
        return Redirect::temporary("/_/setup").into_response();
    }
    file("index.html", &headers)
}

/// `GET /_/{*path}`
pub async fn asset(Path(path): Path<String>, headers: HeaderMap) -> Response {
    file(&path, &headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded() {
        assert!(
            files::FILES
                .iter()
                .any(|(p, _, gz, _)| *p == "index.html" && *gz)
        );
        let mut headers = HeaderMap::new();
        // revalidation
        let etag = file("main.js", &headers).headers()[header::ETAG].clone();
        let mut cached = HeaderMap::new();
        cached.insert(header::IF_NONE_MATCH, etag);
        assert_eq!(file("main.js", &cached).status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            file("index.html", &headers).headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert!(
            file("index.html", &headers)
                .headers()
                .get(header::CONTENT_ENCODING)
                .is_none()
        );
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("br, gzip"),
        );
        assert_eq!(
            file("index.html", &headers).headers()[header::CONTENT_ENCODING],
            "gzip"
        );
        assert_eq!(
            file("../Cargo.toml", &headers).status(),
            StatusCode::NOT_FOUND
        );
        assert!(!accepts_gzip(&HeaderMap::from_iter([(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip;q=0")
        )])));
    }
}
