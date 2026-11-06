//! HTTP routes and the server loop.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};

use crate::records::{auth_api, oauth2_api};
use crate::{collections, logs, realtime, records, settings};
use serde_json::json;
use tower_http::trace::TraceLayer;

use crate::app::{App, Mode};
use crate::setup;

const HEALTH_DB_TIMEOUT: Duration = Duration::from_secs(3);

/// Record writes may carry file uploads (JSON bodies have their own, smaller limit).
fn upload_limit() -> DefaultBodyLimit {
    // Settings → Security & limits sets it (see http_policy)
    DefaultBodyLimit::disable()
}

pub fn router(app: Arc<App>) -> Router {
    let main = app_router(app.clone());
    if app.project().is_some() {
        return main;
    }
    // projects run their own router (and middleware) under /p/{name}/
    Router::new()
        .route("/p/{project}", any(crate::projects::dispatch))
        .route("/p/{project}/", any(crate::projects::dispatch))
        .route("/p/{project}/{*rest}", any(crate::projects::dispatch))
        .with_state(app.clone())
        .merge(main)
        .fallback(|| async { StatusCode::NOT_FOUND })
        // a project's own host names take precedence over every route
        .layer(axum::middleware::from_fn_with_state(
            app,
            crate::projects::host_dispatch,
        ))
}

/// The routes of the main installation, without the projects.
fn project_api(app: &App) -> Router<Arc<App>> {
    if app.project().is_some() {
        return Router::new();
    }
    use crate::projects;
    Router::new()
        .route("/api/projects", get(projects::list).post(projects::create))
        .route("/api/projects/overview", get(crate::fleet::overview))
        .route(
            "/api/projects/{name}/settings",
            get(crate::fleet::settings).patch(crate::fleet::update_settings),
        )
        .route("/api/projects/{name}/restart", post(crate::fleet::restart))
        .route(
            "/api/projects/{name}",
            get(projects::view)
                .patch(projects::update)
                .delete(projects::delete),
        )
}

fn app_router(app: Arc<App>) -> Router {
    Router::new()
        // relative, so it works under a project's /p/{name}/ too
        .route("/", get(|| async { Redirect::temporary("./_/") }))
        .route("/_/", get(crate::ui::index))
        .route("/_/setup", get(setup::page))
        .route("/_/context.js", get(crate::ui::context))
        .route("/_/{*path}", get(crate::ui::asset))
        .route("/api/health", get(health))
        .route("/api/setup", get(setup::status))
        .route("/api/setup/database/test", post(setup::test_database))
        .route("/api/setup/complete", post(setup::complete))
        .route("/api/settings", get(settings::get).patch(settings::update))
        .route("/api/settings/test/s3", post(settings::test_s3))
        .route("/api/settings/test/email", post(settings::test_email))
        .route("/api/settings/mail/preview", post(settings::mail_preview))
        .route("/api/settings/storage", get(crate::storage_info::info))
        .route(
            "/api/settings/storage/clear-thumbs",
            post(crate::storage_info::clear_thumbs),
        )
        .route("/api/system/status", get(crate::status::status))
        .route(
            "/api/branding/assets",
            get(crate::assets::list)
                .post(crate::assets::upload)
                .layer(DefaultBodyLimit::max(crate::assets::MAX_SIZE + 64 * 1024)),
        )
        .route(
            "/api/branding/assets/{id}",
            get(crate::assets::serve)
                .patch(crate::assets::rename)
                .delete(crate::assets::delete),
        )
        .route(
            "/api/settings/apple/generate-client-secret",
            post(settings::generate_apple_client_secret),
        )
        .route(
            "/api/sql",
            get(crate::sql_console::info).post(crate::sql_console::run),
        )
        .route("/api/crons", get(crate::sql_console::crons))
        .route("/api/jobs/runs", get(crate::jobs::runs))
        .route("/api/team", get(crate::roles::list))
        .route("/api/team/me", get(crate::roles::me))
        .route(
            "/api/team/members/{id}",
            axum::routing::patch(crate::roles::set_role),
        )
        .route("/api/team/keys", post(crate::roles::create_key))
        .route(
            "/api/team/keys/{id}",
            axum::routing::delete(crate::roles::delete_key),
        )
        .route("/api/crons/{id}", post(crate::sql_console::run_cron))
        .route(
            "/api/collections/meta/scaffolds",
            get(collections::api::scaffolds),
        )
        .route(
            "/api/collections/meta/dry-run-view",
            post(collections::api::dry_run_view),
        )
        .route(
            "/api/collections/meta/oauth2-providers",
            get(collections::api::oauth2_providers),
        )
        .route(
            "/api/collections/import",
            axum::routing::put(collections::api::import),
        )
        .route(
            "/api/collections/{collection}/truncate",
            axum::routing::delete(records::api::truncate),
        )
        .route(
            "/api/realtime",
            get(realtime::connect).post(realtime::subscribe),
        )
        .route(
            "/api/collections/{collection}/auth-with-password",
            post(auth_api::auth_with_password),
        )
        .route(
            "/api/collections/{collection}/auth-with-otp",
            post(records::otp::auth_with_otp),
        )
        .route(
            "/api/collections/{collection}/request-otp",
            post(records::otp::request_otp),
        )
        .route(
            "/api/collections/{collection}/auth-with-oauth2",
            post(oauth2_api::auth_with_oauth2),
        )
        .route(
            "/api/oauth2-redirect",
            get(oauth2_api::redirect_get).post(oauth2_api::redirect_post),
        )
        .route(
            "/api/collections/{collection}/records/{id}/external-auths",
            get(oauth2_api::list_external_auths),
        )
        .route(
            "/api/collections/{collection}/records/{id}/external-auths/{provider}",
            axum::routing::delete(oauth2_api::unlink_external_auth),
        )
        .route(
            "/api/collections/{collection}/auth-refresh",
            post(auth_api::auth_refresh),
        )
        .route(
            "/api/collections/{collection}/auth-methods",
            get(auth_api::auth_methods),
        )
        .route(
            "/api/collections/{collection}/request-verification",
            post(auth_api::request_verification),
        )
        .route(
            "/api/collections/{collection}/confirm-verification",
            post(auth_api::confirm_verification),
        )
        .route(
            "/api/collections/{collection}/request-password-reset",
            post(auth_api::request_password_reset),
        )
        .route(
            "/api/collections/{collection}/confirm-password-reset",
            post(auth_api::confirm_password_reset),
        )
        .route(
            "/api/collections/{collection}/request-email-change",
            post(auth_api::request_email_change),
        )
        .route(
            "/api/collections/{collection}/confirm-email-change",
            post(auth_api::confirm_email_change),
        )
        .route(
            "/api/collections/{collection}/impersonate/{id}",
            post(auth_api::impersonate),
        )
        .route(
            "/api/collections",
            get(collections::api::list).post(collections::api::create),
        )
        .route(
            "/api/collections/{collection}",
            get(collections::api::view)
                .patch(collections::api::update)
                .delete(collections::api::delete),
        )
        .route(
            "/api/collections/{collection}/records",
            get(records::api::list)
                .post(records::api::create)
                .layer(upload_limit()),
        )
        .route(
            "/api/collections/{collection}/records/{id}",
            get(records::api::view)
                .patch(records::api::update)
                .delete(records::api::delete)
                .layer(upload_limit()),
        )
        .route(
            "/api/batch",
            post(records::batch::batch).layer(upload_limit()),
        )
        .route("/api/files/token", post(records::files::token))
        .route(
            "/api/files/{collection}/{record}/{filename}",
            get(records::files::serve),
        )
        .route(
            "/api/backups",
            get(crate::backups::list).post(crate::backups::create_handler),
        )
        .route(
            "/api/backups/upload",
            post(crate::backups::upload).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/api/backups/{key}",
            get(crate::backups::download).delete(crate::backups::delete),
        )
        .route(
            "/api/backups/{key}/restore",
            post(crate::backups::restore_handler),
        )
        .route("/api/logs", get(logs::list).delete(logs::truncate))
        .route("/api/logs/stats", get(logs::stats))
        .route("/api/logs/{id}", get(logs::view))
        .merge(project_api(&app))
        .merge(app.extra_routes())
        // global middleware from extensions (`routerUse`), after the guard's checks
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            crate::hooks::request_middleware,
        ))
        // the `fields` parameter, on every JSON answer (routerUse ones included)
        .layer(axum::middleware::from_fn(crate::picker::layer))
        // JSON answers gzipped, but never those that carry secrets
        .layer(axum::middleware::from_fn_with_state(
            app.config().effective.server.compression,
            compress_json,
        ))
        // inner layers run last: the guard runs inside the logger, so rejections are logged
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            crate::guard::middleware,
        ))
        // CORS, sizes, HTTPS and maintenance: outside the guard, so its refusals carry CORS
        // headers too
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            crate::http_policy::middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            logs::middleware,
        ))
        .layer(TraceLayer::new_for_http())
        // outermost: everything is counted, rejections included
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            crate::metrics::middleware,
        ))
        .layer(axum::middleware::from_fn(security_headers))
        .with_state(app)
}

/// Paths whose answers carry tokens or secrets, kept out of compression: compressing a secret
/// next to text an attacker can choose leaks it through the compressed size (BREACH).
fn carries_secrets(path: &str) -> bool {
    path.contains("/auth-with-")
        || path.contains("/auth-refresh")
        || path.contains("/impersonate/")
        || path.starts_with("/api/files/token")
        || path.starts_with("/api/setup")
        || path.starts_with("/api/settings/apple/")
}

/// JSON answers below this size go out as they are.
const COMPRESS_ABOVE: usize = 1024;
/// And above this one too (they're built in memory, but a huge one isn't held twice).
const COMPRESS_BELOW: usize = 8 << 20;

/// Gzips JSON answers of 1 KiB or more for clients that take gzip, in one pass over the whole
/// body (JSON answers are built in memory) with a compressor kept per thread, at level 1: a
/// 30-record page goes from 14.5 KB to 2.2 KB for ~25 us. (tower-http's streaming encoder, at
/// level 2, cost lists a fifth of their throughput on the benchmark; brotli several times
/// more CPU per answer.)
///
/// Never compressed: answers on the paths above, any answer whose JSON has a key that names a
/// secret (`"token"`, `"accessToken"`, `"secret"`, `"password"`...: a custom route's sign-in,
/// say), answers that ask not to be transformed
/// (`Cache-Control: no-transform`), and anything but JSON (files are served as stored).
async fn compress_json(
    axum::extract::State(enabled): axum::extract::State<bool>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if !enabled {
        return next.run(request).await;
    }
    let wants_gzip = accepts_gzip(request.headers());
    let secret = carries_secrets(request.uri().path());
    let response = next.run(request).await;
    let headers = response.headers();
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    let no_transform = headers
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("no-transform"));
    let size = axum::body::HttpBody::size_hint(response.body()).exact();
    let eligible = json
        && !secret
        && !no_transform
        && !headers.contains_key(header::CONTENT_ENCODING)
        && size.is_some_and(|n| (COMPRESS_ABOVE as u64..COMPRESS_BELOW as u64).contains(&n));
    if !eligible {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    parts.headers.append(
        header::VARY,
        axum::http::HeaderValue::from_static("accept-encoding"),
    );
    // the size is exact and bounded, so this doesn't wait on anything nor grow past it
    let bytes = match axum::body::to_bytes(body, COMPRESS_BELOW).await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(error = %err, "failed to read a JSON answer to compress");
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };
    if !wants_gzip || has_secret_key(&bytes) {
        return Response::from_parts(parts, axum::body::Body::from(bytes));
    }
    let gzipped = gzip(&bytes);
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_ENCODING,
        axum::http::HeaderValue::from_static("gzip"),
    );
    Response::from_parts(parts, axum::body::Body::from(gzipped))
}

/// JSON keys whose values are secrets, as a custom route may name them (`"token"` in
/// PocketBase's own answers).
const SECRET_KEYS: &[&[u8]] = &[
    b"\"token\":",
    b"\"accessToken\":",
    b"\"access_token\":",
    b"\"refreshToken\":",
    b"\"refresh_token\":",
    b"\"idToken\":",
    b"\"id_token\":",
    b"\"secret\":",
    b"\"clientSecret\":",
    b"\"client_secret\":",
    b"\"password\":",
    b"\"apiKey\":",
    b"\"api_key\":",
];

/// Whether a JSON answer has a key from [`SECRET_KEYS`] (as serde writes JSON, without spaces).
fn has_secret_key(json: &[u8]) -> bool {
    SECRET_KEYS
        .iter()
        .any(|key| memchr::memmem::find(json, key).is_some())
}

/// Whether `Accept-Encoding` takes gzip (`gzip`, `x-gzip` or `*`, without `q=0`).
fn accepts_gzip(headers: &HeaderMap) -> bool {
    let mut gzip = None;
    let mut any = None;
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else { continue };
        for item in value.split(',') {
            let mut parts = item.split(';');
            let name = parts.next().unwrap_or("").trim();
            let q = parts
                .filter_map(|p| {
                    p.trim()
                        .strip_prefix("q=")
                        .or_else(|| p.trim().strip_prefix("Q="))
                })
                .next()
                .and_then(|q| q.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            if name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip") {
                gzip = Some(q > 0.0);
            } else if name == "*" {
                any = Some(q > 0.0);
            }
        }
    }
    gzip.or(any).unwrap_or(false)
}

/// `data` as a gzip stream, with this thread's compressor (reset, not reallocated).
fn gzip(data: &[u8]) -> Vec<u8> {
    use flate2::{Compress, Compression, FlushCompress, Status};
    thread_local! {
        static DEFLATE: std::cell::RefCell<Compress> =
            std::cell::RefCell::new(Compress::new(Compression::fast(), false));
    }
    // header: magic, deflate, no flags, no mtime, no extra flags, unknown OS
    let mut out = Vec::with_capacity(data.len() / 4 + 64);
    out.extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255]);
    DEFLATE.with(|deflate| {
        let mut deflate = deflate.borrow_mut();
        deflate.reset();
        loop {
            let read = deflate.total_in() as usize;
            if out.capacity() - out.len() < 64 {
                out.reserve(data.len() / 8 + 256);
            }
            match deflate.compress_vec(&data[read..], &mut out, FlushCompress::Finish) {
                Ok(Status::StreamEnd) => break,
                Ok(_) => {}
                Err(err) => unreachable!("deflate into a growing buffer can't fail: {err}"),
            }
        }
    });
    let mut crc = crc32fast::Hasher::new();
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// Headers every response gets unless it set its own: no content sniffing, no framing by
/// other sites, and no full URLs (which may carry tokens) sent to other sites as referrers.
async fn security_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    use axum::http::{HeaderValue, header};
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    for (name, value) in [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "SAMEORIGIN"),
        (header::REFERRER_POLICY, "strict-origin-when-cross-origin"),
    ] {
        headers
            .entry(name)
            .or_insert(HeaderValue::from_static(value));
    }
    response
}

/// What the health check's database probe found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Health {
    Up,
    Down,
    TimedOut,
}

/// How long a probe's result answers health checks: a load balancer polling many times a
/// second costs one query a second, and an outage shows within a second.
const HEALTH_FRESH: Duration = Duration::from_secs(1);

/// The health check's database probe, shared by the requests of the next [`HEALTH_FRESH`];
/// requests that arrive while a probe runs wait for it rather than starting their own. Only
/// the outcome is kept (the error is logged, never answered).
#[derive(Default)]
pub(crate) struct HealthProbe {
    last: std::sync::Mutex<Option<(std::time::Instant, Health)>>,
    probing: tokio::sync::Mutex<()>,
}

impl HealthProbe {
    fn fresh(&self) -> Option<Health> {
        let last = *self.last.lock().expect("health lock");
        last.filter(|(at, _)| at.elapsed() < HEALTH_FRESH)
            .map(|(_, health)| health)
    }

    pub(crate) async fn check(&self, pool: &deadpool_postgres::Pool) -> Health {
        if let Some(health) = self.fresh() {
            return health;
        }
        let _probing = self.probing.lock().await;
        if let Some(health) = self.fresh() {
            return health;
        }
        let probe = async {
            let client = pool.get().await.map_err(|e| e.to_string())?;
            client
                .simple_query("SELECT 1")
                .await
                .map_err(|e| e.to_string())?;
            Ok::<_, String>(())
        };
        let health = match tokio::time::timeout(HEALTH_DB_TIMEOUT, probe).await {
            Ok(Ok(())) => Health::Up,
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "health check: the database is unavailable");
                Health::Down
            }
            Err(_) => Health::TimedOut,
        };
        *self.last.lock().expect("health lock") = Some((std::time::Instant::now(), health));
        health
    }
}

async fn health(
    State(app): State<Arc<App>>,
    auth: crate::auth::RequestAuth,
    client_ip: Option<axum::Extension<crate::guard::ClientIp>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let unhealthy = |message: String| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "code": 503, "message": message, "data": {} })),
        )
            .into_response()
    };

    match app.mode() {
        // the details (which may name hosts) are in the logs, not in the answer to anyone
        Mode::Starting { .. } => unhealthy("Starting.".into()),
        Mode::Setup(_) => Json(json!({
            "code": 200,
            "message": "Setup required.",
            "data": { "setupRequired": true },
        }))
        .into_response(),
        Mode::Ready { core } => {
            // for superusers, what the dashboard shows (like PocketBase)
            let mut data = serde_json::Map::new();
            if auth.is_superuser() {
                let ip = client_ip.and_then(|e| e.0.0);
                let settings = core.settings();
                let proxy_header = settings
                    .trusted_proxy
                    .headers
                    .iter()
                    .map(String::as_str)
                    .chain(["CF-Connecting-IP", "Fly-Client-IP", "X-Forwarded-For"])
                    .find(|h| headers.contains_key(*h))
                    .unwrap_or_default();
                data.insert(
                    "canBackup".into(),
                    core.backup_lock.try_lock().is_ok().into(),
                );
                data.insert(
                    "realIP".into(),
                    ip.map(|ip| ip.to_string()).unwrap_or_default().into(),
                );
                data.insert("possibleProxyHeader".into(), proxy_header.into());
            }
            match core.health.check(&core.pool).await {
                Health::Up => Json(json!({
                    "code": 200,
                    "message": "API is healthy.",
                    "data": data,
                }))
                .into_response(),
                Health::Down => unhealthy("Database unavailable.".into()),
                Health::TimedOut => unhealthy("Database unavailable: timed out".into()),
            }
        }
    }
}

/// Address to reach a server listening on `bind` from the same machine.
fn local_address(bind: &str) -> String {
    match bind.rsplit_once(':') {
        Some(("0.0.0.0" | "", port)) => format!("127.0.0.1:{port}"),
        Some(("[::]", port)) => format!("[::1]:{port}"),
        _ => bind.to_string(),
    }
}

/// Requests `/api/health` from a server on this machine and fails unless it answers 200.
pub async fn check_local_health(bind: &str) -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let address = local_address(bind);
    let check = async {
        let mut stream = tokio::net::TcpStream::connect(&address).await?;
        stream
            .write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head).await?;
        anyhow::Ok(head)
    };
    let head = tokio::time::timeout(Duration::from_secs(5), check)
        .await
        .map_err(|_| anyhow::anyhow!("timed out"))??;
    // "HTTP/1.x 200"
    anyhow::ensure!(
        &head[9..12] == b"200",
        "unhealthy: {}",
        String::from_utf8_lossy(&head)
    );
    Ok(())
}

/// Prints the setup link so the person installing slonbase can open the wizard.
pub fn announce_setup(app: &App, bind: &str) {
    let link = format!(
        "http://{}/_/setup?token={}",
        local_address(bind),
        app.setup_token()
    );
    tracing::info!("setup required, open the setup link printed below");
    eprintln!(
        "\n  ┌─ slonbase setup ──────────────────────────────────────────────\n  \
         │ Open this link to finish setting up slonbase:\n  │\n  │   {link}\n  │\n  \
         │ (replace the host if you're connecting from another machine)\n  \
         └───────────────────────────────────────────────────────────────\n"
    );
}

pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn accepts(value: &str) -> bool {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, value.parse().unwrap());
        accepts_gzip(&headers)
    }

    #[test]
    fn accept_encoding() {
        assert!(accepts("gzip"));
        assert!(accepts("gzip, deflate, br, zstd"));
        assert!(accepts("br;q=1.0, GZIP;q=0.5"));
        assert!(accepts("x-gzip"));
        assert!(accepts("*"));
        assert!(!accepts("br"));
        assert!(!accepts("identity"));
        assert!(!accepts("gzip;q=0"));
        assert!(!accepts("gzip; q=0.0, br"));
        assert!(
            !accepts("*, gzip;q=0"),
            "an explicit refusal beats the wildcard"
        );
        assert!(!accepts("*;q=0"));
        assert!(!accepts_gzip(&HeaderMap::new()));
    }

    #[test]
    fn gzip_round_trips() {
        let data: Vec<u8> = (0..50_000u32)
            .flat_map(|i| format!("{{\"n\":{i},\"s\":\"x{}\"}},", i % 97).into_bytes())
            .collect();
        for input in [&data[..], b"", b"{}"] {
            // twice: the second reuses the thread's compressor
            for _ in 0..2 {
                let mut out = Vec::new();
                flate2::read::GzDecoder::new(&gzip(input)[..])
                    .read_to_end(&mut out)
                    .unwrap();
                assert_eq!(out, input);
            }
        }
        assert!(gzip(&data).len() * 2 < data.len());
    }

    #[tokio::test]
    async fn health_probe_is_shared_for_a_second() {
        // nothing listens on port 1: every probe fails at once
        let pool = crate::db::connect(
            "postgres://u:p@127.0.0.1:1/db?connect_timeout=1",
            &Default::default(),
        )
        .unwrap();
        let probe = HealthProbe::default();
        assert_eq!(probe.check(&pool).await, Health::Down);
        let stamped = probe.last.lock().unwrap().unwrap().0;
        assert_eq!(probe.check(&pool).await, Health::Down);
        assert_eq!(
            probe.last.lock().unwrap().unwrap().0,
            stamped,
            "answered from the last probe"
        );
        // and after a second, probed again
        *probe.last.lock().unwrap() = Some((stamped - HEALTH_FRESH, Health::Up));
        assert_eq!(probe.check(&pool).await, Health::Down);
    }

    #[test]
    fn secret_keys() {
        for json in [
            r#"{"token":"x"}"#,
            r#"{"data":{"access_token":"x"}}"#,
            r#"[{"refreshToken":"x"}]"#,
            r#"{"clientSecret":""}"#,
        ] {
            assert!(has_secret_key(json.as_bytes()), "{json}");
        }
        for json in [
            r#"{"type":"password","name":"token"}"#,
            r#"{"authToken":{"duration":604800}}"#,
            r#"{"title":"a \"token\" in text"}"#,
        ] {
            assert!(!has_secret_key(json.as_bytes()), "{json}");
        }
    }

    #[test]
    fn secret_paths() {
        for path in [
            "/api/collections/users/auth-with-password",
            "/api/collections/users/auth-with-oauth2",
            "/api/collections/users/auth-refresh",
            "/api/collections/_superusers/impersonate/abc",
            "/api/files/token",
            "/api/setup/finish",
            "/api/settings/apple/generate-client-secret",
        ] {
            assert!(carries_secrets(path), "{path}");
        }
        assert!(!carries_secrets("/api/collections/users/records"));
    }
}
