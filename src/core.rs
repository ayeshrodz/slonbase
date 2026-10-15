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
    /// The collections, with a generation that every replacement bumps.
    registry: RwLock<(u64, Arc<Registry>)>,
    auth_secret: RwLock<Arc<Vec<u8>>>,
    settings: RwLock<Arc<Settings>>,
    /// Bumped by every settings replacement (held while replacing them).
    settings_generation: std::sync::Mutex<u64>,
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
    /// The registered hooks and cron jobs.
    pub hooks: Arc<crate::hooks::Hooks>,
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
    pub async fn start(
        pool: Pool,
        url: &str,
        config: &Config,
        hooks: Arc<crate::hooks::Hooks>,
    ) -> Result<Arc<Self>> {
        let core = Self::load(pool, config, hooks).await?;
        crate::logs::capture_app_logs(&core.logs);
        realtime::spawn_listener(&core, url.to_string(), config.database.clone());
        core.hooks.ready(&core).await;
        Ok(core)
    }

    pub async fn load(
        pool: Pool,
        config: &Config,
        hooks: Arc<crate::hooks::Hooks>,
    ) -> Result<Arc<Self>> {
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
            registry: RwLock::new((0, Arc::new(registry))),
            auth_secret: RwLock::new(Arc::new(secret.into_bytes())),
            settings: RwLock::new(Arc::new(settings)),
            settings_generation: std::sync::Mutex::new(0),
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
            hooks,
        }))
    }

    pub fn registry(&self) -> Arc<Registry> {
        self.registry.read().expect("registry lock").1.clone()
    }

    pub(crate) fn set_registry(&self, registry: Registry) {
        let mut current = self.registry.write().expect("registry lock");
        *current = (current.0 + 1, Arc::new(registry));
    }

    /// The registry's generation, to pass to [`Core::set_registry_if`].
    pub(crate) fn registry_generation(&self) -> u64 {
        self.registry.read().expect("registry lock").0
    }

    /// Replaces the registry unless it was replaced since `generation`: a reload that read
    /// the collections before a newer local change committed must not undo it.
    pub(crate) fn set_registry_if(&self, generation: u64, registry: Registry) -> bool {
        let mut current = self.registry.write().expect("registry lock");
        if current.0 != generation {
            return false;
        }
        *current = (generation + 1, Arc::new(registry));
        true
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
        let mut generation = self
            .settings_generation
            .lock()
            .expect("settings generation");
        self.replace_settings(settings);
        *generation += 1;
    }

    /// The settings' generation, to pass to [`Core::set_settings_if`].
    pub(crate) fn settings_generation(&self) -> u64 {
        *self
            .settings_generation
            .lock()
            .expect("settings generation")
    }

    /// Replaces the settings unless they were replaced since `generation` (see
    /// [`Core::set_registry_if`]).
    pub(crate) fn set_settings_if(&self, generation: u64, settings: Settings) -> bool {
        let mut current = self
            .settings_generation
            .lock()
            .expect("settings generation");
        if *current != generation {
            return false;
        }
        self.replace_settings(settings);
        *current += 1;
        true
    }

    fn replace_settings(&self, settings: Settings) {
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
        let event = crate::hooks::MailerEvent {
            message,
            kind: String::new(),
            collection: None,
            record: None,
        };
        let sent = Arc::new(std::sync::Mutex::new(None));
        let result = self
            .hooks
            .on_mailer_send
            .trigger(None, event, {
                let sent = sent.clone();
                move |e| async move {
                    let result = mailer.send(&settings, e.message.clone()).await;
                    *sent.lock().expect("sent lock") = Some(result);
                    Ok(e)
                }
            })
            .await;
        result.map_err(|e| anyhow::anyhow!(e.message))?;
        // a handler that skipped `next` didn't send it
        sent.lock().expect("sent lock").take().unwrap_or(Ok(()))
    }

    /// Sends an email in the background; failures are logged. Sending in the background
    /// also keeps response times the same whether or not an email was sent.
    pub fn send_mail(&self, message: mail::Message) {
        self.send_mail_for(message, "", None);
    }

    /// [`Core::send_mail`] for an auth email: `kind` and the record reach the mail hooks.
    pub(crate) fn send_mail_for(
        &self,
        message: mail::Message,
        kind: &str,
        record: Option<&crate::auth::AuthRecord>,
    ) {
        let mailer = self.mailer.read().expect("mailer lock").clone();
        let settings = self.settings();
        let hooks = self.hooks.clone();
        let collection = record.map(|r| r.collection.clone());
        let event = crate::hooks::MailerEvent {
            message,
            kind: kind.to_string(),
            collection: collection.clone(),
            record: record.map(|r| {
                let mut values = r.values.clone();
                values.remove(crate::collections::model::FIELD_PASSWORD);
                values.remove(crate::collections::model::FIELD_TOKEN_KEY);
                values
            }),
        };
        tokio::spawn(async move {
            let to = event.message.to.clone();
            let result = hooks
                .on_mailer_send
                .trigger(collection.as_deref(), event, move |e| async move {
                    if let Err(err) = mailer.send(&settings, e.message.clone()).await {
                        let to = &e.message.to;
                        tracing::error!(%to, error = %format!("{err:#}"), "failed to send an email");
                    }
                    Ok(e)
                })
                .await;
            if let Err(err) = result {
                tracing::error!(%to, error = %err.message, "a mail hook failed; the email wasn't sent");
            }
        });
    }
}
