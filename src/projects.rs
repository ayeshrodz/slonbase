//! Projects: further installations hosted by the main one, each in its own PostgreSQL
//! database on the same server, with its own collections, superusers, settings, files and
//! hooks. A project is served under `/p/{name}/` by an [`App`] of its own, so everything
//! that works for the main installation works for a project unchanged.
//!
//! The list of projects lives in the main database (`_slonbase.projects`); changes are
//! announced on [`CHANNEL`], so every instance starts and stops the same projects.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, Request, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::app::{App, Mode};
use crate::auth::RequestAuth;
use crate::config::Loaded;
use crate::core::Core;
use crate::error::ApiError;
use crate::filter::compile::quote_ident;
use crate::superusers::FieldErrors;

/// The path prefix of projects.
pub const PATH_PREFIX: &str = "/p/";
/// The notification channel announcing changes to the list of projects.
pub const CHANNEL: &str = "slonbase_projects";
const MAX_NAME_LENGTH: usize = 32;

/// A running project.
pub struct Project {
    pub name: String,
    /// The PostgreSQL database holding the project.
    pub database: String,
    pub hosts: Vec<String>,
    pub created: String,
    /// The project's own PostgreSQL role, with its password.
    role: Option<(String, String)>,
    pub app: Arc<App>,
    router: axum::Router,
}

impl Project {
    /// `ready`, `starting` (with the reason, e.g. the database is unreachable) or `setup`.
    pub fn status(&self) -> (&'static str, Option<String>) {
        match self.app.mode() {
            Mode::Ready { .. } => ("ready", None),
            Mode::Starting { status } => ("starting", Some(status)),
            Mode::Setup(_) => ("setup", None),
        }
    }

    fn to_json(&self) -> Value {
        let (status, detail) = self.status();
        json!({
            "name": self.name,
            "database": self.database,
            "hosts": self.hosts,
            "created": self.created,
            "path": format!("{PATH_PREFIX}{}/", self.name),
            "role": self.role.as_ref().map(|(role, _)| role.clone()),
            "status": status,
            "statusDetail": detail.unwrap_or_default(),
        })
    }
}

/// The projects of the main installation.
pub struct Projects {
    root: Weak<App>,
    projects: RwLock<HashMap<String, Arc<Project>>>,
    /// Set once the main installation is ready and the projects are loaded.
    active: AtomicBool,
    /// Serializes changes to the list within this instance.
    lock: tokio::sync::Mutex<()>,
}

/// A row of `_slonbase.projects`.
#[derive(Clone)]
struct Row {
    name: String,
    database: String,
    hosts: Vec<String>,
    created: String,
    role: Option<(String, String)>,
}

impl Project {
    #[cfg(feature = "js")]
    fn row(&self) -> Row {
        Row {
            name: self.name.clone(),
            database: self.database.clone(),
            hosts: self.hosts.clone(),
            created: self.created.clone(),
            role: self.role.clone(),
        }
    }
}

/// How a request reached a project (set on the requests a project serves).
#[derive(Clone, Debug)]
pub struct Routed {
    /// The path the project is served under: `/p/{name}`, or empty for its own host name.
    pub base: String,
    /// The main installation's dashboard: a path on the same host, or an absolute URL when
    /// the project is served on a host name of its own.
    pub main_dashboard: String,
}

impl Projects {
    pub(crate) fn new(root: Weak<App>) -> Self {
        Self {
            root,
            projects: RwLock::new(HashMap::new()),
            active: AtomicBool::new(false),
            lock: tokio::sync::Mutex::new(()),
        }
    }

    /// The running project with this name.
    pub fn get(&self, name: &str) -> Option<Arc<Project>> {
        self.projects
            .read()
            .expect("projects lock")
            .get(name)
            .cloned()
    }

    /// The running project served on this host name (without a port).
    pub fn by_host(&self, host: &str) -> Option<Arc<Project>> {
        self.projects
            .read()
            .expect("projects lock")
            .values()
            .find(|p| p.hosts.iter().any(|h| h == host))
            .cloned()
    }

    /// The running projects, by name.
    pub fn list(&self) -> Vec<Arc<Project>> {
        let mut list: Vec<_> = self
            .projects
            .read()
            .expect("projects lock")
            .values()
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// Loads and starts the projects, and follows changes made by other instances.
    /// Called when the main installation becomes ready (only the first call does anything).
    pub(crate) fn activate(&self) {
        if self.active.swap(true, Ordering::SeqCst) {
            return;
        }
        let root = self.root.clone();
        tokio::spawn(async move {
            let mut delay = Duration::from_secs(1);
            loop {
                let Some(app) = root.upgrade() else { return };
                let config = app.config().effective;
                let Some(url) = config.database.url.clone() else {
                    return;
                };
                drop(app);
                // listen first, so no change is missed between the load and the listen
                let listener = crate::db::listen(&url, &config.database, CHANNEL).await;
                if let Some(app) = root.upgrade() {
                    if let Err(err) = app.projects.reload().await {
                        tracing::error!(error = %format!("{err:#}"), "failed to load the projects");
                    }
                }
                let (_client, mut notifications) = match listener {
                    Ok(listener) => listener,
                    Err(err) => {
                        tracing::warn!(error = %format!("{err:#}"), retry_in = ?delay, "projects listener failed to connect");
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(Duration::from_secs(30));
                        continue;
                    }
                };
                delay = Duration::from_secs(1);
                while notifications.recv().await.is_some() {
                    let Some(app) = root.upgrade() else { return };
                    if let Err(err) = app.projects.reload().await {
                        tracing::error!(error = %format!("{err:#}"), "failed to reload the projects");
                    }
                }
                tracing::warn!("projects listener disconnected, reconnecting");
            }
        });
    }

    /// Starts the projects added and stops those removed since the last load.
    async fn reload(&self) -> anyhow::Result<()> {
        let _guard = self.lock.lock().await;
        let root = self
            .root
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("stopped"))?;
        let core = root.core().map_err(|e| anyhow::anyhow!(e.message))?;
        let rows = load_rows(&core).await?;

        let current: Vec<String> = self
            .projects
            .read()
            .expect("projects lock")
            .keys()
            .cloned()
            .collect();
        for name in current {
            if !rows.iter().any(|r| r.name == name) {
                self.stop(&name);
            }
        }
        for row in rows {
            let existing = self.get(&row.name);
            match existing {
                Some(project) if project.database == row.database => {
                    if project.hosts != row.hosts {
                        let updated = Project {
                            name: project.name.clone(),
                            database: project.database.clone(),
                            hosts: row.hosts,
                            created: project.created.clone(),
                            role: project.role.clone(),
                            app: project.app.clone(),
                            router: project.router.clone(),
                        };
                        self.insert(updated);
                    }
                }
                _ => {
                    let project = start(&root, row).await?;
                    let app = project.app.clone();
                    tokio::spawn(async move {
                        if let Err(err) = app.bootstrap().await {
                            tracing::error!(project = app.project().unwrap_or_default(), error = %format!("{err:#}"), "failed to start a project");
                        }
                    });
                    self.insert(project);
                }
            }
        }
        Ok(())
    }

    /// Restarts a project on this instance (its hooks changed): the new app takes over once
    /// it's ready, and requests in flight finish on the old one.
    #[cfg(feature = "js")]
    async fn restart(&self, name: &str) -> anyhow::Result<()> {
        let _guard = self.lock.lock().await;
        let root = self
            .root
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("stopped"))?;
        let Some(old) = self.get(name) else {
            return Ok(());
        };
        let project = start(&root, old.row()).await?;
        if !matches!(project.app.mode(), Mode::Starting { ref status } if status.contains("hooks"))
        {
            let bootstrap = project.app.bootstrap();
            if tokio::time::timeout(Duration::from_secs(30), bootstrap)
                .await
                .is_err()
            {
                tracing::warn!(project = %name, "the restarted project isn't ready yet");
            }
        }
        self.insert(project);
        if let Mode::Ready { core } = old.app.mode() {
            // realtime clients reconnect, to the new app
            core.realtime.disconnect_all();
        }
        tracing::info!(project = %name, "restarted project (its hooks changed)");
        Ok(())
    }

    fn insert(&self, project: Project) -> Arc<Project> {
        let project = Arc::new(project);
        self.projects
            .write()
            .expect("projects lock")
            .insert(project.name.clone(), project.clone());
        project
    }

    /// Stops a project on this instance: its requests answer 503 and its background
    /// work ends once the last request holding its core finishes.
    fn stop(&self, name: &str) {
        let removed = self.projects.write().expect("projects lock").remove(name);
        if let Some(project) = removed {
            if let Mode::Ready { core } = project.app.mode() {
                core.realtime.disconnect_all();
                core.pool.close();
            }
            project.app.set_mode(Mode::Starting {
                status: "the project was deleted".into(),
            });
            tracing::info!(project = %name, "stopped project");
        }
    }

    /// Creates a project: a new database with slonbase's system tables, its first
    /// superuser, and the app serving it.
    /// With `own_role`, the project gets a PostgreSQL role of its own (named like its
    /// database), which owns the database and is the only other role that can connect to it.
    pub async fn create(
        &self,
        name: &str,
        email: &str,
        password: &str,
        own_role: bool,
    ) -> Result<Arc<Project>, ApiError> {
        let mut errors = FieldErrors::new();
        if let Err(message) = validate_name(name) {
            errors.insert("name", message.to_string());
        }
        for (field, message) in crate::superusers::validate(email, password) {
            let field = match field {
                "email" => "superuserEmail",
                "password" => "superuserPassword",
                other => other,
            };
            errors.insert(field, message);
        }
        if !errors.is_empty() {
            return Err(ApiError::validation(errors));
        }

        let _guard = self.lock.lock().await;
        let root = self
            .root
            .upgrade()
            .ok_or_else(|| ApiError::unavailable("slonbase is stopping."))?;
        let core = root.core()?;
        let client = core
            .pool
            .get()
            .await
            .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
        let taken = client
            .query_opt("SELECT 1 FROM _slonbase.projects WHERE name = $1", &[&name])
            .await
            .map_err(|e| ApiError::internal(e.into()))?;
        if taken.is_some() {
            return Err(name_error("A project with this name already exists."));
        }
        let main_database: String = client
            .query_one("SELECT current_database()", &[])
            .await
            .map_err(|e| ApiError::internal(e.into()))?
            .get(0);
        let database = database_name(&main_database, name)
            .ok_or_else(|| name_error("The name is too long for this server's database names."))?;
        let exists = client
            .query_opt("SELECT 1 FROM pg_database WHERE datname = $1", &[&database])
            .await
            .map_err(|e| ApiError::internal(e.into()))?;
        if exists.is_some() {
            return Err(name_error(format!(
                "A database named {database} already exists on the server."
            )));
        }
        let role = if own_role {
            let taken = client
                .query_opt("SELECT 1 FROM pg_roles WHERE rolname = $1", &[&database])
                .await
                .map_err(|e| ApiError::internal(e.into()))?;
            if taken.is_some() {
                return Err(name_error(format!(
                    "A role named {database} already exists on the server."
                )));
            }
            let password = crate::ids::random_secret(32);
            client
                .batch_execute(&format!(
                    "CREATE ROLE {role} LOGIN PASSWORD {password};
                     GRANT {role} TO CURRENT_USER;",
                    role = quote_ident(&database),
                    password = quote_literal(&password),
                ))
                .await
                .map_err(|err| {
                    if err.code() == Some(&tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE)
                    {
                        ApiError::bad_request(
                            "The database role can't create roles. Grant it the CREATEROLE \
                             privilege (ALTER ROLE ... CREATEROLE), or create the project \
                             without a role of its own.",
                        )
                    } else {
                        ApiError::internal(
                            anyhow::anyhow!(err).context("failed to create the role"),
                        )
                    }
                })?;
            Some((database.clone(), password))
        } else {
            None
        };
        let owner = role
            .as_ref()
            .map(|(role, _)| format!(" OWNER {}", quote_ident(role)))
            .unwrap_or_default();
        let created_db = client
            .batch_execute(&format!(
                "CREATE DATABASE {}{owner}",
                quote_ident(&database)
            ))
            .await;
        if let Err(err) = created_db {
            if let Some((role, _)) = &role {
                let _ = client
                    .batch_execute(&format!("DROP ROLE IF EXISTS {}", quote_ident(role)))
                    .await;
            }
            return Err(
                if err.code() == Some(&tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE) {
                    ApiError::bad_request(
                        "The database role can't create databases. Grant it the CREATEDB \
                         privilege (ALTER ROLE ... CREATEDB) to create projects.",
                    )
                } else {
                    ApiError::internal(
                        anyhow::anyhow!(err).context("failed to create the database"),
                    )
                },
            );
        }
        if role.is_some() {
            // only the owner (and the main installation, a member of it) may connect
            if let Err(err) = client
                .batch_execute(&format!(
                    "REVOKE ALL ON DATABASE {} FROM PUBLIC",
                    quote_ident(&database)
                ))
                .await
            {
                tracing::warn!(%database, error = %err, "failed to revoke public access to the project's database");
            }
        }
        tracing::info!(project = %name, %database, "created the project's database");

        let created = async {
            let row = Row {
                name: name.to_string(),
                database: database.clone(),
                hosts: Vec::new(),
                created: String::new(),
                role: role.clone(),
            };
            let project = start(&root, row).await?;
            let pool = crate::db::connect(
                project
                    .app
                    .config()
                    .effective
                    .database
                    .url
                    .as_deref()
                    .unwrap_or_default(),
                &project.app.config().effective.database,
            )?;
            let prepared = crate::app::prepare(&pool).await;
            let first = match prepared {
                Ok(_) => crate::superusers::create_first(&pool, email, password).await,
                Err(err) => Err(err),
            };
            pool.close();
            first?;
            project.app.bootstrap().await?;
            let project_core = project.app.core().map_err(|e| anyhow::anyhow!(e.message))?;
            // email links and the dashboard's title point at the project
            let main_settings = core.settings();
            let app_url = format!(
                "{}{PATH_PREFIX}{name}",
                main_settings.meta.app_url.trim_end_matches('/')
            );
            let patch = json!({ "meta": { "appName": name, "appURL": app_url } });
            crate::settings::apply_patch(
                &project_core,
                patch.as_object().cloned().unwrap_or_default(),
            )
            .await
            .map_err(|e| anyhow::anyhow!(e.message))?;
            let created: String = client
                .query_one(
                    "INSERT INTO _slonbase.projects (name, database, role, role_password)
                     VALUES ($1, $2, $3, $4)
                     RETURNING to_char(created AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.MS\"Z\"')",
                    &[
                        &name,
                        &database,
                        &role.as_ref().map(|(r, _)| r.clone()),
                        &role.as_ref().map(|(_, p)| p.clone()),
                    ],
                )
                .await?
                .get(0);
            client
                .execute("SELECT pg_notify($1, $2)", &[&CHANNEL, &name])
                .await?;
            anyhow::Ok(Project { created, ..project })
        };
        match created.await {
            Ok(project) => {
                tracing::info!(project = %name, "created project");
                Ok(self.insert(project))
            }
            Err(err) => {
                // leave nothing behind
                if let Err(drop_err) = client
                    .batch_execute(&format!(
                        "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                        quote_ident(&database)
                    ))
                    .await
                {
                    tracing::error!(%database, error = %drop_err, "failed to drop the database of a project that couldn't be created");
                }
                if let Some((role, _)) = &role {
                    let _ = client
                        .batch_execute(&format!("DROP ROLE IF EXISTS {}", quote_ident(role)))
                        .await;
                }
                Err(ApiError::internal(
                    err.context("failed to create the project"),
                ))
            }
        }
    }

    /// Deletes a project: backs it up (unless `backup` is false), stops it on every
    /// instance and drops its database. Its files, and the backup unless it's in S3, are
    /// kept in `<name>.deleted-<time>` in the projects directory.
    pub async fn delete(&self, name: &str, backup: bool) -> Result<Deleted, ApiError> {
        let _guard = self.lock.lock().await;
        let root = self
            .root
            .upgrade()
            .ok_or_else(|| ApiError::unavailable("slonbase is stopping."))?;
        let core = root.core()?;
        let client = core
            .pool
            .get()
            .await
            .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
        let row = client
            .query_opt(
                "SELECT database, role, role_password FROM _slonbase.projects WHERE name = $1",
                &[&name],
            )
            .await
            .map_err(|e| ApiError::internal(e.into()))?
            .ok_or_else(ApiError::not_found)?;
        let database: String = row.get(0);
        let role: Option<(String, String)> = match (row.get(1), row.get(2)) {
            (Some(role), Some(password)) => Some((role, password)),
            _ => None,
        };
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();

        // the backup comes first: no backup, no delete
        let mut backup_in_s3 = false;
        let backup_name = format!("pre-delete-{stamp}.zip");
        if backup {
            let project_core = match self.get(name).map(|p| p.app.core()) {
                Some(Ok(core)) => core,
                // not running here (e.g. from the command line): start it for the backup
                _ => {
                    let row = Row {
                        name: name.to_string(),
                        database: database.clone(),
                        hosts: Vec::new(),
                        created: String::new(),
                        role: role.clone(),
                    };
                    let project = start(&root, row).await.map_err(ApiError::internal)?;
                    project.app.bootstrap().await.map_err(ApiError::internal)?;
                    project.app.core().map_err(|e| {
                        ApiError::bad_request(format!(
                            "The project can't be backed up ({}). Delete it without a backup \
                             to go ahead anyway.",
                            e.message
                        ))
                    })?
                }
            };
            backup_in_s3 = project_core.settings().backups.s3.enabled;
            crate::backups::create(&project_core, &backup_name)
                .await
                .map_err(|err| {
                    let mut err = ApiError::from(err);
                    err.message = format!("The project couldn't be backed up: {}", err.message);
                    err
                })?;
            tracing::info!(project = %name, backup = %backup_name, "backed up the project before deleting it");
        }

        client
            .execute("DELETE FROM _slonbase.projects WHERE name = $1", &[&name])
            .await
            .map_err(|e| ApiError::internal(e.into()))?;
        self.stop(name);
        let _ = client
            .execute("SELECT pg_notify($1, $2)", &[&CHANNEL, &name])
            .await;
        client
            .batch_execute(&format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                quote_ident(&database)
            ))
            .await
            .map_err(|e| {
                ApiError::internal(
                    anyhow::anyhow!(e).context("failed to drop the project's database"),
                )
            })?;
        if let Some((role, _)) = &role {
            if let Err(err) = client
                .batch_execute(&format!("DROP ROLE IF EXISTS {}", quote_ident(role)))
                .await
            {
                tracing::warn!(%role, error = %err, "failed to drop the project's role");
            }
        }

        let dir = root.config().effective.projects.path.join(name);
        let mut kept = None;
        if dir.exists() {
            let target = dir.with_file_name(format!("{name}.deleted-{stamp}"));
            match std::fs::rename(&dir, &target) {
                Ok(()) => kept = Some(target),
                Err(err) => {
                    tracing::warn!(dir = %dir.display(), error = %err, "failed to move a deleted project's files");
                    kept = Some(dir);
                }
            }
        }
        let backup = backup.then(|| {
            if backup_in_s3 {
                format!("{backup_name} in the project's backups bucket")
            } else {
                kept.as_ref()
                    .map(|dir| dir.join("backups").join(&backup_name).display().to_string())
                    .unwrap_or(backup_name)
            }
        });
        tracing::info!(project = %name, %database, "deleted project");
        Ok(Deleted {
            backup,
            files: kept.map(|dir| dir.display().to_string()),
        })
    }
}

impl Projects {
    /// Sets the host names a project is served on (besides `/p/{name}/`).
    pub async fn set_hosts(&self, name: &str, hosts: &[String]) -> Result<Arc<Project>, ApiError> {
        let mut clean: Vec<String> = Vec::new();
        for host in hosts {
            let host = normalize_host(host);
            if host.is_empty() {
                continue;
            }
            if !valid_host(&host) {
                return Err(ApiError::validation(FieldErrors::from([(
                    "hosts",
                    format!("{host:?} isn't a valid host name."),
                )])));
            }
            if !clean.contains(&host) {
                clean.push(host);
            }
        }
        {
            let _guard = self.lock.lock().await;
            let root = self
                .root
                .upgrade()
                .ok_or_else(|| ApiError::unavailable("slonbase is stopping."))?;
            let core = root.core()?;
            let client = core
                .pool
                .get()
                .await
                .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
            let taken = client
                .query_opt(
                    "SELECT name FROM _slonbase.projects WHERE name <> $1 AND hosts && $2",
                    &[&name, &clean],
                )
                .await
                .map_err(|e| ApiError::internal(e.into()))?;
            if let Some(row) = taken {
                let other: String = row.get(0);
                return Err(ApiError::validation(FieldErrors::from([(
                    "hosts",
                    format!("The project {other} already uses one of these host names."),
                )])));
            }
            let updated = client
                .execute(
                    "UPDATE _slonbase.projects SET hosts = $2 WHERE name = $1",
                    &[&name, &clean],
                )
                .await
                .map_err(|e| ApiError::internal(e.into()))?;
            if updated == 0 {
                return Err(ApiError::not_found());
            }
            let _ = client
                .execute("SELECT pg_notify($1, $2)", &[&CHANNEL, &name])
                .await;
        }
        self.reload().await.map_err(ApiError::internal)?;
        self.get(name).ok_or_else(ApiError::not_found)
    }
}

/// A host name as matched against projects: lowercase, without a port or trailing dot.
pub fn normalize_host(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    let host = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name.to_string(),
        _ => host,
    };
    host.trim_end_matches('.').to_string()
}

fn valid_host(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// What's left of a deleted project.
pub struct Deleted {
    /// Where the backup made before deleting is.
    pub backup: Option<String>,
    /// Where the project's files were moved.
    pub files: Option<String>,
}

async fn load_rows(core: &Core) -> anyhow::Result<Vec<Row>> {
    let client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    let rows = client
        .query(
            "SELECT name, database, hosts,
                    to_char(created AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.MS\"Z\"'),
                    role, role_password
             FROM _slonbase.projects ORDER BY name",
            &[],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| Row {
            name: r.get(0),
            database: r.get(1),
            hosts: r.get(2),
            created: r.get(3),
            role: match (r.get(4), r.get(5)) {
                (Some(role), Some(password)) => Some((role, password)),
                _ => None,
            },
        })
        .collect())
}

/// Builds a project's app (not started yet): the main configuration, with the project's
/// database and its own directories under `projects.path`.
async fn start(root: &App, row: Row) -> anyhow::Result<Project> {
    let config = project_config(&root.config(), &row.name, &row.database, row.role.clone());

    #[allow(unused_mut)]
    let mut extensions = crate::Extensions::default();
    #[cfg(feature = "js")]
    let hooks_error = {
        // a project's hooks failing to load leaves the project unavailable, not the server
        let hooks = config.effective.hooks.clone();
        crate::js::load(&hooks, &mut extensions).await.err()
    };
    #[cfg(not(feature = "js"))]
    let hooks_error: Option<anyhow::Error> = None;

    let app = App::for_project(&row.name, config.clone(), extensions)?;
    let router = crate::server::router(app.clone());
    #[cfg(feature = "js")]
    if config.effective.hooks.watch {
        watch_hooks(root, &row.name, &app, config.effective.hooks.dir.clone());
    }
    if let Some(err) = hooks_error {
        tracing::error!(project = %row.name, error = %format!("{err:#}"), "failed to load the project's hooks");
        app.set_mode(Mode::Starting {
            status: format!("the project's hooks failed to load: {err:#}"),
        });
    }
    Ok(Project {
        name: row.name,
        database: row.database,
        hosts: row.hosts,
        created: row.created,
        role: row.role,
        app,
        router,
    })
}

/// Restarts the project when its hook files change, as long as this app serves it.
#[cfg(feature = "js")]
fn watch_hooks(root: &App, name: &str, app: &Arc<App>, dir: std::path::PathBuf) {
    let root = root.projects.root.clone();
    let watched = Arc::downgrade(app);
    let name = name.to_string();
    tokio::spawn(async move {
        let gone = {
            let watched = watched.clone();
            async move {
                while watched.strong_count() > 0 {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        };
        tokio::select! {
            _ = crate::js::changed(dir) => {}
            _ = gone => return,
        }
        let Some(root) = root.upgrade() else { return };
        let current = root.projects.get(&name);
        let still_served = matches!(
            (current, watched.upgrade()),
            (Some(project), Some(app)) if Arc::ptr_eq(&project.app, &app)
        );
        if still_served {
            tracing::info!(project = %name, "the project's hook files changed, restarting it");
            if let Err(err) = root.projects.restart(&name).await {
                tracing::error!(project = %name, error = %format!("{err:#}"), "failed to restart the project");
            }
        }
    });
}

/// The configuration of a project: the main one, with the project's database and its own
/// directories under `projects.path`.
pub fn project_config(
    main: &Loaded,
    name: &str,
    database: &str,
    role: Option<(String, String)>,
) -> Loaded {
    let dir = main.effective.projects.path.join(name);
    let mut effective = main.effective.clone();
    effective.database.dbname = Some(database.to_string());
    effective.database.user = role;
    effective.database.max_connections = main.effective.projects.max_connections;
    effective.storage.path = dir.join("storage");
    effective.backups.path = dir.join("backups");
    effective.hooks.dir = dir.join("hooks");
    effective.migrations.dir = dir.join("migrations");
    Loaded {
        path: main.path.clone(),
        file: main.file.clone(),
        effective,
        // a project never saves the main configuration
        database_url_locked: true,
    }
}

/// The database of the project with this name, and its own role if it has one, from the
/// main installation's list.
pub async fn database_of(
    core: &Core,
    name: &str,
) -> anyhow::Result<Option<(String, Option<(String, String)>)>> {
    let client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    Ok(client
        .query_opt(
            "SELECT database, role, role_password FROM _slonbase.projects WHERE name = $1",
            &[&name],
        )
        .await?
        .map(|r| {
            let role = match (r.get(1), r.get(2)) {
                (Some(role), Some(password)) => Some((role, password)),
                _ => None,
            };
            (r.get(0), role)
        }))
}

/// Project names: lowercase letters, digits and dashes, starting with a letter.
pub fn validate_name(name: &str) -> Result<(), &'static str> {
    let valid = (2..=MAX_NAME_LENGTH).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err("Use 2 to 32 lowercase letters, digits and dashes, starting with a letter.")
    }
}

/// The database of a project: `<main database>_<name>` (dashes become underscores).
fn database_name(main: &str, name: &str) -> Option<String> {
    let database = format!("{main}_{}", name.replace('-', "_"));
    // PostgreSQL truncates longer names
    (database.len() <= 63).then_some(database)
}

fn name_error(message: impl Into<String>) -> ApiError {
    ApiError::validation(FieldErrors::from([("name", message.into())]))
}

// ---------------------------------------------------------------------- routing

/// Serves `/p/{name}/...` with the project's app (the path without the prefix).
pub(crate) async fn dispatch(State(app): State<Arc<App>>, req: Request) -> Response {
    let path = req.uri().path();
    let rest = &path[PATH_PREFIX.len()..];
    let (name, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let Some(project) = app.projects.get(name) else {
        return ApiError::new(StatusCode::NOT_FOUND, "The project doesn't exist.").into_response();
    };
    if tail.is_empty() {
        // relative links (the dashboard's, `./_/`) need the trailing slash
        let query = req
            .uri()
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        return Redirect::permanent(&format!("{PATH_PREFIX}{name}/{query}")).into_response();
    }
    let path_and_query = match req.uri().query() {
        Some(query) => format!("{tail}?{query}"),
        None => tail.to_string(),
    };
    let routed = Routed {
        base: format!("{PATH_PREFIX}{name}"),
        main_dashboard: "/_/".into(),
    };
    forward(&project, req, &path_and_query, routed).await
}

/// Serves requests for a project's host names with the project's app; other requests go
/// on to the main installation.
pub(crate) async fn host_dispatch(
    State(app): State<Arc<App>>,
    req: Request,
    next: axum::middleware::Next,
) -> Response {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| req.uri().host())
        .map(normalize_host);
    let project = host.and_then(|host| app.projects.by_host(&host));
    let Some(project) = project else {
        return next.run(req).await;
    };
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    // the main installation's dashboard is on its app URL
    let main_dashboard = match app.core() {
        Ok(core) => format!("{}/_/", core.settings().meta.app_url.trim_end_matches('/')),
        Err(_) => "/_/".into(),
    };
    let routed = Routed {
        base: String::new(),
        main_dashboard,
    };
    forward(&project, req, &path_and_query, routed).await
}

async fn forward(
    project: &Project,
    req: Request,
    path_and_query: &str,
    routed: Routed,
) -> Response {
    let uri = match path_and_query.parse::<Uri>() {
        Ok(uri) => uri,
        Err(_) => return ApiError::bad_request("Invalid path.").into_response(),
    };
    // a fresh request: the main router's path parameters must not reach the project's
    // handlers, only the client's address
    let (parts, body) = req.into_parts();
    let mut forwarded = Request::new(body);
    *forwarded.method_mut() = parts.method;
    *forwarded.uri_mut() = uri;
    *forwarded.version_mut() = parts.version;
    *forwarded.headers_mut() = parts.headers;
    if let Some(connect_info) = parts
        .extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        forwarded.extensions_mut().insert(*connect_info);
    }
    forwarded.extensions_mut().insert(routed);
    match project.router.clone().oneshot(forwarded).await {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

// ---------------------------------------------------------------------- API

fn require_superuser(auth: &RequestAuth) -> Result<(), ApiError> {
    if auth.is_superuser() {
        Ok(())
    } else {
        Err(ApiError::superusers_only())
    }
}

/// `GET /api/projects`
pub(crate) async fn list(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
) -> Result<Json<Value>, ApiError> {
    require_superuser(&auth)?;
    app.core()?;
    let items: Vec<Value> = app.projects.list().iter().map(|p| p.to_json()).collect();
    Ok(Json(json!({ "items": items })))
}

/// `GET /api/projects/{name}`
pub(crate) async fn view(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_superuser(&auth)?;
    let project = app.projects.get(&name).ok_or_else(ApiError::not_found)?;
    Ok(Json(project.to_json()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    superuser_email: String,
    #[serde(default)]
    superuser_password: String,
    #[serde(default)]
    own_role: bool,
}

/// `POST /api/projects`
pub(crate) async fn create(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    Json(req): Json<CreateRequest>,
) -> Result<Json<Value>, ApiError> {
    require_superuser(&auth)?;
    let project = app
        .projects
        .create(
            &req.name,
            &req.superuser_email,
            &req.superuser_password,
            req.own_role,
        )
        .await?;
    Ok(Json(project.to_json()))
}

#[derive(Deserialize)]
pub(crate) struct UpdateRequest {
    hosts: Option<Vec<String>>,
}

/// `PATCH /api/projects/{name}`: `{"hosts": [...]}`
pub(crate) async fn update(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(req): Json<UpdateRequest>,
) -> Result<Json<Value>, ApiError> {
    require_superuser(&auth)?;
    let project = match req.hosts {
        Some(hosts) => app.projects.set_hosts(&name, &hosts).await?,
        None => app.projects.get(&name).ok_or_else(ApiError::not_found)?,
    };
    Ok(Json(project.to_json()))
}

#[derive(Deserialize)]
pub(crate) struct DeleteQuery {
    #[serde(default)]
    confirm: String,
    #[serde(default = "default_true")]
    backup: bool,
}

fn default_true() -> bool {
    true
}

/// `DELETE /api/projects/{name}?confirm={name}[&backup=false]`: the name is repeated so a
/// project is never deleted by mistake. Answers where the backup and files were kept.
pub(crate) async fn delete(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    axum::extract::Path(name): axum::extract::Path<String>,
    Query(query): Query<DeleteQuery>,
) -> Result<Json<Value>, ApiError> {
    require_superuser(&auth)?;
    if query.confirm != name {
        return Err(ApiError::bad_request(
            "Repeat the project's name in the confirm parameter to delete it.",
        ));
    }
    let deleted = app.projects.delete(&name, query.backup).await?;
    Ok(Json(
        json!({ "backup": deleted.backup, "files": deleted.files }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["ab", "blog", "my-app-2", "a1"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "a",
            "Blog",
            "1app",
            "my_app",
            "app-",
            "a b",
            &"x".repeat(33),
        ] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
        assert_eq!(
            database_name("slonbase", "my-app").as_deref(),
            Some("slonbase_my_app")
        );
        assert_eq!(database_name(&"d".repeat(60), "abcd"), None);
        assert_eq!(normalize_host("Shop.Example.com:8090"), "shop.example.com");
        assert_eq!(normalize_host("shop.example.com."), "shop.example.com");
        assert!(valid_host("shop.example.com") && valid_host("localhost"));
        assert!(!valid_host("-x.com") && !valid_host("a..b") && !valid_host("a_b.com"));
    }
}
