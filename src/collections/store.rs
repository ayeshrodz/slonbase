//! Collection metadata (`_slonbase.collections`) and the in-memory registry.
//!
//! Every change runs the table DDL and the metadata write in one transaction, under the
//! schema advisory lock, so a failed change leaves both untouched.

use std::sync::Arc;

use anyhow::{Context, Result};
use deadpool_postgres::{GenericClient, Pool};

use super::model::{Collection, FieldKind};
use super::schema;
use crate::datetime;
use crate::db::describe_pool_error;
use crate::migrations::SCHEMA_LOCK_KEY;

/// A snapshot of all collections, ordered by creation.
#[derive(Debug, Default, Clone)]
pub struct Registry {
    items: Vec<Arc<Collection>>,
}

impl Registry {
    pub fn new(items: Vec<Collection>) -> Self {
        Self {
            items: items.into_iter().map(Arc::new).collect(),
        }
    }

    pub fn all(&self) -> &[Arc<Collection>] {
        &self.items
    }

    pub fn by_id(&self, id: &str) -> Option<&Arc<Collection>> {
        self.items.iter().find(|c| c.id == id)
    }

    /// Finds a collection by id or (case-insensitive) name.
    pub fn get(&self, id_or_name: &str) -> Option<&Arc<Collection>> {
        self.by_id(id_or_name).or_else(|| {
            self.items
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(id_or_name))
        })
    }

    /// Relation fields (collection, field name) that point to `collection_id`.
    pub fn references_to(&self, collection_id: &str) -> Vec<(Arc<Collection>, String)> {
        let mut refs = Vec::new();
        for c in &self.items {
            for f in &c.fields {
                if let FieldKind::Relation(o) = &f.kind {
                    if o.collection_id == collection_id {
                        refs.push((c.clone(), f.name.clone()));
                    }
                }
            }
        }
        refs
    }
}

pub async fn load(client: &impl GenericClient) -> Result<Registry> {
    let rows = client
        .query(
            &format!(
                "SELECT data, {}, {} FROM _slonbase.collections ORDER BY created, id",
                datetime::sql_format("created"),
                datetime::sql_format("updated")
            ),
            &[],
        )
        .await?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let data: serde_json::Value = row.get(0);
        let mut c: Collection =
            serde_json::from_value(data).context("invalid stored collection")?;
        c.created = row.get(1);
        c.updated = row.get(2);
        items.push(c);
    }
    Ok(Registry::new(items))
}

pub async fn load_from_pool(pool: &Pool) -> Result<Registry> {
    let client = pool.get().await.map_err(describe_pool_error)?;
    load(&client).await
}

fn stored_json(c: &Collection) -> serde_json::Value {
    let mut value = serde_json::to_value(c).expect("collection serializes");
    if let Some(obj) = value.as_object_mut() {
        // kept in their own columns
        obj.remove("created");
        obj.remove("updated");
    }
    value
}

#[derive(Debug)]
pub enum WriteError {
    /// A database constraint rejected the change (e.g. a duplicate name from another instance).
    Conflict(String),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for WriteError {
    fn from(e: anyhow::Error) -> Self {
        WriteError::Other(e)
    }
}

impl From<tokio_postgres::Error> for WriteError {
    fn from(e: tokio_postgres::Error) -> Self {
        if let Some(db) = e.as_db_error() {
            use tokio_postgres::error::SqlState;
            let code = db.code();
            if *code == SqlState::UNIQUE_VIOLATION
                || *code == SqlState::DUPLICATE_TABLE
                || *code == SqlState::DUPLICATE_OBJECT
            {
                return WriteError::Conflict(db.message().to_string());
            }
            if *code == SqlState::NOT_NULL_VIOLATION
                || *code == SqlState::INVALID_TEXT_REPRESENTATION
            {
                return WriteError::Conflict(db.message().to_string());
            }
        }
        WriteError::Other(e.into())
    }
}

/// Stores a new collection's definition (its table must be created in the same transaction).
pub async fn insert_metadata(
    client: &impl GenericClient,
    c: &Collection,
) -> Result<(), tokio_postgres::Error> {
    client
        .execute(
            "INSERT INTO _slonbase.collections (id, name, data) VALUES ($1, $2, $3)",
            &[&c.id, &c.name, &stored_json(c)],
        )
        .await?;
    Ok(())
}

async fn apply_one(
    tx: &deadpool_postgres::Transaction<'_>,
    change: &Change<'_>,
) -> Result<(), WriteError> {
    let statements = match change {
        Change::Create(c) => schema::create(c),
        Change::Update { old, new } => schema::update(old, new),
        Change::Delete(c) => schema::drop(c),
    };
    for statement in &statements {
        tracing::debug!(%statement, "collection DDL");
        tx.batch_execute(statement).await?;
    }

    match change {
        Change::Create(c) => insert_metadata(tx, c).await?,
        Change::Update { new, .. } => {
            tx.execute(
                "UPDATE _slonbase.collections SET name = $2, data = $3, updated = now() WHERE id = $1",
                &[&new.id, &new.name, &stored_json(new)],
            )
            .await?;
        }
        Change::Delete(c) => {
            tx.execute("DELETE FROM _slonbase.collections WHERE id = $1", &[&c.id])
                .await?;
            // OTPs and MFA sessions go with the collection (foreign keys); linked accounts
            // are rows of a collection
            tx.execute(
                &format!(
                    r#"DELETE FROM {} WHERE "collectionRef" = $1"#,
                    super::system::EXTERNAL_AUTHS_TABLE
                ),
                &[&c.id],
            )
            .await?;
        }
    }
    Ok(())
}

pub enum Change<'a> {
    Create(&'a Collection),
    Update {
        old: &'a Collection,
        new: &'a Collection,
    },
    Delete(&'a Collection),
}

/// Applies a validated change and returns the reloaded registry.
pub async fn apply(pool: &Pool, change: Change<'_>) -> Result<Registry, WriteError> {
    apply_all(pool, vec![change]).await
}

/// Applies validated changes in one transaction and returns the reloaded registry.
pub async fn apply_all(pool: &Pool, changes: Vec<Change<'_>>) -> Result<Registry, WriteError> {
    let mut client = pool.get().await.map_err(describe_pool_error)?;
    let tx = client.transaction().await?;
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&SCHEMA_LOCK_KEY])
        .await?;
    for change in &changes {
        apply_one(&tx, change).await?;
    }

    let registry = load(&tx).await?;
    // other instances reload their collections when this commits
    crate::realtime::notify(&tx, "collections").await?;
    tx.commit().await?;
    Ok(registry)
}
