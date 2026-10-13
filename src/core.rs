//! State shared by request handlers once the app is ready.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use deadpool_postgres::Pool;

use crate::collections::store::{self, Registry};
use crate::config::Config;
use crate::db::describe_pool_error;
use crate::ids;
use crate::mail::{self, Mailer, SmtpMailer};
use crate::realtime::{self, Hub};
use crate::settings::Settings;
use crate::storage::Storage;

pub struct Core {
    pub pool: Pool,
    registry: RwLock<Arc<Registry>>,
    auth_secret: RwLock<Arc<Vec<u8>>>,
    settings: RwLock<Arc<Settings>>,
    mailer: RwLock<Arc<dyn Mailer>>,
    /// Realtime subscribers connected to this instance.
    pub realtime: Hub,
    /// The directory for backups (unless they're stored in S3).
    pub backups_dir: PathBuf,
    /// Held while a backup is created or restored on this instance.
    pub backup_lock: tokio::sync::Mutex<()>,
    /// Rate limit counters of this instance.
    pub rate_limiter: crate::guard::RateLimiter,
    /// The request and app logs queue.
    pub logs: crate::logs::Logger,
    /// Whether the dashboard's SQL console may run queries.
    pub sql_console: bool,
    /// The storage directory used unless S3 is enabled in the settings.
    local_storage: PathBuf,
    storage: RwLock<Arc<Storage>>,
    /// Serializes collection changes within this instance (the database
    /// advisory lock serializes them across instances).
    pub(crate) schema_lock: tokio::sync::Mutex<()>,
}

/// The installation's signing secret, created once and shared by all instances.
async fn load_auth_secret(client: &deadpool_postgres::Client) -> Result<String> {
    client
        .execute(
            "INSERT INTO _slonbase.params (key, value) VALUES ('auth_secret', to_jsonb($1::text))
             ON CONFLICT (key) DO NOTHING",
            &[&ids::random_secret(32)],
        )
        .await?;
    Ok(client
        .query_one(
            "SELECT value #>> '{}' FROM _slonbase.params WHERE key = 'auth_secret'",
            &[],
        )
        .await?
        .get(0))
}

fn storage_for(settings: &Settings, local: &std::path::Path) -> Storage {
    if settings.s3.enabled {
        Storage::s3(settings.s3.config())
    } else {
        Storage::local(local)
    }
}

impl Core {
    /// Loads the core and starts the listener that delivers realtime events and keeps
    /// collections and settings in sync with other instances.
    pub async fn start(pool: Pool, url: &str, config: &Config) -> Result<Arc<Self>> {
        let core = Self::load(pool, config).await?;
        crate::logs::capture_app_logs(&core.logs);
        realtime::spawn_listener(&core, url.to_string(), config.database.clone());
        Ok(core)
    }

    pub async fn load(pool: Pool, config: &Config) -> Result<Arc<Self>> {
        let local_storage = config.storage.path.clone();
        let client = pool.get().await.map_err(describe_pool_error)?;

        let secret = load_auth_secret(&client).await?;

        let registry = store::load(&client).await?;
        let settings = Settings::load(&client).await?;
        drop(client);

        let logs = crate::logs::Logger::start(pool.clone());
        let storage = storage_for(&settings, &local_storage);
        tracing::info!(storage = %storage.describe(), "file storage");
        Ok(Arc::new(Self {
            pool,
            registry: RwLock::new(Arc::new(registry)),
            auth_secret: RwLock::new(Arc::new(secret.into_bytes())),
            settings: RwLock::new(Arc::new(settings)),
            mailer: RwLock::new(Arc::new(SmtpMailer)),
            realtime: Hub::default(),
            logs,
            rate_limiter: Default::default(),
            backups_dir: config.backups.path.clone(),
            backup_lock: tokio::sync::Mutex::new(()),
            sql_console: config.server.sql_console,
            local_storage,
            storage: RwLock::new(Arc::new(storage)),
            schema_lock: tokio::sync::Mutex::new(()),
        }))
    }

    pub fn registry(&self) -> Arc<Registry> {
        self.registry.read().expect("registry lock").clone()
    }

    pub(crate) fn set_registry(&self, registry: Registry) {
        *self.registry.write().expect("registry lock") = Arc::new(registry);
    }

    /// The local storage directory (used unless S3 is enabled).
    pub fn local_storage_path(&self) -> &std::path::Path {
        &self.local_storage
    }

    pub(crate) fn auth_secret(&self) -> Arc<Vec<u8>> {
        self.auth_secret.read().expect("secret lock").clone()
    }

    /// Reloads everything a restored backup replaced: the auth secret (so tokens issued
    /// before the restore stop working), collections and settings. Realtime clients are
    /// disconnected, since their auth may no longer exist.
    pub async fn reload_after_restore(&self) -> Result<()> {
        let client = self.pool.get().await.map_err(describe_pool_error)?;
        let secret = load_auth_secret(&client).await?;
        let registry = store::load(&client).await?;
        let settings = Settings::load(&client).await?;
        drop(client);
        *self.auth_secret.write().expect("secret lock") = Arc::new(secret.into_bytes());
        self.set_registry(registry);
        self.set_settings(settings);
        self.realtime.disconnect_all();
        Ok(())
    }

    pub fn settings(&self) -> Arc<Settings> {
        self.settings.read().expect("settings lock").clone()
    }

    /// Replaces the settings (switching the file storage when its settings changed).
    pub(crate) fn set_settings(&self, settings: Settings) {
        let previous = self.settings();
        let storage_changed = previous.s3 != settings.s3;
        if previous.rate_limits != settings.rate_limits {
            self.rate_limiter.reset();
        }
        if storage_changed {
            let storage = storage_for(&settings, &self.local_storage);
            tracing::info!(storage = %storage.describe(), "file storage changed");
            *self.storage.write().expect("storage lock") = Arc::new(storage);
        }
        *self.settings.write().expect("settings lock") = Arc::new(settings);
    }

    /// The file storage. Hold on to the returned value for one operation only, so a
    /// settings change takes effect for the next one.
    pub fn storage(&self) -> Arc<Storage> {
        self.storage.read().expect("storage lock").clone()
    }

    /// Replaces the mailer (tests use [`mail::MemoryMailer`]).
    pub fn set_mailer(&self, mailer: Arc<dyn Mailer>) {
        *self.mailer.write().expect("mailer lock") = mailer;
    }

    /// Sends an email and waits for the result (the settings' test email).
    pub async fn send_mail_now(&self, message: mail::Message) -> Result<()> {
        let mailer = self.mailer.read().expect("mailer lock").clone();
        let settings = self.settings();
        mailer.send(&settings, message).await
    }

    /// Sends an email in the background; failures are logged. Sending in the background
    /// also keeps response times the same whether or not an email was sent.
    pub fn send_mail(&self, message: mail::Message) {
        let mailer = self.mailer.read().expect("mailer lock").clone();
        let settings = self.settings();
        tokio::spawn(async move {
            let to = message.to.clone();
            if let Err(err) = mailer.send(&settings, message).await {
                tracing::error!(%to, error = %format!("{err:#}"), "failed to send an email");
            }
        });
    }
}
