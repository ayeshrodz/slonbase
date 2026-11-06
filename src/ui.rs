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

/// How a file may be cached: for good under the build's version (`v/{version}/`, which the
/// page links to), otherwise revalidated each time.
fn file(path: &str, headers: &HeaderMap) -> Response {
    if let Some(rest) = path.strip_prefix("v/") {
        let Some((version, path)) = rest.split_once('/') else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let mut response = serve(path, headers);
        if version == files::VERSION && response.status().is_success() {
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
        }
        // an older version (a tab open across an upgrade) gets today's file, revalidated
        return response;
    }
    serve(path, headers)
}

fn serve(path: &str, headers: &HeaderMap) -> Response {
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
        return Redirect::temporary("setup").into_response();
    }
    file("index.html", &headers)
}

/// `GET /_/context.js`: what the dashboard manages, before its modules load. `base` is the
/// path the installation is served under, `main` the main installation's dashboard.
pub async fn context(
    State(app): State<Arc<App>>,
    routed: Option<axum::Extension<crate::projects::Routed>>,
) -> Response {
    let (base, main) = match &routed {
        Some(axum::Extension(r)) => (r.base.clone(), r.main_dashboard.clone()),
        None => (String::new(), "/_/".to_string()),
    };
    let mut context = serde_json::json!({ "project": app.project(), "base": base, "main": main });
    if let Ok(core) = app.core() {
        context["branding"] = branding(&core.settings(), &base);
    }
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        format!("window.SLONBASE_CONTEXT = {context};\n"),
    )
        .into_response()
}

/// What the dashboard and its sign-in page show before anyone signs in: the application's
/// name, accent and images, and the sign-in page's settings. All of it is public (the sign-in
/// page shows it); nothing else from the settings is.
fn branding(settings: &crate::settings::Settings, base: &str) -> serde_json::Value {
    let asset = |id: &str| {
        if id.is_empty() {
            serde_json::Value::Null
        } else {
            crate::assets::url(base, id).into()
        }
    };
    let login = &settings.login;
    serde_json::json!({
        "appName": settings.meta.app_name,
        "appURL": settings.meta.app_url,
        "accent": settings.meta.accent_color,
        "logo": asset(&settings.meta.logo),
        "logoDark": asset(&settings.meta.logo_dark),
        "favicon": asset(&settings.meta.favicon),
        "login": {
            "title": login.title,
            "subtitle": login.subtitle,
            "logo": asset(&login.logo),
            "layout": login.layout,
            "background": login.background,
            "backgroundImage": if login.background == "asset" { asset(&login.background_asset) } else { serde_json::Value::Null },
            "notice": login.notice,
            "footerLinks": login.footer_links,
            "showPoweredBy": login.show_powered_by,
        },
    })
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
        // the page links its files under the build's version, cached for good
        let page = {
            let (_, data, _, _) = files::FILES
                .iter()
                .find(|(p, ..)| *p == "index.html")
                .unwrap();
            let mut plain = String::new();
            flate2::read::GzDecoder::new(*data)
                .read_to_string(&mut plain)
                .unwrap();
            plain
        };
        let versioned = format!("./v/{}/main.js", files::VERSION);
        assert!(page.contains(&versioned), "{page}");
        assert!(page.contains("rel=\"modulepreload\""), "{page}");
        assert!(
            page.contains("\"./context.js\""),
            "context.js stays per request"
        );
        let main = file(&versioned[2..], &HeaderMap::new());
        assert_eq!(main.status(), StatusCode::OK);
        assert_eq!(
            main.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        let old = file("v/0000000000000000/main.js", &HeaderMap::new());
        assert_eq!(old.status(), StatusCode::OK);
        assert_eq!(old.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(
            file("v/nothing", &HeaderMap::new()).status(),
            StatusCode::NOT_FOUND
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
