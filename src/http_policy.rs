//! What the API accepts beyond rate limits (Settings → Security & limits, `security`):
//! browsers on which origins may call it (CORS), how large a request may be, HTTPS only
//! (HSTS), and maintenance, when only superusers get through. Only `/api/` is concerned: the
//! dashboard is served from the API's own address.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::app::App;
use crate::settings::Security;

/// `scheme://host[:port]`, nothing more.
pub fn valid_origin(origin: &str) -> bool {
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    matches!(scheme, "http" | "https")
        && !rest.is_empty()
        && origin.len() <= 255
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}

/// The `Access-Control-Allow-Origin` an origin gets, if it's allowed.
fn allow_origin(security: &Security, origin: &HeaderValue) -> Option<HeaderValue> {
    if security.allowed_origins.iter().any(|o| o == "*") {
        return Some(HeaderValue::from_static("*"));
    }
    let origin_str = origin.to_str().ok()?;
    security
        .allowed_origins
        .iter()
        .any(|o| o.eq_ignore_ascii_case(origin_str))
        .then(|| origin.clone())
}

fn error(status: StatusCode, message: &str) -> Response {
    (
        status,
        axum::Json(json!({ "status": status.as_u16(), "message": message, "data": {} })),
    )
        .into_response()
}

/// Paths open during maintenance: health checks and superusers signing in.
fn open_in_maintenance(path: &str) -> bool {
    path == "/api/health"
        || path.starts_with("/api/collections/_superusers/")
        || path == "/api/collections/_superusers"
}

pub async fn middleware(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    if !request.uri().path().starts_with("/api/") {
        return next.run(request).await;
    }
    let Ok(core) = app.core() else {
        return next.run(request).await;
    };
    let settings = core.settings();
    let security = &settings.security;
    let path = request.uri().path().to_string();
    let allowed = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|o| allow_origin(security, o));

    // a browser asking whether it may
    if request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
    {
        let mut response = StatusCode::NO_CONTENT.into_response();
        let h = response.headers_mut();
        h.insert(header::VARY, HeaderValue::from_static("Origin"));
        if let Some(origin) = allowed {
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            h.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("GET, HEAD, POST, PUT, PATCH, DELETE"),
            );
            let asked = request
                .headers()
                .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
                .cloned()
                .unwrap_or(HeaderValue::from_static("authorization, content-type"));
            h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, asked);
            h.insert(
                header::ACCESS_CONTROL_MAX_AGE,
                HeaderValue::from_static("86400"),
            );
        }
        return response;
    }

    // too large (backup uploads have no limit: they're as large as the installation)
    let limit = u64::from(security.max_upload_mb) * 1024 * 1024;
    let request = if path == "/api/backups/upload" {
        request
    } else {
        use axum::body::HttpBody;
        let length = request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .or_else(|| request.body().size_hint().exact());
        if length.is_some_and(|l| l > limit) {
            let mut response = error(
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!("The request is larger than {} MB.", security.max_upload_mb),
            );
            if let Some(origin) = allowed {
                response
                    .headers_mut()
                    .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            }
            return response;
        }
        // a body without a length is cut off at the limit
        let (parts, body) = request.into_parts();
        Request::from_parts(
            parts,
            Body::new(http_body_util::Limited::new(body, limit as usize)),
        )
    };

    // maintenance: superusers only
    if security.maintenance && !open_in_maintenance(&path) {
        let token = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(crate::auth::token_from_header)
            .map(str::to_string);
        let superuser = match token {
            Some(token) => crate::auth::authenticate(&core, &token)
                .await
                .is_superuser(),
            None => false,
        };
        if !superuser {
            let message = match security.maintenance_message.trim() {
                "" => "Down for maintenance. Try again shortly.",
                m => m,
            };
            let mut response = error(StatusCode::SERVICE_UNAVAILABLE, message);
            let h = response.headers_mut();
            h.insert(header::RETRY_AFTER, HeaderValue::from_static("120"));
            if let Some(origin) = allowed {
                h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            }
            return response;
        }
    }

    let https = settings.meta.app_url.starts_with("https://");
    let hsts = security.hsts && https;
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    if let Some(origin) = allowed {
        if origin != "*" {
            h.append(header::VARY, HeaderValue::from_static("Origin"));
        }
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    if hsts {
        h.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        for ok in [
            "https://app.example.com",
            "http://localhost:5173",
            "https://[::1]:8443",
        ] {
            assert!(valid_origin(ok), "{ok}");
        }
        for bad in [
            "app.example.com",
            "https://a.example.com/",
            "ftp://x",
            "https://",
            "https://a b",
        ] {
            assert!(!valid_origin(bad), "{bad}");
        }
        let security = Security {
            allowed_origins: vec!["https://app.example.com".into()],
            ..Security::default()
        };
        let o = |s: &str| HeaderValue::from_str(s).unwrap();
        assert_eq!(
            allow_origin(&security, &o("https://app.example.com")),
            Some(o("https://app.example.com"))
        );
        assert_eq!(
            allow_origin(&security, &o("https://evil.example.com")),
            None
        );
        assert_eq!(
            allow_origin(&Security::default(), &o("https://any.example.com")),
            Some(o("*"))
        );
    }
}
