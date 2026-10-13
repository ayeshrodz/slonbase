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
    auth_secret: Vec<u8>,
    settings: RwLock<Arc<Settings>>,
    mailer: RwLock<Arc<dyn Mailer>>,
    /// Realtime subscribers connected to this instance.
    pub realtime: Hub,
    /// The storage directory used unless S3 is enabled in the settings.
    local_storage: PathBuf,
    storage: RwLock<Arc<Storage>>,
    /// Serializes collection changes within this instance (the database
    /// advisory lock serializes them across instances).
    pub(crate) schema_lock: tokio::sync::Mutex<()>,
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
        let core = Self::load(pool, config.storage.path.clone()).await?;
        realtime::spawn_listener(&core, url.to_string(), config.database.clone());
        Ok(core)
    }

    pub async fn load(pool: Pool, local_storage: PathBuf) -> Result<Arc<Self>> {
        let client = pool.get().await.map_err(describe_pool_error)?;

        // created once per installation and shared by all instances
        client
            .execute(
                "INSERT INTO _slonbase.params (key, value) VALUES ('auth_secret', to_jsonb($1::text))
                 ON CONFLICT (key) DO NOTHING",
                &[&ids::random_secret(32)],
            )
            .await?;
        let secret: String = client
            .query_one(
                "SELECT value #>> '{}' FROM _slonbase.params WHERE key = 'auth_secret'",
                &[],
            )
            .await?
            .get(0);

        let registry = store::load(&client).await?;
        let settings = Settings::load(&client).await?;
        drop(client);

        let storage = storage_for(&settings, &local_storage);
        tracing::info!(storage = %storage.describe(), "file storage");
        Ok(Arc::new(Self {
            pool,
            registry: RwLock::new(Arc::new(registry)),
            auth_secret: secret.into_bytes(),
            settings: RwLock::new(Arc::new(settings)),
            mailer: RwLock::new(Arc::new(SmtpMailer)),
            realtime: Hub::default(),
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

    pub(crate) fn auth_secret(&self) -> &[u8] {
        &self.auth_secret
    }

    pub fn settings(&self) -> Arc<Settings> {
        self.settings.read().expect("settings lock").clone()
    }

    /// Replaces the settings (switching the file storage when its settings changed).
    pub(crate) fn set_settings(&self, settings: Settings) {
        let previous = self.settings();
        let storage_changed = previous.s3 != settings.s3;
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
