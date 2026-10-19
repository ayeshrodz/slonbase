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

    /// Directory for the projects' files, backups and hooks [default: slonbase_data/projects].
    #[arg(long, global = true, env = "SLONBASE_PROJECTS_PATH")]
    projects_path: Option<PathBuf>,

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
            )
            .await?;
            let report = crate::import_pb::run(&core, &source, !no_settings).await?;
            print!("{report}");
            Ok(())
        }
        Command::Superuser { action } => {
            let core = offline_core(&cli.config, overrides).await?;
            superuser_command(&core, action).await
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

/// The core for a command that works on the database without serving (import, superuser).
async fn offline_core(
    config_path: &std::path::Path,
    overrides: Overrides,
) -> Result<std::sync::Arc<crate::core::Core>> {
    let loaded = config::load(config_path, overrides)?;
    let cfg = &loaded.effective;
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
