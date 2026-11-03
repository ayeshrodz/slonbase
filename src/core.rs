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
    /// List totals kept between requests (see [`crate::count_cache`]).
    pub(crate) counts: crate::count_cache::CountCache,
    /// Encrypts the settings and the signing secret in the database (`server.encryption_key`).
    secrets_key: Option<Arc<crate::secrets::SecretsKey>>,
    /// Encrypts new backups (`backups.encryption_key`).
    pub(crate) backup_key: Option<String>,
    /// Held while a backup is created or restored on this instance.
    pub backup_lock: tokio::sync::Mutex<()>,
    /// Rate limit counters of this instance.
    pub rate_limiter: crate::guard::RateLimiter,
    pub mail_throttle: crate::guard::MailThrottle,
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
    /// Where automigrate writes migration files (when it's on).
    automigrate_dir: Option<std::path::PathBuf>,
    /// The registered hooks and cron jobs.
    pub hooks: Arc<crate::hooks::Hooks>,
}

/// The installation's signing secret, created once and shared by all instances (encrypted
/// with `key` if there's one).
async fn load_auth_secret(
    client: &deadpool_postgres::Client,
    key: Option<&crate::secrets::SecretsKey>,
) -> Result<String> {
    let new = serde_json::Value::String(ids::random_secret(32));
    client
        .execute(
            "INSERT INTO _slonbase.params (key, value) VALUES ('auth_secret', $1)
             ON CONFLICT (key) DO NOTHING",
            &[&crate::secrets::to_stored(&new, key)],
        )
        .await?;
    let stored = client
        .query_one(
            "SELECT value FROM _slonbase.params WHERE key = 'auth_secret'",
            &[],
        )
        .await?
        .get(0);
    match crate::secrets::from_stored(stored, key, "token signing secret")? {
        serde_json::Value::String(secret) => Ok(secret),
        _ => anyhow::bail!("the token signing secret isn't text"),
    }
}

/// The encryption key of the configuration, if there's one.
fn secrets_key(config: &Config) -> Result<Option<Arc<crate::secrets::SecretsKey>>> {
    config
        .server
        .encryption_key
        .as_deref()
        .map(|key| crate::secrets::SecretsKey::new(key).map(Arc::new))
        .transpose()
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
        Self::start_with(pool, url, config, hooks, true).await
    }

    /// Like [`Core::start`]; `app_logs` makes this core's logs receive the process's
    /// warnings and errors (the main installation's, not a project's).
    pub(crate) async fn start_with(
        pool: Pool,
        url: &str,
        config: &Config,
        hooks: Arc<crate::hooks::Hooks>,
        app_logs: bool,
    ) -> Result<Arc<Self>> {
        let core = Self::load(pool, config, hooks).await?;
        if app_logs {
            crate::logs::capture_app_logs(&core.logs);
        }
        realtime::spawn_listener(&core, url.to_string(), config.database.clone());
        core.hooks.ready(&core).await;
        crate::settings::reloaded(&core);
        Ok(core)
    }

    pub async fn load(
        pool: Pool,
        config: &Config,
        hooks: Arc<crate::hooks::Hooks>,
    ) -> Result<Arc<Self>> {
        let local_storage = config.storage.path.clone();
        let client = pool.get().await.map_err(describe_pool_error)?;
        let secrets_key = secrets_key(config)?;
        if let Some(key) = &secrets_key {
            let sealed = crate::secrets::seal_plain(&client, key).await?
                + crate::secrets::seal_plain_rows(&client, key).await?;
            if sealed > 0 {
                tracing::info!(count = sealed, "encrypted stored secrets");
            }
        }

        let secret = load_auth_secret(&client, secrets_key.as_deref()).await?;

        let registry = store::load_with(&client, secrets_key.as_deref()).await?;
        let settings = Settings::load(&client, secrets_key.as_deref()).await?;
        drop(client);

        let logs = crate::logs::Logger::start(pool.clone(), settings.logs.max_data_size);
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
            mail_throttle: Default::default(),
            backups_dir: config.backups.path.clone(),
            backup_key: config.backups.encryption_key.clone(),
            secrets_key,
            counts: Default::default(),
            backup_lock: tokio::sync::Mutex::new(()),
            sql_console: config.server.sql_console,
            local_storage,
            storage: RwLock::new(Arc::new(storage)),
            schema_lock: tokio::sync::Mutex::new(()),
            automigrate_dir: config
                .migrations
                .automigrate
                .then(|| config.migrations.dir.clone()),
            hooks,
        }))
    }

    /// Where automigrate writes migration files, or `None` when it's off.
    pub fn automigrate_dir(&self) -> Option<&std::path::Path> {
        self.automigrate_dir.as_deref()
    }

    pub fn registry(&self) -> Arc<Registry> {
        self.registry.read().expect("registry lock").1.clone()
    }

    pub(crate) fn set_registry(&self, registry: Registry) {
        // a changed collection (a dropped field, a new rule) can change any count
        self.counts.changed();
        // and the prepared statements' tables may have changed
        self.pool.manager().statement_caches.clear();
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
        // a backup made without the key comes back unencrypted
        if let Some(key) = self.secrets_key() {
            crate::secrets::seal_plain(&client, key).await?;
            crate::secrets::seal_plain_rows(&client, key).await?;
        }
        let secret = load_auth_secret(&client, self.secrets_key()).await?;
        let registry = store::load_with(&client, self.secrets_key()).await?;
        let settings = Settings::load(&client, self.secrets_key()).await?;
        drop(client);
        *self.auth_secret.write().expect("secret lock") = Arc::new(secret.into_bytes());
        self.set_registry(registry);
        self.set_settings(settings);
        self.counts.changed();
        self.realtime.disconnect_all();
        Ok(())
    }

    /// Records a change of data that the realtime triggers don't report (a `TRUNCATE`, say),
    /// here and on the other instances, so no stale list total is served.
    pub(crate) async fn data_changed_everywhere(
        &self,
        client: &impl deadpool_postgres::GenericClient,
    ) {
        crate::realtime::written(self);
        if let Err(err) = crate::realtime::notify(client, "changed").await {
            tracing::debug!(error = %err, "failed to tell the other instances about a data change");
        }
    }

    /// [`Core::secrets_key`], to keep (in a task, say).
    pub(crate) fn secrets_key_owned(&self) -> Option<Arc<crate::secrets::SecretsKey>> {
        self.secrets_key.clone()
    }

    /// The key the settings and the signing secret are encrypted with, if any.
    pub(crate) fn secrets_key(&self) -> Option<&crate::secrets::SecretsKey> {
        self.secrets_key.as_deref()
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
        self.logs.set_max_data_size(settings.logs.max_data_size);
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
