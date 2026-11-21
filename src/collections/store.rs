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
use crate::secrets::SecretsKey;

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

/// Loads every collection, decrypting their OAuth2 client secrets with `key` (see
/// [`crate::secrets`]).
pub async fn load_with(client: &impl GenericClient, key: Option<&SecretsKey>) -> Result<Registry> {
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
        let mut data: serde_json::Value = row.get(0);
        crate::secrets::open_collection(&mut data, key)?;
        let mut c: Collection =
            serde_json::from_value(data).context("invalid stored collection")?;
        c.created = row.get(1);
        c.updated = row.get(2);
        items.push(c);
    }
    Ok(Registry::new(items))
}

pub async fn load_from_pool_with(pool: &Pool, key: Option<&SecretsKey>) -> Result<Registry> {
    let client = pool.get().await.map_err(describe_pool_error)?;
    load_with(&client, key).await
}

/// The stored form of a collection, its OAuth2 client secrets encrypted with `key`.
fn stored_json(c: &Collection, key: Option<&SecretsKey>) -> serde_json::Value {
    let mut value = serde_json::to_value(c).expect("collection serializes");
    crate::secrets::seal_collection(&mut value, key);
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
                || *code == SqlState::CANNOT_COERCE
                || *code == SqlState::DATATYPE_MISMATCH
                || *code == SqlState::INVALID_DATETIME_FORMAT
                || *code == SqlState::FEATURE_NOT_SUPPORTED
            {
                return WriteError::Conflict(db.message().to_string());
            }
            // e.g. dropping a column or table a view collection uses
            if *code == SqlState::DEPENDENT_OBJECTS_STILL_EXIST {
                let detail = db.detail().map(|d| format!(" ({d})")).unwrap_or_default();
                return WriteError::Conflict(format!(
                    "{}{detail}; change or delete the view collections that use it first",
                    db.message()
                ));
            }
        }
        WriteError::Other(e.into())
    }
}

/// Recreates a view collection from its query, updating its fields when the columns it
/// returns changed.
async fn rebuild_view(
    tx: &deadpool_postgres::Transaction<'_>,
    view: &mut Collection,
    key: Option<&SecretsKey>,
) -> Result<(), WriteError> {
    tx.batch_execute(super::view::SEARCH_PATH).await?;
    let query = view.view_query.clone().unwrap_or_default();
    let fields = super::view::infer_fields(tx, &query, &view.fields)
        .await
        .map_err(|message| {
            WriteError::Conflict(format!(
                "it would break the view collection {:?} ({message}); change its query first",
                view.name
            ))
        })?;
    let changed = fields != view.fields;
    view.fields = fields;
    for statement in schema::create(view) {
        tx.batch_execute(&statement).await?;
    }
    if changed {
        tx.execute(
            "UPDATE _slonbase.collections SET data = $2, updated = now() WHERE id = $1",
            &[&view.id, &stored_json(view, key)],
        )
        .await?;
    }
    Ok(())
}

/// Recreates view collections whose PostgreSQL view is missing (e.g. dropped by hand).
/// Problems are logged; the collection then fails on reads until its query is fixed.
pub async fn heal_views_with(pool: &Pool, key: Option<&SecretsKey>) -> Result<()> {
    let mut client = pool.get().await.map_err(describe_pool_error)?;
    let registry = load_with(&client, key).await?;
    for view in registry.all().iter().filter(|c| c.is_view()) {
        let exists: bool = client
            .query_one(
                "SELECT to_regclass($1) IS NOT NULL",
                &[&schema::table_ref(&view.name)],
            )
            .await?
            .get(0);
        if exists {
            continue;
        }
        let tx = client.transaction().await?;
        tx.execute("SELECT pg_advisory_xact_lock($1)", &[&SCHEMA_LOCK_KEY])
            .await?;
        let mut view = view.as_ref().clone();
        match rebuild_view(&tx, &mut view, key).await {
            Ok(()) => {
                tx.commit().await?;
                tracing::warn!(collection = %view.name, "recreated a missing view");
            }
            Err(err) => {
                drop(tx);
                tracing::error!(collection = %view.name, error = ?err, "failed to recreate a missing view");
            }
        }
    }
    Ok(())
}

/// Stores a new collection's definition (its table must be created in the same transaction).
pub async fn insert_metadata_with(
    client: &impl GenericClient,
    c: &Collection,
    key: Option<&SecretsKey>,
) -> Result<(), tokio_postgres::Error> {
    client
        .execute(
            "INSERT INTO _slonbase.collections (id, name, data) VALUES ($1, $2, $3)",
            &[&c.id, &c.name, &stored_json(c, key)],
        )
        .await?;
    Ok(())
}

async fn apply_one(
    tx: &deadpool_postgres::Transaction<'_>,
    change: &Change<'_>,
    key: Option<&SecretsKey>,
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
        Change::Create(c) => insert_metadata_with(tx, c, key).await?,
        Change::Update { new, .. } => {
            tx.execute(
                "UPDATE _slonbase.collections SET name = $2, data = $3, updated = now() WHERE id = $1",
                &[&new.id, &new.name, &stored_json(new, key)],
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

/// A stored file of a file field that a change removed, to delete once the change commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedFile {
    pub collection_id: String,
    pub record_id: String,
    pub name: String,
}

/// What applying changes did.
pub struct Applied {
    /// The registry as it is after the changes.
    pub registry: Registry,
    /// The files of the file fields the changes removed (their columns are gone).
    pub removed_files: Vec<RemovedFile>,
    /// The views (ids) the caller said it would make again, that couldn't be rebuilt from their
    /// old queries: they are missing until it does.
    pub unbuilt_views: Vec<String>,
}

/// Applies a validated change and returns the reloaded registry.
pub async fn apply_with(
    pool: &Pool,
    change: Change<'_>,
    key: Option<&SecretsKey>,
) -> Result<Applied, WriteError> {
    apply_all_with(pool, vec![change], key).await
}

/// Applies validated changes in one transaction and returns the reloaded registry.
pub async fn apply_all_with(
    pool: &Pool,
    changes: Vec<Change<'_>>,
    key: Option<&SecretsKey>,
) -> Result<Applied, WriteError> {
    let mut client = pool.get().await.map_err(describe_pool_error)?;
    let tx = client.transaction().await?;
    let applied = apply_in_with(&tx, changes, key).await?;
    tx.commit().await?;
    Ok(applied)
}

/// Applies validated changes in the caller's transaction and returns the registry as it is
/// in it. Other instances reload their collections when the transaction commits.
pub async fn apply_in_with(
    tx: &deadpool_postgres::Transaction<'_>,
    changes: Vec<Change<'_>>,
    key: Option<&SecretsKey>,
) -> Result<Applied, WriteError> {
    apply_in_with_views_pending(tx, changes, key, &[]).await
}

/// [`apply_in_with`] for a change whose views (by id) are applied by the caller next: they are
/// rebuilt from the queries they have, and when the change made one of those wrong (a column a
/// view reads is renamed, and the view's query is updated to the new name in the same import)
/// the view is left out, and reported in [`Applied::unbuilt_views`].
pub async fn apply_in_with_views_pending(
    tx: &deadpool_postgres::Transaction<'_>,
    mut changes: Vec<Change<'_>>,
    key: Option<&SecretsKey>,
    pending_views: &[String],
) -> Result<Applied, WriteError> {
    // views go after the tables they read, and are dropped before them
    changes.sort_by_key(|change| match change {
        Change::Delete(c) if c.is_view() => 0,
        Change::Delete(_) => 1,
        Change::Create(c) | Change::Update { new: c, .. } if !c.is_view() => 2,
        _ => 3,
    });
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&SCHEMA_LOCK_KEY])
        .await?;

    // the views the change deletes go first: the views it keeps may read them (a kept view
    // can't be dropped, to be made again, while one that reads it is still there)
    let mut view_deletes = Vec::new();
    changes.retain(|change| match change {
        Change::Delete(c) if c.is_view() => {
            view_deletes.push(*c);
            false
        }
        _ => true,
    });
    for view in view_deletes {
        apply_one(tx, &Change::Delete(view), key).await?;
    }

    // Table changes run with the other view collections dropped, so PostgreSQL's view
    // dependencies never block them; the views are then recreated from their queries
    // (and their fields re-inferred). A view the change breaks fails the whole change.
    let touches_tables = changes.iter().any(|change| match change {
        Change::Create(c) | Change::Delete(c) => !c.is_view(),
        Change::Update { new, .. } => !new.is_view(),
    });
    let mut rebuild: Vec<(Collection, bool)> = Vec::new();
    let mut unbuilt_views: Vec<String> = Vec::new();
    // a view that is replaced can't be dropped while another reads it: the views that read it
    // (and those that read them) are dropped meanwhile and made again from their queries
    let changed_views: Vec<&Collection> = changes
        .iter()
        .filter_map(|change| match change {
            Change::Update { old, new } if new.is_view() && *old != *new => Some(*new),
            _ => None,
        })
        .collect();
    if !touches_tables && !changed_views.is_empty() {
        let changed: Vec<&str> = changes
            .iter()
            .map(|change| match change {
                Change::Create(c) | Change::Delete(c) => c.id.as_str(),
                Change::Update { new, .. } => new.id.as_str(),
            })
            .collect();
        let all = load_with(tx, key).await?;
        let mut names: Vec<String> = changed_views
            .iter()
            .map(|v| v.name.to_lowercase())
            .collect();
        let mut readers: Vec<Arc<Collection>> = Vec::new();
        loop {
            let before = readers.len();
            for view in all.all().iter().filter(|c| c.is_view()) {
                if changed.contains(&view.id.as_str()) || readers.iter().any(|r| r.id == view.id) {
                    continue;
                }
                let query = view.view_query.clone().unwrap_or_default().to_lowercase();
                if names.iter().any(|n| super::tx::mentions(&query, n)) {
                    names.push(view.name.to_lowercase());
                    readers.push(view.clone());
                }
            }
            if readers.len() == before {
                break;
            }
        }
        // (dropped after the ones that read them, made again before: the loops below order them)
        for view in &readers {
            rebuild.push((view.as_ref().clone(), pending_views.contains(&view.id)));
        }
        let mut to_drop = readers;
        while !to_drop.is_empty() {
            let before = to_drop.len();
            let mut left = Vec::new();
            let mut first_error = None;
            for view in to_drop {
                tx.batch_execute("SAVEPOINT drop_view").await?;
                let mut dropped = Ok(());
                for statement in schema::drop(&view) {
                    if let Err(e) = tx.batch_execute(&statement).await {
                        dropped = Err(e);
                        break;
                    }
                }
                match dropped {
                    Ok(()) => tx.batch_execute("RELEASE SAVEPOINT drop_view").await?,
                    Err(e) => {
                        tx.batch_execute("ROLLBACK TO SAVEPOINT drop_view").await?;
                        tx.batch_execute("RELEASE SAVEPOINT drop_view").await?;
                        first_error.get_or_insert(e);
                        left.push(view);
                    }
                }
            }
            if left.len() == before {
                return Err(first_error.expect("a view left has an error").into());
            }
            to_drop = left;
        }
    }
    if touches_tables {
        let changed: Vec<&str> = changes
            .iter()
            .map(|change| match change {
                Change::Create(c) | Change::Delete(c) => c.id.as_str(),
                Change::Update { new, .. } => new.id.as_str(),
            })
            .collect();
        let mut to_drop: Vec<Arc<Collection>> = load_with(tx, key)
            .await?
            .all()
            .iter()
            .filter(|c| c.is_view() && !changed.contains(&c.id.as_str()))
            .cloned()
            .collect();
        for view in &to_drop {
            rebuild.push((view.as_ref().clone(), pending_views.contains(&view.id)));
        }
        // a view that others read can only be dropped after them, and the collections come in
        // no such order: the ones PostgreSQL refuses wait until the others are gone
        while !to_drop.is_empty() {
            let before = to_drop.len();
            let mut left = Vec::new();
            let mut first_error = None;
            for view in to_drop {
                tx.batch_execute("SAVEPOINT drop_view").await?;
                let mut dropped = Ok(());
                for statement in schema::drop(&view) {
                    if let Err(e) = tx.batch_execute(&statement).await {
                        dropped = Err(e);
                        break;
                    }
                }
                match dropped {
                    Ok(()) => tx.batch_execute("RELEASE SAVEPOINT drop_view").await?,
                    Err(e) => {
                        tx.batch_execute("ROLLBACK TO SAVEPOINT drop_view").await?;
                        tx.batch_execute("RELEASE SAVEPOINT drop_view").await?;
                        first_error.get_or_insert(e);
                        left.push(view);
                    }
                }
            }
            if left.len() == before {
                return Err(first_error.expect("a view left has an error").into());
            }
            to_drop = left;
        }
    }

    let mut removed_files = Vec::new();
    for change in &changes {
        if let Change::Update { old, new } = change {
            removed_files.extend(files_of_removed_fields(tx, old, new).await?);
        }
        apply_one(tx, change, key).await?;
    }
    // the views are made again the same way: one that reads another waits until it is there
    // (a view of the caller's that its old query no longer allows is left out, see above)
    while !rebuild.is_empty() {
        let before = rebuild.len();
        let mut left = Vec::new();
        for (mut view, pending) in rebuild {
            tx.batch_execute("SAVEPOINT rebuild_view").await?;
            match rebuild_view(tx, &mut view, key).await {
                Ok(()) => tx.batch_execute("RELEASE SAVEPOINT rebuild_view").await?,
                Err(WriteError::Conflict(message)) => {
                    tx.batch_execute("ROLLBACK TO SAVEPOINT rebuild_view")
                        .await?;
                    tx.batch_execute("RELEASE SAVEPOINT rebuild_view").await?;
                    left.push((view, pending, message));
                }
                Err(other) => return Err(other),
            }
        }
        if left.len() == before {
            // none could be made: what is left is broken
            for (view, pending, message) in left {
                if pending {
                    unbuilt_views.push(view.id.clone());
                } else {
                    return Err(WriteError::Conflict(message));
                }
            }
            break;
        }
        rebuild = left.into_iter().map(|(v, p, _)| (v, p)).collect();
    }

    let registry = load_with(tx, key).await?;
    // other instances reload their collections when this commits
    crate::realtime::notify(tx, "collections").await?;
    Ok(Applied {
        registry,
        removed_files,
        unbuilt_views,
    })
}

/// The stored files of the file fields of `old` that `new` no longer has, read before their
/// columns are dropped.
async fn files_of_removed_fields(
    tx: &deadpool_postgres::Transaction<'_>,
    old: &Collection,
    new: &Collection,
) -> Result<Vec<RemovedFile>, WriteError> {
    let mut files = Vec::new();
    if old.is_view() {
        return Ok(files);
    }
    let table = schema::table_ref(&old.name);
    for field in &old.fields {
        if !matches!(field.kind, FieldKind::File(_)) || new.fields.iter().any(|f| f.id == field.id)
        {
            continue;
        }
        let column = crate::filter::quote_ident(&field.name);
        let sql = if field.is_multi() {
            format!("SELECT t.id, v FROM {table} t, jsonb_array_elements_text(t.{column}) v")
        } else {
            format!("SELECT id, {column} FROM {table} WHERE {column} <> ''")
        };
        for row in tx.query(&sql, &[]).await? {
            files.push(RemovedFile {
                collection_id: old.id.clone(),
                record_id: row.get(0),
                name: row.get(1),
            });
        }
    }
    Ok(files)
}

// The signatures before encryption at rest (1.3), kept for programs using the library: they
// read and write without an encryption key (an installation using one needs the `_with` forms).

/// [`load_with`] without an encryption key.
pub async fn load(client: &impl GenericClient) -> Result<Registry> {
    load_with(client, None).await
}

/// [`load_from_pool_with`] without an encryption key.
pub async fn load_from_pool(pool: &Pool) -> Result<Registry> {
    load_from_pool_with(pool, None).await
}

/// [`insert_metadata_with`] without an encryption key.
pub async fn insert_metadata(
    client: &impl GenericClient,
    c: &Collection,
) -> Result<(), tokio_postgres::Error> {
    insert_metadata_with(client, c, None).await
}

/// [`apply_with`] without an encryption key.
pub async fn apply(pool: &Pool, change: Change<'_>) -> Result<Applied, WriteError> {
    apply_with(pool, change, None).await
}

/// [`apply_all_with`] without an encryption key.
pub async fn apply_all(pool: &Pool, changes: Vec<Change<'_>>) -> Result<Applied, WriteError> {
    apply_all_with(pool, changes, None).await
}

/// [`apply_in_with`] without an encryption key.
pub async fn apply_in(
    tx: &deadpool_postgres::Transaction<'_>,
    changes: Vec<Change<'_>>,
) -> Result<Applied, WriteError> {
    apply_in_with(tx, changes, None).await
}

/// [`heal_views_with`] without an encryption key.
pub async fn heal_views(pool: &Pool) -> Result<()> {
    heal_views_with(pool, None).await
}
