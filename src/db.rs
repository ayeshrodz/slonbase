//! PostgreSQL connection pool and server checks.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use serde::Serialize;
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::config::DatabaseConfig;

/// Oldest supported PostgreSQL major version.
pub const MIN_SERVER_VERSION_NUM: i32 = 140_000;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const POOL_TIMEOUT: Duration = Duration::from_secs(10);

fn parse_url(url: &str) -> Result<tokio_postgres::Config> {
    let mut pg: tokio_postgres::Config = url
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid database URL: {e}"))?;
    if pg.get_application_name().is_none() {
        pg.application_name("slonbase");
    }
    if pg.get_connect_timeout().is_none() {
        pg.connect_timeout(CONNECT_TIMEOUT);
    }
    Ok(pg)
}

/// Builds a connection pool. Connections are opened lazily,
/// so call [`check_server`] to verify the database is reachable.
pub fn connect(url: &str, cfg: &DatabaseConfig) -> Result<Pool> {
    let pg = parse_url(url)?;
    let tls = tls_connector(cfg.ca_file.as_deref())?;
    let manager = Manager::from_config(
        pg,
        tls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );

    Pool::builder(manager)
        .max_size(cfg.max_connections)
        .runtime(Runtime::Tokio1)
        .create_timeout(Some(CONNECT_TIMEOUT))
        .wait_timeout(Some(POOL_TIMEOUT))
        .recycle_timeout(Some(POOL_TIMEOUT))
        .build()
        .context("failed to build the connection pool")
}

/// Opens a dedicated (non-pooled) connection that `LISTEN`s on `channel`. Notifications
/// are forwarded to the returned receiver, which closes when the connection is lost.
pub async fn listen(
    url: &str,
    cfg: &DatabaseConfig,
    channel: &str,
) -> Result<(
    tokio_postgres::Client,
    tokio::sync::mpsc::UnboundedReceiver<tokio_postgres::Notification>,
)> {
    let pg = parse_url(url)?;
    let tls = tls_connector(cfg.ca_file.as_deref())?;
    let (client, mut connection) = pg.connect(tls).await?;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            match std::future::poll_fn(|cx| connection.poll_message(cx)).await {
                Some(Ok(tokio_postgres::AsyncMessage::Notification(n))) => {
                    if tx.send(n).is_err() {
                        break;
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(err)) => {
                    tracing::warn!(error = %err, "listener connection lost");
                    break;
                }
                None => break,
            }
        }
    });

    client.batch_execute(&format!("LISTEN {channel}")).await?;
    Ok((client, rx))
}

fn tls_connector(ca_file: Option<&Path>) -> Result<MakeRustlsConnect> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    if let Some(path) = ca_file {
        let certs = CertificateDer::pem_file_iter(path)
            .with_context(|| format!("failed to read CA file {}", path.display()))?;
        for cert in certs {
            let cert = cert.with_context(|| format!("invalid CA file {}", path.display()))?;
            roots.add(cert)?;
        }
    }

    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();

    Ok(MakeRustlsConnect::new(config))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub server_version: String,
    pub database: String,
    pub user: String,
    /// Whether a slonbase installation already exists in this database.
    pub existing_install: bool,
    /// Whether that installation already has at least one superuser.
    pub has_superusers: bool,
}

/// Connects and verifies that the server can host slonbase:
/// a supported version and the privileges needed to create our schema.
pub async fn check_server(pool: &Pool) -> Result<ServerInfo> {
    let client = pool.get().await.map_err(describe_pool_error)?;

    let row = client
        .query_one(
            "SELECT current_setting('server_version_num')::int,
                    current_setting('server_version'),
                    current_database()::text,
                    current_user::text,
                    has_database_privilege(current_database(), 'CREATE'),
                    to_regclass('_slonbase.migrations') IS NOT NULL,
                    EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = '_slonbase')",
            &[],
        )
        .await?;

    let version_num: i32 = row.get(0);
    let server_version: String = row.get(1);
    let can_create: bool = row.get(4);
    let existing_install: bool = row.get(5);
    let schema_exists: bool = row.get(6);

    if version_num < MIN_SERVER_VERSION_NUM {
        anyhow::bail!("PostgreSQL {server_version} is not supported (14 or newer is required)");
    }
    if schema_exists && !existing_install {
        anyhow::bail!(
            "the database already has a \"_slonbase\" schema that was not created by slonbase"
        );
    }
    if !existing_install && !can_create {
        anyhow::bail!(
            "the database user lacks the CREATE privilege on this database \
             (needed once to create the \"_slonbase\" schema)"
        );
    }

    let has_superusers = existing_install
        && client
            .query_one(
                "SELECT to_regclass('_slonbase.superusers') IS NOT NULL
                    AND EXISTS (SELECT 1 FROM _slonbase.superusers)",
                &[],
            )
            .await
            .map(|r| r.get::<_, Option<bool>>(0).unwrap_or(false))
            .unwrap_or(false);

    Ok(ServerInfo {
        server_version,
        database: row.get(2),
        user: row.get(3),
        existing_install,
        has_superusers,
    })
}

/// Turns pool errors into messages a person can act on.
pub fn describe_pool_error(err: deadpool_postgres::PoolError) -> anyhow::Error {
    use deadpool_postgres::PoolError;
    match err {
        PoolError::Backend(e) => anyhow::anyhow!(describe_pg_error(&e)),
        PoolError::Timeout(_) => anyhow::anyhow!("timed out connecting to the database"),
        other => anyhow::anyhow!("database connection failed: {other}"),
    }
}

pub fn describe_pg_error(err: &tokio_postgres::Error) -> String {
    if let Some(db) = err.as_db_error() {
        return format!("database error: {}", db.message());
    }
    // include the source chain (e.g. "connection refused", TLS errors)
    let mut msg = format!("database connection failed: {err}");
    let mut source = std::error::Error::source(err);
    while let Some(s) = source {
        msg.push_str(&format!(": {s}"));
        source = s.source();
    }
    msg
}
