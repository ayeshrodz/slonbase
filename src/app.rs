//! Application state and the startup state machine.
//!
//! ```text
//! Starting ──(no database configured, or no superuser yet)──▶ Setup ──(setup completed)──▶ Ready
//!     └──────────────────────(installed)──────────────────────────────────────────────────▲
//! ```
//!
//! Once `Ready`, the app never returns to `Setup`: a database outage only makes
//! requests fail with 503 while the pool keeps reconnecting.

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::Result;
use deadpool_postgres::Pool;

use crate::config::Loaded;
use crate::core::Core;
use crate::{db, ids, migrations, superusers};

const MIN_SETUP_TOKEN_LENGTH: usize = 32;

#[derive(Clone)]
pub enum Mode {
    Starting { status: String },
    Setup(SetupMode),
    Ready { core: Arc<Core> },
}

#[derive(Clone)]
pub struct SetupMode {
    pub token: Arc<str>,
    /// Set when the database is already configured (config file or env)
    /// and only the first superuser is missing.
    pub pool: Option<Pool>,
}

pub struct App {
    mode: RwLock<Mode>,
    config: Mutex<Loaded>,
    setup_token: Arc<str>,
    /// Serializes setup completion requests.
    pub(crate) setup_lock: tokio::sync::Mutex<()>,
    pub(crate) hooks: Arc<crate::hooks::Hooks>,
    /// Extra routes, merged into the router.
    routes: axum::Router<Arc<App>>,
}

impl App {
    /// Creates the app in the `Starting` mode. `setup_token` lets automation
    /// preset the token; otherwise a random one is generated.
    pub fn new(config: Loaded, setup_token: Option<String>) -> Result<Arc<Self>> {
        Self::with_extensions(config, setup_token, crate::Extensions::default())
    }

    /// Creates the app with hooks, routes and cron jobs.
    pub fn with_extensions(
        config: Loaded,
        setup_token: Option<String>,
        extensions: crate::Extensions,
    ) -> Result<Arc<Self>> {
        let token = match setup_token {
            Some(t) if t.len() < MIN_SETUP_TOKEN_LENGTH => {
                anyhow::bail!(
                    "the setup token must be at least {MIN_SETUP_TOKEN_LENGTH} characters"
                )
            }
            Some(t) => t,
            None => ids::random_secret(32),
        };

        Ok(Arc::new(Self {
            mode: RwLock::new(Mode::Starting {
                status: "starting".into(),
            }),
            config: Mutex::new(config),
            setup_token: token.into(),
            setup_lock: tokio::sync::Mutex::new(()),
            hooks: Arc::new(extensions.hooks),
            routes: extensions.routes,
        }))
    }

    /// The ready state, or a 503 while starting or in setup.
    pub fn core(&self) -> Result<Arc<Core>, crate::error::ApiError> {
        match self.mode() {
            Mode::Ready { core } => Ok(core),
            Mode::Setup(_) => Err(crate::error::ApiError::unavailable(
                "slonbase hasn't been set up yet.",
            )),
            Mode::Starting { .. } => Err(crate::error::ApiError::unavailable(
                "slonbase is starting, try again shortly.",
            )),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode.read().expect("mode lock").clone()
    }

    pub(crate) fn set_mode(&self, mode: Mode) {
        *self.mode.write().expect("mode lock") = mode;
    }

    pub fn config(&self) -> Loaded {
        self.config.lock().expect("config lock").clone()
    }

    pub(crate) fn set_config(&self, loaded: Loaded) {
        *self.config.lock().expect("config lock") = loaded;
    }

    pub(crate) fn extra_routes(&self) -> axum::Router<Arc<App>> {
        self.routes.clone()
    }

    /// The registered hooks.
    pub fn hooks(&self) -> &crate::hooks::Hooks {
        &self.hooks
    }

    pub fn setup_token(&self) -> &str {
        &self.setup_token
    }

    /// Brings the app to `Setup` or `Ready`. Transient database errors are retried
    /// with backoff (useful when the database starts after the app); only errors
    /// that a retry can't fix are returned.
    pub async fn bootstrap(&self) -> Result<()> {
        let config = self.config();
        let Some(url) = config.effective.database.url.clone() else {
            self.enter_setup(None);
            return Ok(());
        };

        // fails only on configuration errors (bad URL, unreadable CA file)
        let pool = db::connect(&url, &config.effective.database)?;

        let mut delay = Duration::from_secs(1);
        loop {
            let attempt = async {
                if prepare(&pool).await? {
                    Ok(Some(
                        Core::start(pool.clone(), &url, &config.effective, self.hooks.clone())
                            .await?,
                    ))
                } else {
                    anyhow::Ok(None)
                }
            };
            match attempt.await {
                Ok(Some(core)) => {
                    tracing::info!("database ready");
                    self.set_mode(Mode::Ready { core });
                    return Ok(());
                }
                Ok(None) => {
                    self.enter_setup(Some(pool));
                    return Ok(());
                }
                Err(err) => {
                    let status = format!("{err:#}");
                    tracing::warn!(error = %status, retry_in = ?delay, "database not ready");
                    self.set_mode(Mode::Starting { status });
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    fn enter_setup(&self, pool: Option<Pool>) {
        self.set_mode(Mode::Setup(SetupMode {
            token: self.setup_token.clone(),
            pool,
        }));
    }
}

/// Checks the server, applies migrations and reports whether a superuser exists.
pub(crate) async fn prepare(pool: &Pool) -> Result<bool> {
    db::check_server(pool).await?;
    let applied = migrations::run(pool).await?;
    if !applied.is_empty() {
        tracing::info!(?applied, "applied system migrations");
    }
    crate::collections::system::ensure(pool).await?;
    crate::collections::store::heal_views(pool).await?;
    Ok(superusers::count(pool).await? > 0)
}
