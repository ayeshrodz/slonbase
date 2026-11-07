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

fn parse_url(url: &str, cfg: &DatabaseConfig) -> Result<tokio_postgres::Config> {
    let mut pg: tokio_postgres::Config = url
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid database URL: {e}"))?;
    if let Some(dbname) = &cfg.dbname {
        pg.dbname(dbname);
    }
    if let Some((user, password)) = &cfg.user {
        pg.user(user);
        pg.password(password);
    }
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
    let mut pg = parse_url(url, cfg)?;
    // the realtime triggers don't NOTIFY for slonbase's own writes: it announces them after
    // they commit (see `realtime::written`), so writes don't queue on PostgreSQL's notify lock
    let options = match pg.get_options() {
        Some(own) if !own.is_empty() => format!("{own} -c slonbase.notify=off"),
        _ => "-c slonbase.notify=off".to_string(),
    };
    pg.options(&options);
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
    // a pooler in transaction mode can't carry LISTEN: the listener may have a direct URL
    let pg = parse_url(cfg.listen_url.as_deref().unwrap_or(url), cfg)?;
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

    // the `_superusers` collection, or the table older versions used
    let mut has_superusers = false;
    if existing_install {
        for table in [r#"slonbase."_superusers""#, "_slonbase.superusers"] {
            let exists = client
                .query_one("SELECT to_regclass($1) IS NOT NULL", &[&table])
                .await
                .is_ok_and(|r| r.get::<_, bool>(0));
            if exists {
                has_superusers = client
                    .query_one(&format!("SELECT EXISTS (SELECT 1 FROM {table})"), &[])
                    .await
                    .is_ok_and(|r| r.get::<_, bool>(0));
                break;
            }
        }
    }

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

/// Statements kept prepared on one connection, at most; past it, that connection's are
/// dropped and prepared again as they come.
const MAX_STATEMENTS: usize = 256;
/// SQL texts remembered for [`seen_before`]; past it, they're forgotten.
const MAX_SEEN: usize = 8192;

/// Whether this SQL text ran recently. Only SQL seen before is prepared and kept, so a query
/// that comes once (a filter typed once, or many sent to fill the caches) runs as before, and
/// the cache holds what repeats: the queries of the app's screens.
fn seen_before(sql: &str) -> bool {
    use std::hash::BuildHasher;
    static STATE: std::sync::LazyLock<std::hash::RandomState> =
        std::sync::LazyLock::new(std::hash::RandomState::new);
    static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<u64>>> =
        std::sync::LazyLock::new(Default::default);
    let hash = STATE.hash_one(sql);
    let mut seen = SEEN.lock().expect("seen SQL lock");
    if seen.contains(&hash) {
        return true;
    }
    if seen.len() >= MAX_SEEN {
        seen.clear();
    }
    seen.insert(hash);
    false
}

/// A prepared statement whose result columns changed under it (a table altered by another
/// program); it's dropped from the cache.
fn is_stale_plan(err: &tokio_postgres::Error) -> bool {
    err.code() == Some(&tokio_postgres::error::SqlState::FEATURE_NOT_SUPPORTED)
        && err
            .as_db_error()
            .is_some_and(|db| db.message().contains("cached plan"))
}

/// Runs queries whose parameters are all text (the filter compiler's [`Params`]). SQL that
/// repeats runs as a statement prepared once per connection (PostgreSQL parses and plans it
/// once: a lookup by id costs about a third), the rest in one round trip with the parameter
/// types sent along, unprepared. Statements are bounded per connection ([`MAX_STATEMENTS`])
/// and dropped when collections change.
///
/// [`Params`]: crate::filter::Params
pub trait TextQuery: Sync {
    type Inner: tokio_postgres::GenericClient + Sync;

    fn inner(&self) -> &Self::Inner;

    fn statement_cache(&self) -> &deadpool_postgres::StatementCache;

    /// Whether a failed statement aborts a transaction (no retry then).
    fn in_transaction(&self) -> bool;

    fn prepare_text(
        &self,
        sql: &str,
        count: usize,
    ) -> impl std::future::Future<Output = Result<tokio_postgres::Statement, tokio_postgres::Error>> + Send;

    /// The statement for `sql` when it repeats (see [`seen_before`]).
    fn prepared(
        &self,
        sql: &str,
        count: usize,
    ) -> impl std::future::Future<
        Output = Result<Option<tokio_postgres::Statement>, tokio_postgres::Error>,
    > + Send {
        async move {
            if !seen_before(sql) {
                return Ok(None);
            }
            let cache = self.statement_cache();
            if cache.size() >= MAX_STATEMENTS {
                cache.clear();
            }
            self.prepare_text(sql, count).await.map(Some)
        }
    }

    fn query_text(
        &self,
        sql: &str,
        params: &crate::filter::Params,
    ) -> impl std::future::Future<Output = Result<Vec<tokio_postgres::Row>, tokio_postgres::Error>> + Send
    {
        let typed = params.typed();
        async move {
            use tokio_postgres::GenericClient as _;
            if let Some(statement) = self.prepared(sql, typed.len()).await? {
                let values: Vec<_> = typed.iter().map(|(v, _)| *v).collect();
                match self.inner().query(&statement, &values).await {
                    Err(err) if is_stale_plan(&err) => {
                        self.statement_cache()
                            .remove(sql, &vec![tokio_postgres::types::Type::TEXT; typed.len()]);
                        if self.in_transaction() {
                            return Err(err);
                        }
                    }
                    result => return result,
                }
            }
            self.inner().query_typed(sql, &typed).await
        }
    }

    fn query_text_one(
        &self,
        sql: &str,
        params: &crate::filter::Params,
    ) -> impl std::future::Future<Output = Result<tokio_postgres::Row, tokio_postgres::Error>> + Send
    {
        let typed = params.typed();
        async move {
            use tokio_postgres::GenericClient as _;
            if let Some(statement) = self.prepared(sql, typed.len()).await? {
                let values: Vec<_> = typed.iter().map(|(v, _)| *v).collect();
                match self.inner().query_one(&statement, &values).await {
                    Err(err) if is_stale_plan(&err) => {
                        self.statement_cache()
                            .remove(sql, &vec![tokio_postgres::types::Type::TEXT; typed.len()]);
                        if self.in_transaction() {
                            return Err(err);
                        }
                    }
                    result => return result,
                }
            }
            self.inner().query_typed_one(sql, &typed).await
        }
    }

    fn query_text_opt(
        &self,
        sql: &str,
        params: &crate::filter::Params,
    ) -> impl std::future::Future<
        Output = Result<Option<tokio_postgres::Row>, tokio_postgres::Error>,
    > + Send {
        let typed = params.typed();
        async move {
            use tokio_postgres::GenericClient as _;
            if let Some(statement) = self.prepared(sql, typed.len()).await? {
                let values: Vec<_> = typed.iter().map(|(v, _)| *v).collect();
                match self.inner().query_opt(&statement, &values).await {
                    Err(err) if is_stale_plan(&err) => {
                        self.statement_cache()
                            .remove(sql, &vec![tokio_postgres::types::Type::TEXT; typed.len()]);
                        if self.in_transaction() {
                            return Err(err);
                        }
                    }
                    result => return result,
                }
            }
            self.inner().query_typed_opt(sql, &typed).await
        }
    }
}

impl TextQuery for deadpool_postgres::Client {
    type Inner = tokio_postgres::Client;
    fn inner(&self) -> &Self::Inner {
        self
    }
    fn statement_cache(&self) -> &deadpool_postgres::StatementCache {
        &self.statement_cache
    }
    fn in_transaction(&self) -> bool {
        false
    }
    fn prepare_text(
        &self,
        sql: &str,
        count: usize,
    ) -> impl std::future::Future<Output = Result<tokio_postgres::Statement, tokio_postgres::Error>> + Send
    {
        let types = vec![tokio_postgres::types::Type::TEXT; count];
        async move { self.prepare_typed_cached(sql, &types).await }
    }
}

impl<'a> TextQuery for deadpool_postgres::Transaction<'a> {
    type Inner = tokio_postgres::Transaction<'a>;
    fn inner(&self) -> &Self::Inner {
        self
    }
    fn statement_cache(&self) -> &deadpool_postgres::StatementCache {
        &self.statement_cache
    }
    fn in_transaction(&self) -> bool {
        true
    }
    fn prepare_text(
        &self,
        sql: &str,
        count: usize,
    ) -> impl std::future::Future<Output = Result<tokio_postgres::Statement, tokio_postgres::Error>> + Send
    {
        let types = vec![tokio_postgres::types::Type::TEXT; count];
        async move { self.prepare_typed_cached(sql, &types).await }
    }
}
