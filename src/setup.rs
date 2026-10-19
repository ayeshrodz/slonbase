//! First-run setup: connect a database, initialize it and create the first superuser.
//!
//! The web endpoints only work while the app is in setup mode and require the one-time
//! setup token printed at startup (sent as the `X-Setup-Token` header).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use deadpool_postgres::Pool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::app::{self, App, Mode, SetupMode};
use crate::config::{self, Loaded};
use crate::db::{self, ServerInfo};
use crate::error::ApiError;
use crate::ids::constant_time_eq;
use crate::superusers::{self, CreateFirst, FieldErrors};

pub const TOKEN_HEADER: &str = "x-setup-token";

const SETUP_PAGE: &str = include_str!("../assets/setup.html");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRequest {
    #[serde(default)]
    pub database_url: Option<String>,
    pub email: String,
    pub password: String,
}

#[derive(Debug)]
pub enum SetupError {
    Validation(FieldErrors),
    BadRequest(String),
    Internal(anyhow::Error),
}

impl From<SetupError> for ApiError {
    fn from(err: SetupError) -> Self {
        match err {
            SetupError::Validation(errors) => ApiError::validation(errors),
            SetupError::BadRequest(message) => ApiError::bad_request(message),
            SetupError::Internal(err) => {
                tracing::error!(error = %format!("{err:#}"), "setup failed");
                // only the person holding the setup token sees this, and they need the details
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Setup failed: {err:#}"),
                )
            }
        }
    }
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetupError::Validation(errors) => {
                let parts: Vec<String> = errors.iter().map(|(k, v)| format!("{k}: {v}")).collect();
                write!(f, "{}", parts.join("; "))
            }
            SetupError::BadRequest(message) => write!(f, "{message}"),
            SetupError::Internal(err) => write!(f, "{err:#}"),
        }
    }
}

/// Runs the whole setup. `pool` is the already configured database, if any.
/// Returns the pool and the config to persist (when the URL came from the request).
pub async fn perform(
    pool: Option<Pool>,
    loaded: &Loaded,
    req: SetupRequest,
) -> Result<(Pool, Option<Loaded>), SetupError> {
    let mut errors = superusers::validate(&req.email, &req.password);
    let request_url = req
        .database_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty());

    let (pool, new_url) = match (pool, request_url) {
        (Some(pool), None) => (pool, None),
        (Some(_), Some(_)) => {
            return Err(SetupError::BadRequest(
                "The database is already configured; leave the database URL empty.".into(),
            ));
        }
        (None, Some(url)) => {
            if !errors.is_empty() {
                return Err(SetupError::Validation(errors));
            }
            let pool = db::connect(url, &loaded.effective.database)
                .map_err(|e| SetupError::BadRequest(format!("{e:#}")))?;
            (pool, Some(url.to_string()))
        }
        (None, None) => {
            errors.insert("databaseUrl", "A database URL is required.".into());
            return Err(SetupError::Validation(errors));
        }
    };
    if !errors.is_empty() {
        return Err(SetupError::Validation(errors));
    }

    app::prepare(&pool)
        .await
        .map_err(|e| SetupError::BadRequest(format!("{e:#}")))?;

    match superusers::create_first(&pool, &req.email, &req.password)
        .await
        .map_err(SetupError::Internal)?
    {
        CreateFirst::Created => tracing::info!("created the first superuser"),
        CreateFirst::AlreadyExists => {
            // reconnecting to an existing installation: prove you own it
            if !superusers::verify(&pool, &req.email, &req.password)
                .await
                .map_err(SetupError::Internal)?
            {
                return Err(SetupError::BadRequest(
                    "This database already contains a slonbase installation. \
                     Sign in with one of its superusers to reconnect to it."
                        .into(),
                ));
            }
            tracing::info!("reconnected to an existing installation");
        }
    }

    let new_config = match new_url {
        Some(url) => {
            let mut updated = loaded.clone();
            updated.file.database.url = Some(url.clone());
            updated.effective.database.url = Some(url);
            config::save(&updated.path, &updated.file).map_err(SetupError::Internal)?;
            tracing::info!(path = %updated.path.display(), "saved the database configuration");
            Some(updated)
        }
        None => None,
    };

    Ok((pool, new_config))
}

/// Returns the current setup state if the request carries a valid token.
fn authorize(app: &App, headers: &HeaderMap) -> Result<SetupMode, ApiError> {
    let Mode::Setup(setup) = app.mode() else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "Setup has already been completed.",
        ));
    };
    let provided = headers
        .get(TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !constant_time_eq(provided, &setup.token) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Missing or invalid setup token. Use the setup link printed in the server logs.",
        ));
    }
    Ok(setup)
}

pub async fn page() -> impl IntoResponse {
    (
        [
            ("cache-control", "no-store"),
            ("referrer-policy", "no-referrer"),
            ("x-frame-options", "DENY"),
        ],
        Html(SETUP_PAGE),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    needs_database: bool,
}

pub async fn status(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let setup = authorize(&app, &headers)?;
    Ok(Json(StatusResponse {
        needs_database: setup.pool.is_none(),
    }))
}

#[derive(Deserialize)]
pub struct TestDatabaseRequest {
    url: String,
}

pub async fn test_database(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<TestDatabaseRequest>,
) -> Result<Json<ServerInfo>, ApiError> {
    let setup = authorize(&app, &headers)?;
    if setup.pool.is_some() {
        return Err(ApiError::bad_request("The database is already configured."));
    }

    let pool = db::connect(req.url.trim(), &app.config().effective.database)
        .map_err(|e| ApiError::bad_request(format!("{e:#}")))?;
    let result = db::check_server(&pool).await;
    pool.close();

    result
        .map(Json)
        .map_err(|e| ApiError::bad_request(format!("{e:#}")))
}

pub async fn complete(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<SetupRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let _guard = app.setup_lock.lock().await;
    // checked after taking the lock: a concurrent request may have just finished
    let setup = authorize(&app, &headers)?;

    let (pool, new_config) = perform(setup.pool, &app.config(), req).await?;
    if let Some(loaded) = new_config {
        app.set_config(loaded);
    }
    let config = app.config();
    let url = config.effective.database.url.clone().unwrap_or_default();
    let core = crate::core::Core::start(pool, &url, &config.effective, app.hooks.clone())
        .await
        .map_err(ApiError::internal)?;
    app.ready(core);
    tracing::info!("setup completed");

    Ok(Json(json!({ "message": "Setup completed." })))
}
