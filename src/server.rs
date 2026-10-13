//! HTTP routes and the server loop.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};

use crate::records::{auth_api, oauth2_api};
use crate::{collections, realtime, records, settings};
use serde_json::json;
use tower_http::trace::TraceLayer;

use crate::app::{App, Mode};
use crate::setup;

const HEALTH_DB_TIMEOUT: Duration = Duration::from_secs(3);

/// Record writes may carry file uploads (JSON bodies have their own, smaller limit).
fn upload_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(records::files::MAX_MULTIPART_SIZE)
}

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/_/") }))
        .route("/_/", get(dashboard))
        .route("/_/setup", get(setup::page))
        .route("/api/health", get(health))
        .route("/api/setup", get(setup::status))
        .route("/api/setup/database/test", post(setup::test_database))
        .route("/api/setup/complete", post(setup::complete))
        .route("/api/settings", get(settings::get).patch(settings::update))
        .route("/api/settings/test/s3", post(settings::test_s3))
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
        .route("/api/files/token", post(records::files::token))
        .route(
            "/api/files/{collection}/{record}/{filename}",
            get(records::files::serve),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(app)
}

async fn health(State(app): State<Arc<App>>) -> Response {
    let unhealthy = |message: String| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "code": 503, "message": message, "data": {} })),
        )
            .into_response()
    };

    match app.mode() {
        Mode::Starting { status } => unhealthy(format!("Starting: {status}")),
        Mode::Setup(_) => Json(json!({
            "code": 200,
            "message": "Setup required.",
            "data": { "setupRequired": true },
        }))
        .into_response(),
        Mode::Ready { core } => {
            let check = async {
                let client = core.pool.get().await.map_err(|e| e.to_string())?;
                client
                    .simple_query("SELECT 1")
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(())
            };
            match tokio::time::timeout(HEALTH_DB_TIMEOUT, check).await {
                Ok(Ok(())) => Json(json!({
                    "code": 200,
                    "message": "API is healthy.",
                    "data": {},
                }))
                .into_response(),
                Ok(Err(err)) => unhealthy(format!("Database unavailable: {err}")),
                Err(_) => unhealthy("Database unavailable: timed out".into()),
            }
        }
    }
}

async fn dashboard(State(app): State<Arc<App>>) -> Response {
    match app.mode() {
        Mode::Setup(_) => Redirect::temporary("/_/setup").into_response(),
        _ => Html(
            "<!doctype html><meta charset=utf-8><title>slonbase</title>\
             <p style=\"font-family:system-ui;margin:3rem\">slonbase is running. \
             The admin dashboard arrives in a later release.</p>",
        )
        .into_response(),
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
