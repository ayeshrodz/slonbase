//! Configuration loading and persistence.
//!
//! Precedence (highest first): CLI flags > environment variables > config file > defaults.
//! Flags and env vars are resolved by clap (see `main.rs`) and applied on top of the
//! file config via [`Overrides`].

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_BIND: &str = "127.0.0.1:8090";
pub const DEFAULT_MAX_CONNECTIONS: usize = 16;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub storage: StorageConfig,
    pub backups: BackupsConfig,
    pub hooks: HooksConfig,
    pub projects: ProjectsConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectsConfig {
    /// Directory with each project's files, backups and hooks (`<path>/<project>/...`).
    pub path: PathBuf,
    /// Maximum number of pooled connections of each project.
    pub max_connections: usize,
}

impl Default for ProjectsConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("slonbase_data/projects"),
            max_connections: 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HooksConfig {
    /// Directory with the JavaScript hooks (`*.pb.js`); ignored when it doesn't exist.
    pub dir: PathBuf,
    /// Number of JavaScript runtimes (each runs one hook at a time).
    pub pool_size: usize,
    /// Restart the server when the hook files change.
    pub watch: bool,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("slonbase_hooks"),
            pool_size: 10,
            watch: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackupsConfig {
    /// Directory for backups (unless the settings store them in S3).
    pub path: PathBuf,
}

impl Default for BackupsConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("slonbase_data/backups"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    /// Directory for uploaded files. Instances sharing a database must share it too.
    pub path: PathBuf,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("slonbase_data/storage"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Address the HTTP server listens on.
    pub bind: String,
    /// Enables the dashboard's SQL console, which runs any SQL a superuser enters with the
    /// database role's privileges (off by default, see the README).
    pub sql_console: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND.to_string(),
            sql_console: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseConfig {
    /// PostgreSQL connection URL. When missing, the server starts in setup mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Maximum number of pooled connections.
    pub max_connections: usize,
    /// Extra PEM CA bundle to trust for TLS connections (e.g. a managed provider's CA).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<PathBuf>,
    /// Connects to this database instead of the URL's (set for projects, never saved).
    #[serde(skip)]
    pub dbname: Option<String>,
    /// Connects as this role instead of the URL's (projects with a role of their own).
    #[serde(skip)]
    pub user: Option<(String, String)>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: None,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            ca_file: None,
            dbname: None,
            user: None,
        }
    }
}

/// Values coming from CLI flags or environment variables.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub bind: Option<String>,
    pub database_url: Option<String>,
    pub max_connections: Option<usize>,
    pub ca_file: Option<PathBuf>,
    pub storage_path: Option<PathBuf>,
    pub backups_path: Option<PathBuf>,
    pub sql_console: Option<bool>,
    pub hooks_dir: Option<PathBuf>,
    pub hooks_pool_size: Option<usize>,
    pub hooks_watch: Option<bool>,
    pub projects_path: Option<PathBuf>,
}

/// The result of loading the configuration.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// Where the config file lives (it may not exist yet).
    pub path: PathBuf,
    /// The config as stored in the file, without overrides.
    /// This is what gets written back, so env values are never persisted.
    pub file: Config,
    /// The config the server actually runs with.
    pub effective: Config,
    /// True when the database URL comes from a flag or env var,
    /// so the setup wizard must not ask for (or save) one.
    pub database_url_locked: bool,
}

pub fn load(path: &Path, overrides: Overrides) -> Result<Loaded> {
    let file = match fs::read_to_string(path) {
        Ok(raw) => toml::from_str::<Config>(&raw)
            .with_context(|| format!("invalid config file {}", path.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", path.display()));
        }
    };

    let mut effective = file.clone();
    let database_url_locked = overrides.database_url.is_some();
    if let Some(bind) = overrides.bind {
        effective.server.bind = bind;
    }
    if let Some(url) = overrides.database_url {
        effective.database.url = Some(url);
    }
    if let Some(max) = overrides.max_connections {
        effective.database.max_connections = max;
    }
    if let Some(ca_file) = overrides.ca_file {
        effective.database.ca_file = Some(ca_file);
    }
    if let Some(path) = overrides.storage_path {
        effective.storage.path = path;
    }
    if let Some(path) = overrides.backups_path {
        effective.backups.path = path;
    }
    if let Some(enabled) = overrides.sql_console {
        effective.server.sql_console = enabled;
    }
    if let Some(dir) = overrides.hooks_dir {
        effective.hooks.dir = dir;
    }
    if let Some(size) = overrides.hooks_pool_size {
        effective.hooks.pool_size = size;
    }
    if let Some(watch) = overrides.hooks_watch {
        effective.hooks.watch = watch;
    }
    if let Some(path) = overrides.projects_path {
        effective.projects.path = path;
    }

    if effective.database.max_connections == 0 {
        anyhow::bail!("database.max_connections must be greater than 0");
    }
    if effective.projects.max_connections == 0 {
        anyhow::bail!("projects.max_connections must be greater than 0");
    }

    Ok(Loaded {
        path: path.to_path_buf(),
        file,
        effective,
        database_url_locked,
    })
}

/// Atomically writes the config file with owner-only permissions (it contains the
/// database password): write a temp file in the same directory, fsync, then rename.
pub fn save(path: &Path, config: &Config) -> Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let mut body = String::from(
        "# Written by slonbase. You can edit this file; restart slonbase to apply changes.\n\n",
    );
    body.push_str(&toml::to_string_pretty(config)?);

    let file_name = path
        .file_name()
        .context("config path has no file name")?
        .to_string_lossy();
    let tmp_path = dir.join(format!(".{file_name}.tmp-{}", crate::ids::random_id(8)));

    let result = (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp_path)
            .with_context(|| format!("failed to create {}", tmp_path.display()))?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp_path, path)
            .with_context(|| format!("failed to write {}", path.display()))?;
        // persist the rename itself
        #[cfg(unix)]
        fs::File::open(&dir)?.sync_all()?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(&dir.path().join("config.toml"), Overrides::default()).unwrap();
        assert_eq!(loaded.effective, Config::default());
        assert!(!loaded.database_url_locked);
    }

    #[test]
    fn overrides_win_but_are_not_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[server]\nbind = \"0.0.0.0:9000\"\n[database]\nurl = \"postgres://file\"\n",
        )
        .unwrap();

        let loaded = load(
            &path,
            Overrides {
                database_url: Some("postgres://env".into()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(loaded.effective.server.bind, "0.0.0.0:9000");
        assert_eq!(
            loaded.effective.database.url.as_deref(),
            Some("postgres://env")
        );
        assert_eq!(loaded.file.database.url.as_deref(), Some("postgres://file"));
        assert!(loaded.database_url_locked);
    }

    #[test]
    fn rejects_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[database]\nurll = \"typo\"\n").unwrap();
        assert!(load(&path, Overrides::default()).is_err());
    }

    #[test]
    fn save_roundtrips_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let mut config = Config::default();
        config.database.url = Some("postgres://u:secret@db/app".into());

        save(&path, &config).unwrap();
        save(&path, &config).unwrap(); // overwriting works too

        let loaded = load(&path, Overrides::default()).unwrap();
        assert_eq!(loaded.file, config);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // no temp files left behind
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
