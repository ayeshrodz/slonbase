//! The `slonbase` command line, also available to programs that embed slonbase with
//! their own hooks and routes (see [`crate::Extensions`]).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::parser::ValueSource;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use tracing_subscriber::{EnvFilter, Layer};

use crate::Extensions;
use crate::app::{App, Mode};
use crate::config::{self, Overrides};
use crate::{db, server, setup};

#[derive(Parser)]
#[command(
    name = "slonbase",
    version,
    about = "A lightweight backend powered by PostgreSQL"
)]
struct Cli {
    /// Path to the config file (created by the setup wizard if missing).
    #[arg(
        long,
        global = true,
        env = "SLONBASE_CONFIG",
        default_value = "slonbase.toml"
    )]
    config: PathBuf,

    /// Address to listen on [default: 127.0.0.1:8090].
    #[arg(long, global = true, env = "SLONBASE_BIND")]
    bind: Option<String>,

    /// PostgreSQL connection URL.
    #[arg(
        long,
        global = true,
        env = "SLONBASE_DATABASE_URL",
        hide_env_values = true
    )]
    database_url: Option<String>,

    /// Maximum number of pooled database connections [default: 16].
    #[arg(long, global = true, env = "SLONBASE_DATABASE_MAX_CONNECTIONS")]
    database_max_connections: Option<usize>,

    /// Extra PEM CA bundle to trust for database TLS connections.
    #[arg(long, global = true, env = "SLONBASE_DATABASE_CA_FILE")]
    database_ca_file: Option<PathBuf>,

    /// Directory for uploaded files [default: slonbase_data/storage].
    #[arg(long, global = true, env = "SLONBASE_STORAGE_PATH")]
    storage_path: Option<PathBuf>,

    /// Directory for backups [default: slonbase_data/backups].
    #[arg(long, global = true, env = "SLONBASE_BACKUPS_PATH")]
    backups_path: Option<PathBuf>,

    /// Enable the dashboard's SQL console (runs any SQL with the database role's privileges).
    #[arg(long, global = true, env = "SLONBASE_SQL_CONSOLE")]
    sql_console: bool,

    /// Directory with JavaScript hooks (`*.pb.js`) [default: slonbase_hooks].
    #[arg(long, global = true, env = "SLONBASE_HOOKS_DIR")]
    hooks_dir: Option<PathBuf>,

    /// Number of JavaScript runtimes for the hooks [default: 10].
    #[arg(long, global = true, env = "SLONBASE_HOOKS_POOL")]
    hooks_pool: Option<usize>,

    /// Restart when the hook files change [default: true].
    #[arg(long, global = true, env = "SLONBASE_HOOKS_WATCH")]
    hooks_watch: Option<bool>,

    /// Directory with JavaScript migrations (`*.js`) [default: slonbase_migrations].
    #[arg(long, global = true, env = "SLONBASE_MIGRATIONS_DIR")]
    migrations_dir: Option<PathBuf>,

    /// Write a migration file for every collection change made in the dashboard [default: false].
    #[arg(long, global = true, env = "SLONBASE_AUTOMIGRATE")]
    automigrate: Option<bool>,

    /// Directory for the projects' files, backups and hooks [default: slonbase_data/projects].
    #[arg(long, global = true, env = "SLONBASE_PROJECTS_PATH")]
    projects_path: Option<PathBuf>,

    /// Run `superuser`, `import-pocketbase` or `migrate` on this project instead of the main installation.
    #[arg(long, global = true, env = "SLONBASE_PROJECT")]
    project: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum SuperuserAction {
    /// Create a superuser, or set the password of the one with this email.
    Upsert { email: String, password: String },
    /// Set the password of an existing superuser (signs out its sessions).
    Update { email: String, password: String },
    /// Delete a superuser (the last one can't be deleted).
    Delete { email: String },
    /// Print a one-time password for a superuser, to sign in when emails can't be sent
    /// (for example to pass multi-factor auth). OTPs must be on for _superusers.
    Otp { email: String },
}

#[derive(Subcommand)]
enum MigrateAction {
    /// Apply the pending migrations (the server also does when it starts).
    Up,
    /// Revert the last applied migrations.
    Down {
        /// How many to revert.
        #[arg(default_value_t = 1)]
        count: usize,
    },
    /// Write a new, blank migration file.
    Create {
        /// What the migration does, for its file name (e.g. add_posts).
        name: String,
    },
    /// Write a migration that recreates every collection as it is now.
    Collections,
    /// Forget applied migrations whose files were deleted.
    HistorySync,
    /// List the migration files and whether they're applied.
    List,
}

#[derive(Subcommand)]
enum ProjectAction {
    /// List the projects.
    List,
    /// Create a project: a new database on the same server, with its first superuser.
    Create {
        /// The project's name (lowercase letters, digits and dashes); it's served at /p/{name}/.
        name: String,
        /// Email of the project's first superuser.
        #[arg(long, env = "SLONBASE_SUPERUSER_EMAIL")]
        email: String,
        /// Password of the project's first superuser (prefer the env var).
        #[arg(long, env = "SLONBASE_SUPERUSER_PASSWORD", hide_env_values = true)]
        password: String,
        /// Give the project a PostgreSQL role of its own, owning its database (needs CREATEROLE).
        #[arg(long)]
        own_role: bool,
    },
    /// Show or set the host names a project is served on, besides /p/{name}/.
    Hosts {
        name: String,
        /// The host names (replacing the current ones), e.g. shop.example.com.
        hosts: Vec<String>,
        /// Remove all host names.
        #[arg(long, conflicts_with = "hosts")]
        clear: bool,
    },
    /// Delete a project: back it up, then drop its database.
    Delete {
        name: String,
        /// The project's name again, to confirm (asked for when missing).
        #[arg(long)]
        confirm: Option<String>,
        /// Don't back the project up first.
        #[arg(long)]
        no_backup: bool,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Start the server (default).
    Serve {
        /// Preset the one-time setup token instead of generating one (min 32 characters).
        #[arg(long, env = "SLONBASE_SETUP_TOKEN", hide_env_values = true)]
        setup_token: Option<String>,
    },
    /// Set up slonbase without the web wizard: initialize the database and
    /// create the first superuser. A --database-url passed here is saved to the config file.
    Setup {
        /// Email of the first superuser.
        #[arg(long, env = "SLONBASE_SUPERUSER_EMAIL")]
        email: String,
        /// Password of the first superuser (prefer the env var to keep it out of shell history).
        #[arg(long, env = "SLONBASE_SUPERUSER_PASSWORD", hide_env_values = true)]
        password: String,
    },
    /// Check that a local server is healthy (exit code 0) - for container health checks.
    Health,
    /// Manage superusers from the command line (with the database of the config).
    Superuser {
        #[command(subcommand)]
        action: SuperuserAction,
    },
    /// Import a PocketBase installation (a pb_data directory or a backup zip): collections,
    /// records with their password hashes, superusers, OAuth2 links, files and settings.
    /// Run it with the server stopped, against a database without those collections.
    ImportPocketbase {
        /// The pb_data directory (with data.db) or a PocketBase backup zip.
        source: std::path::PathBuf,
        /// Don't import PocketBase's settings (SMTP, S3, ...).
        #[arg(long)]
        no_settings: bool,
    },
    /// Manage projects: further installations, each in its own database (see docs/projects.md).
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
    /// Apply, revert or write JavaScript migrations (see docs/migrations.md).
    Migrate {
        #[command(subcommand)]
        action: MigrateAction,
    },
}

/// Runs the command line (`serve`, `setup`, `health`) with the given extensions. Logs go
/// to stderr (filtered with `RUST_LOG`) unless a tracing subscriber is already set.
pub async fn run(extensions: Extensions) -> ExitCode {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(
                    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
                ),
        )
        // warnings and errors also go to the logs shown in the dashboard
        .with(crate::logs::AppLogsLayer)
        .try_init();

    match run_cli(extensions).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run_cli(extensions: Extensions) -> Result<()> {
    let matches = Cli::command().get_matches();
    // global flags can appear before or after the subcommand
    let url_from_flag = std::iter::once(&matches)
        .chain(matches.subcommand().map(|(_, sub)| sub))
        .any(|m| m.value_source("database_url") == Some(ValueSource::CommandLine));
    let cli = Cli::from_arg_matches(&matches)?;

    let overrides = Overrides {
        bind: cli.bind,
        database_url: cli.database_url,
        max_connections: cli.database_max_connections,
        ca_file: cli.database_ca_file,
        storage_path: cli.storage_path,
        backups_path: cli.backups_path,
        sql_console: cli.sql_console.then_some(true),
        hooks_dir: cli.hooks_dir,
        hooks_pool_size: cli.hooks_pool,
        hooks_watch: cli.hooks_watch,
        migrations_dir: cli.migrations_dir,
        automigrate: cli.automigrate,
        projects_path: cli.projects_path,
    };

    match cli.command.unwrap_or(Command::Serve { setup_token: None }) {
        Command::Serve { setup_token } => {
            serve(
                config::load(&cli.config, overrides)?,
                setup_token,
                extensions,
            )
            .await
        }
        Command::Health => {
            let loaded = config::load(&cli.config, overrides)?;
            server::check_local_health(&loaded.effective.server.bind).await
        }
        Command::ImportPocketbase {
            source,
            no_settings,
        } => {
            // the operator already has the database: view queries may be created
            let core = offline_core(
                &cli.config,
                Overrides {
                    sql_console: Some(true),
                    ..overrides
                },
                cli.project.as_deref(),
            )
            .await?;
            let report = crate::import_pb::run(&core, &source, !no_settings).await?;
            print!("{report}");
            Ok(())
        }
        Command::Superuser { action } => {
            let core = offline_core(&cli.config, overrides, cli.project.as_deref()).await?;
            superuser_command(&core, action).await
        }
        Command::Project { action } => {
            let loaded = config::load(&cli.config, overrides)?;
            project_command(loaded, action).await
        }
        Command::Migrate { action } => {
            let (core, cfg) =
                offline_core_with_config(&cli.config, overrides, cli.project.as_deref()).await?;
            migrate_command(&core, &cfg, action).await
        }
        Command::Setup { email, password } => {
            // a URL given on the command line is meant to be saved,
            // so treat it like one entered in the wizard
            let request_url = if url_from_flag {
                overrides.database_url.clone()
            } else {
                None
            };
            let loaded = config::load(
                &cli.config,
                Overrides {
                    database_url: if url_from_flag {
                        None
                    } else {
                        overrides.database_url.clone()
                    },
                    ..overrides
                },
            )?;

            let pool = match (&request_url, &loaded.effective.database.url) {
                (Some(_), _) => None,
                (None, Some(url)) => Some(db::connect(url, &loaded.effective.database)?),
                (None, None) => anyhow::bail!(
                    "no database configured: pass --database-url or set SLONBASE_DATABASE_URL"
                ),
            };

            let req = setup::SetupRequest {
                database_url: request_url,
                email,
                password,
            };
            let (_, saved) = setup::perform(pool, &loaded, req)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if let Some(saved) = saved {
                println!(
                    "Saved the database configuration to {}",
                    saved.path.display()
                );
            }
            println!("Setup completed. Start the server with `slonbase serve`.");
            Ok(())
        }
    }
}

/// The core for a command that works on the database without serving (import, superuser):
/// the main installation's, or a project's.
async fn offline_core(
    config_path: &std::path::Path,
    overrides: Overrides,
    project: Option<&str>,
) -> Result<std::sync::Arc<crate::core::Core>> {
    Ok(offline_core_with_config(config_path, overrides, project)
        .await?
        .0)
}

/// [`offline_core`] with the configuration it runs with.
async fn offline_core_with_config(
    config_path: &std::path::Path,
    overrides: Overrides,
    project: Option<&str>,
) -> Result<(std::sync::Arc<crate::core::Core>, config::Config)> {
    let loaded = config::load(config_path, overrides)?;
    let main = core_for(&loaded.effective).await?;
    let Some(name) = project else {
        return Ok((main, loaded.effective));
    };
    let (database, role) = crate::projects::database_of(&main, name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("there's no project named {name:?}"))?;
    let project = crate::projects::project_config(&loaded, name, &database, role);
    Ok((core_for(&project.effective).await?, project.effective))
}

#[cfg(feature = "js")]
async fn migrate_command(
    core: &std::sync::Arc<crate::core::Core>,
    cfg: &config::Config,
    action: MigrateAction,
) -> Result<()> {
    use crate::js::migrate;
    let dir = &cfg.migrations.dir;
    match action {
        MigrateAction::Up => {
            let applied = migrate::up(core, dir, &cfg.hooks.dir).await?;
            if applied.is_empty() {
                println!("No pending migrations in {}.", dir.display());
            }
            for name in applied {
                println!("Applied {name}");
            }
        }
        MigrateAction::Down { count } => {
            let reverted = migrate::down(core, dir, &cfg.hooks.dir, count).await?;
            if reverted.is_empty() {
                println!("No applied migrations to revert.");
            }
            for name in reverted {
                println!("Reverted {name}");
            }
        }
        MigrateAction::Create { name } => {
            let path = migrate::write_blank(dir, &name)?;
            println!("Wrote {}", path.display());
        }
        MigrateAction::Collections => {
            let path = migrate::write_snapshot(dir, &core.registry())?;
            // it describes the collections as they are: there's nothing to apply
            let client = core.pool.get().await.map_err(db::describe_pool_error)?;
            let file = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            client
                .execute(
                    "INSERT INTO _slonbase.app_migrations (file) VALUES ($1) ON CONFLICT DO NOTHING",
                    &[&file],
                )
                .await?;
            println!("Wrote {} (marked as applied here)", path.display());
        }
        MigrateAction::HistorySync => {
            let removed = migrate::sync_history(core, dir).await?;
            if removed.is_empty() {
                println!("The history matches the files in {}.", dir.display());
            }
            for name in removed {
                println!("Forgot {name}");
            }
        }
        MigrateAction::List => {
            let files = migrate::files(dir)?;
            let applied = migrate::applied(core).await?;
            if files.is_empty() && applied.is_empty() {
                println!("No migrations in {}.", dir.display());
            }
            for (name, _) in &files {
                let state = if applied.contains(name) {
                    "applied"
                } else {
                    "pending"
                };
                println!("{state:<8} {name}");
            }
            for name in applied
                .iter()
                .filter(|a| !files.iter().any(|(n, _)| n == *a))
            {
                println!("{:<8} {name} (the file is missing)", "applied");
            }
        }
    }
    Ok(())
}

#[cfg(not(feature = "js"))]
async fn migrate_command(
    _core: &std::sync::Arc<crate::core::Core>,
    _cfg: &config::Config,
    _action: MigrateAction,
) -> Result<()> {
    anyhow::bail!("this build has no JavaScript engine, which migrations need")
}

async fn core_for(cfg: &config::Config) -> Result<std::sync::Arc<crate::core::Core>> {
    let url = cfg.database.url.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "no database configured: run `slonbase setup` first, or pass --database-url"
        )
    })?;
    let pool = db::connect(&url, &cfg.database)?;
    crate::migrations::run(&pool).await?;
    crate::core::Core::load(
        pool,
        cfg,
        std::sync::Arc::new(crate::hooks::Hooks::default()),
    )
    .await
}

async fn project_command(loaded: config::Loaded, action: ProjectAction) -> Result<()> {
    let core = core_for(&loaded.effective).await?;
    // the main installation, ready but without starting its projects
    let app = App::new(loaded, None)?;
    app.set_mode(crate::app::Mode::Ready { core: core.clone() });
    let fail = |err: crate::error::ApiError| {
        let mut message = err.message;
        for (field, detail) in &err.data {
            if let Some(detail) = detail["message"].as_str() {
                message.push_str(&format!("\n  {field}: {detail}"));
            }
        }
        anyhow::anyhow!(message)
    };
    match action {
        ProjectAction::List => {
            let client = core.pool.get().await.map_err(db::describe_pool_error)?;
            let rows = client
                .query(
                    "SELECT name, database, to_char(created, 'YYYY-MM-DD HH24:MI') FROM _slonbase.projects ORDER BY name",
                    &[],
                )
                .await?;
            if rows.is_empty() {
                println!("No projects yet. Create one with `slonbase project create <name>`.");
            }
            for row in rows {
                let (name, database, created): (String, String, String) =
                    (row.get(0), row.get(1), row.get(2));
                println!("{name:<24} /p/{name}/  database {database}, created {created}");
            }
            Ok(())
        }
        ProjectAction::Create {
            name,
            email,
            password,
            own_role,
        } => {
            let project = app
                .projects()
                .create(&name, &email, &password, own_role)
                .await
                .map_err(fail)?;
            println!(
                "Created project {name} (database {}{}). Running servers serve it at /p/{name}/.",
                project.database,
                if own_role {
                    ", with a role of its own"
                } else {
                    ""
                }
            );
            Ok(())
        }
        ProjectAction::Hosts { name, hosts, clear } => {
            if hosts.is_empty() && !clear {
                let client = core.pool.get().await.map_err(db::describe_pool_error)?;
                let row = client
                    .query_opt(
                        "SELECT hosts FROM _slonbase.projects WHERE name = $1",
                        &[&name],
                    )
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("there's no project named {name:?}"))?;
                let hosts: Vec<String> = row.get(0);
                if hosts.is_empty() {
                    println!("{name} has no host names; it's served at /p/{name}/.");
                }
                for host in hosts {
                    println!("{host}");
                }
                return Ok(());
            }
            let project = app
                .projects()
                .set_hosts(&name, &hosts)
                .await
                .map_err(fail)?;
            if project.hosts.is_empty() {
                println!("{name} has no host names now.");
            } else {
                println!(
                    "{name} is served on {} (and /p/{name}/).",
                    project.hosts.join(", ")
                );
            }
            Ok(())
        }
        ProjectAction::Delete {
            name,
            confirm,
            no_backup,
        } => {
            let confirm = match confirm {
                Some(confirm) => confirm,
                None => {
                    use std::io::Write;
                    print!("This drops the database of project {name}. Type its name to confirm: ");
                    std::io::stdout().flush()?;
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line.trim().to_string()
                }
            };
            if confirm != name {
                anyhow::bail!("the name didn't match, nothing was deleted");
            }
            let deleted = app
                .projects()
                .delete(&name, !no_backup)
                .await
                .map_err(fail)?;
            println!("Deleted project {name}.");
            if let Some(backup) = deleted.backup {
                println!("Backup: {backup}");
            }
            if let Some(files) = deleted.files {
                println!("Files: {files}");
            }
            Ok(())
        }
    }
}

async fn superuser_command(
    core: &std::sync::Arc<crate::core::Core>,
    action: SuperuserAction,
) -> Result<()> {
    use crate::collections::model::SUPERUSERS_COLLECTION_NAME as SUPERUSERS;
    let find = |email: String| {
        let core = core.clone();
        async move {
            let mut params = serde_json::Map::new();
            params.insert("email".into(), serde_json::Value::String(email.clone()));
            core.find_records_by_filter(SUPERUSERS, "email = {:email}", "", 1, 0, &params)
                .await
                .map_err(|e| anyhow::anyhow!(e.message))
                .map(|found| found.into_iter().next())
        }
    };
    let save = |data: serde_json::Map<String, serde_json::Value>| {
        let core = core.clone();
        async move {
            core.save_record(SUPERUSERS, data).await.map_err(|e| {
                let details: Vec<String> = e
                    .data
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v["message"].as_str().unwrap_or_default()))
                    .collect();
                anyhow::anyhow!("{} {}", e.message, details.join("; "))
            })
        }
    };
    let password_data = |id: Option<&serde_json::Value>, email: &str, password: &str| {
        let mut data = serde_json::Map::new();
        if let Some(id) = id {
            data.insert("id".into(), id.clone());
        } else {
            data.insert("email".into(), serde_json::Value::String(email.to_string()));
        }
        data.insert(
            "password".into(),
            serde_json::Value::String(password.to_string()),
        );
        data.insert(
            "passwordConfirm".into(),
            serde_json::Value::String(password.to_string()),
        );
        data
    };
    match action {
        SuperuserAction::Upsert { email, password } => {
            let existing = find(email.clone()).await?;
            let id = existing.as_ref().and_then(|r| r.get("id"));
            save(password_data(id, &email, &password)).await?;
            println!(
                "{} superuser {email}",
                if existing.is_some() {
                    "Updated"
                } else {
                    "Created"
                }
            );
        }
        SuperuserAction::Update { email, password } => {
            let existing = find(email.clone())
                .await?
                .ok_or_else(|| anyhow::anyhow!("no superuser has the email {email:?}"))?;
            save(password_data(existing.get("id"), &email, &password)).await?;
            println!("Updated superuser {email}");
        }
        SuperuserAction::Delete { email } => {
            let existing = find(email.clone())
                .await?
                .ok_or_else(|| anyhow::anyhow!("no superuser has the email {email:?}"))?;
            let id = existing
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            core.delete_record(SUPERUSERS, &id)
                .await
                .map_err(|e| anyhow::anyhow!(e.message))?;
            println!("Deleted superuser {email}");
        }
        SuperuserAction::Otp { email } => {
            let (id, password) = crate::records::otp::issue(core, SUPERUSERS, &email).await?;
            let minutes = core
                .registry()
                .get(SUPERUSERS)
                .and_then(|c| c.auth_options().map(|o| o.otp.duration))
                .unwrap_or(0)
                / 60;
            println!("One-time password for {email}: {password}");
            println!("otpId: {id} (valid for about {minutes} min, single use)");
        }
    }
    Ok(())
}

async fn serve(
    loaded: config::Loaded,
    setup_token: Option<String>,
    #[allow(unused_mut)] mut extensions: Extensions,
) -> Result<()> {
    #[cfg(feature = "js")]
    crate::js::load(&loaded.effective.hooks, &mut extensions).await?;
    let bind = loaded.effective.server.bind.clone();
    let hooks = loaded.effective.hooks.clone();
    let app = App::with_extensions(loaded, setup_token, extensions)?;

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("failed to listen on {bind}"))?;
    tracing::info!(address = %bind, version = env!("CARGO_PKG_VERSION"), "slonbase listening");

    // the server answers health checks (503) while the database is being prepared;
    // an unrecoverable bootstrap error shuts the server down gracefully
    let (fatal_tx, fatal_rx) = tokio::sync::oneshot::channel::<anyhow::Error>();
    {
        let app = app.clone();
        tokio::spawn(async move {
            match app.bootstrap().await {
                Ok(()) => {
                    if let Mode::Setup(_) = app.mode() {
                        server::announce_setup(&app, &bind);
                    }
                }
                Err(err) => {
                    let _ = fatal_tx.send(err);
                }
            }
        });
    }

    let fatal = std::sync::Arc::new(std::sync::Mutex::new(None));
    let restart = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopping = std::sync::Arc::new(tokio::sync::Notify::new());
    let shutdown = {
        let (fatal, restart, stopping) = (fatal.clone(), restart.clone(), stopping.clone());
        let app = app.clone();
        async move {
            tokio::select! {
                _ = server::shutdown_signal() => {}
                Ok(err) = fatal_rx => {
                    *fatal.lock().expect("fatal lock") = Some(err);
                }
                _ = hooks_changed(&hooks) => {
                    tracing::info!("the hook files changed, restarting");
                    restart.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            // end the realtime streams, so only requests in flight hold up the stop
            if let Ok(core) = app.core() {
                core.realtime.disconnect_all();
            }
            stopping.notify_one();
        }
    };

    let serve = axum::serve(
        listener,
        server::router(app).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown);
    tokio::select! {
        result = serve => result.context("server error")?,
        // nor do connections that don't finish in time
        _ = async {
            stopping.notified().await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        } => {}
    }

    if let Some(err) = fatal.lock().expect("fatal lock").take() {
        return Err(err);
    }
    if restart.load(std::sync::atomic::Ordering::SeqCst) {
        return restart_process();
    }
    Ok(())
}

/// How long a stop or restart waits for open requests.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Resolves when the hook files change (never, when watching is off or without the
/// JavaScript engine).
async fn hooks_changed(hooks: &config::HooksConfig) {
    #[cfg(feature = "js")]
    if hooks.watch {
        return crate::js::changed(hooks.dir.clone()).await;
    }
    let _ = hooks;
    std::future::pending::<()>().await
}

/// Replaces the process with a fresh start of the same command.
fn restart_process() -> Result<()> {
    let exe = std::env::current_exe().context("failed to find the executable to restart")?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&exe)
            .args(std::env::args_os().skip(1))
            .exec();
        Err(anyhow::anyhow!(
            "failed to restart {}: {err}",
            exe.display()
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = exe;
        anyhow::bail!("the hook files changed; restart slonbase to load them")
    }
}
