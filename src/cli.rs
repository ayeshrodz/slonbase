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
    extensions: Extensions,
) -> Result<()> {
    let bind = loaded.effective.server.bind.clone();
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
    let shutdown = {
        let fatal = fatal.clone();
        async move {
            tokio::select! {
                _ = server::shutdown_signal() => {}
                Ok(err) = fatal_rx => {
                    *fatal.lock().expect("fatal lock") = Some(err);
                }
            }
        }
    };

    axum::serve(
        listener,
        server::router(app).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
    .context("server error")?;

    match fatal.lock().expect("fatal lock").take() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}
