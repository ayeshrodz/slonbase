//! System schema migrations.
//!
//! All pending migrations run in a single transaction guarded by a transaction-level
//! advisory lock, so several instances starting at once apply them exactly once and a
//! failure leaves the database untouched.

use anyhow::{Context, Result};
use deadpool_postgres::Pool;

use crate::db::describe_pool_error;

/// Advisory lock key for schema changes ("slonbase" in ASCII).
pub const SCHEMA_LOCK_KEY: i64 = 0x736c_6f6e_6261_7365;

pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        name: "collections",
        sql: include_str!("../migrations/0002_collections.sql"),
    },
    Migration {
        version: 3,
        name: "events",
        sql: include_str!("../migrations/0003_events.sql"),
    },
    Migration {
        version: 4,
        name: "auth_methods",
        sql: include_str!("../migrations/0004_auth_methods.sql"),
    },
    Migration {
        version: 5,
        name: "logs",
        sql: include_str!("../migrations/0005_logs.sql"),
    },
    Migration {
        version: 6,
        name: "auth_origins",
        sql: include_str!("../migrations/0006_auth_origins.sql"),
    },
];

/// Applies all pending migrations and returns the versions that were applied.
pub async fn run(pool: &Pool) -> Result<Vec<i32>> {
    run_list(pool, MIGRATIONS).await
}

async fn run_list(pool: &Pool, migrations: &[Migration]) -> Result<Vec<i32>> {
    let mut client = pool.get().await.map_err(describe_pool_error)?;
    let tx = client.transaction().await?;

    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&SCHEMA_LOCK_KEY])
        .await?;
    tx.batch_execute(
        "CREATE SCHEMA IF NOT EXISTS _slonbase;
         CREATE TABLE IF NOT EXISTS _slonbase.migrations (
             version    INT PRIMARY KEY,
             name       TEXT NOT NULL,
             applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
         );",
    )
    .await?;

    let applied: Vec<i32> = tx
        .query("SELECT version FROM _slonbase.migrations", &[])
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();

    // refuse to run against a database migrated by a newer slonbase,
    // its schema may be incompatible with this binary
    let latest_known = migrations.iter().map(|m| m.version).max().unwrap_or(0);
    if let Some(newer) = applied.iter().copied().filter(|v| *v > latest_known).max() {
        anyhow::bail!(
            "the database schema (version {newer}) is newer than this slonbase build \
             supports (version {latest_known}); upgrade slonbase"
        );
    }

    let mut newly_applied = Vec::new();
    for m in migrations.iter().filter(|m| !applied.contains(&m.version)) {
        tx.batch_execute(m.sql)
            .await
            .with_context(|| format!("migration {} ({}) failed", m.version, m.name))?;
        tx.execute(
            "INSERT INTO _slonbase.migrations (version, name) VALUES ($1, $2)",
            &[&m.version, &m.name],
        )
        .await?;
        newly_applied.push(m.version);
    }

    tx.commit().await?;
    Ok(newly_applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_unique_and_ascending() {
        let versions: Vec<i32> = MIGRATIONS.iter().map(|m| m.version).collect();
        let mut sorted = versions.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(versions, sorted);
    }
}
