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

    #[command(subcommand)]
    command: Option<Command>,
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
        .with(tracing_subscriber::fmt::layer().with_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        ))
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
            let loaded = config::load(
                &cli.config,
                Overrides {
                    sql_console: Some(true),
                    ..overrides
                },
            )?;
            let cfg = &loaded.effective;
            let url = cfg.database.url.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "no database configured: run `slonbase setup` first, or pass --database-url"
                )
            })?;
            let pool = db::connect(&url, &cfg.database)?;
            crate::migrations::run(&pool).await?;
            let core = crate::core::Core::load(
                pool,
                cfg,
                std::sync::Arc::new(crate::hooks::Hooks::default()),
            )
            .await?;
            let report = crate::import_pb::run(&core, &source, !no_settings).await?;
            print!("{report}");
            Ok(())
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
            stopping.notify_one();
        }
    };

    let serve = axum::serve(
        listener,
        server::router(app).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown);
    let restarting = restart.clone();
    tokio::select! {
        result = serve => result.context("server error")?,
        // long-lived connections (realtime) don't hold up a restart
        _ = async {
            stopping.notified().await;
            if restarting.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(RESTART_GRACE).await;
            } else {
                std::future::pending::<()>().await;
            }
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

/// How long a restart waits for open requests.
const RESTART_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

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
