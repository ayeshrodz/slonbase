//! Collection changes inside a caller's transaction, for JavaScript migrations: the same
//! validation as the collections API, without its hooks. The registry passed in is the one
//! the transaction sees, and the one returned includes the change.

use std::sync::Arc;

use deadpool_postgres::Transaction;
use serde_json::{Map, Value};

use super::model::Collection;
use super::store::{self, Change, Registry, WriteError};
use super::validate;
use crate::error::ApiError;

fn write_error(err: WriteError, message: &str) -> ApiError {
    match err {
        WriteError::Conflict(detail) => ApiError::bad_request(format!("{message} {detail}.")),
        WriteError::Other(e) => ApiError::internal(e),
    }
}

fn db_error(e: tokio_postgres::Error) -> ApiError {
    ApiError::internal(e.into())
}

fn parse(body: Map<String, Value>) -> Result<Collection, ApiError> {
    serde_json::from_value(Value::Object(body))
        .map_err(|e| ApiError::bad_request(format!("Invalid collection data: {e}.")))
}

/// Fills a view collection's fields from its query, inspected in a savepoint that's rolled
/// back (so it sees the collections created earlier in the transaction).
async fn infer_view(
    tx: &Transaction<'_>,
    c: &mut Collection,
    old: Option<&Collection>,
    errors_prefix: &str,
    failed: &str,
) -> Result<(), ApiError> {
    if !c.is_view() {
        return Ok(());
    }
    let query = c.view_query.clone().unwrap_or_default();
    let previous = if c.fields.is_empty() {
        old.map(|o| o.fields.clone()).unwrap_or_default()
    } else {
        c.fields.clone()
    };
    tx.batch_execute("SAVEPOINT slonbase_view")
        .await
        .map_err(db_error)?;
    tx.batch_execute(super::view::SEARCH_PATH)
        .await
        .map_err(db_error)?;
    let inferred = super::view::infer_fields(tx, &query, &previous).await;
    tx.batch_execute("ROLLBACK TO SAVEPOINT slonbase_view")
        .await
        .map_err(db_error)?;
    match inferred {
        Ok(fields) => {
            c.fields = fields;
            Ok(())
        }
        Err(message) => {
            let mut errors = crate::validation::Errors::new();
            errors.invalid(format!("{errors_prefix}viewQuery"), message);
            Err(ApiError::invalid_data(failed, &errors))
        }
    }
}

/// For PocketBase's system collections in imports (its exports and snapshots have them):
/// the ones slonbase has as collections take the settings (rules, auth options), not the
/// fields, and the others (`_mfas`, `_otps`, `_authOrigins`, tables of slonbase's own) are
/// skipped. Returns false to skip the definition.
pub fn importable_system(registry: &Registry, map: &mut Map<String, Value>) -> bool {
    if map.get("system").and_then(Value::as_bool) != Some(true) {
        return true;
    }
    let name = map.get("name").and_then(Value::as_str).unwrap_or_default();
    if registry.get(name).is_none_or(|c| !c.system) {
        return false;
    }
    for key in ["id", "fields", "indexes", "type", "system"] {
        map.remove(key);
    }
    true
}

/// What an import changed.
pub struct Imported {
    pub registry: Registry,
    /// The saved collections, in the order given.
    pub saved: Vec<Arc<Collection>>,
    /// The collections deleted (with `delete_missing`), whose files can go once it commits.
    pub deleted: Vec<Arc<Collection>>,
}

/// Creates or updates collections from their JSON definitions (merged over the stored ones
/// they match by id, or by name without an id), and with `delete_missing` deletes the
/// other non-system collections. Tables change first, then views, whose queries can read
/// the tables created by the same import. `single` is for saving one collection (errors
/// aren't under `collections.<i>`).
pub async fn import(
    tx: &Transaction<'_>,
    registry: &Registry,
    items: Vec<Value>,
    delete_missing: bool,
    single: bool,
) -> Result<Imported, ApiError> {
    let failed = if single {
        "Failed to save collection."
    } else {
        "Failed to import collections."
    };
    let prefix = |i: usize| {
        if single {
            String::new()
        } else {
            format!("collections.{i}.")
        }
    };
    let mut pairs: Vec<(Option<Arc<Collection>>, Collection)> = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        let Value::Object(mut map) = item else {
            return Err(ApiError::bad_request(format!(
                "{failed} Collection {i} must be an object."
            )));
        };
        if !importable_system(registry, &mut map) {
            continue;
        }
        let id = map.get("id").and_then(Value::as_str).unwrap_or_default();
        let name = map.get("name").and_then(Value::as_str).unwrap_or_default();
        let old = registry
            .by_id(id)
            .or_else(|| {
                registry
                    .get(name)
                    .filter(|_| id.is_empty() || registry.by_id(id).is_none())
            })
            .cloned();
        let mut merged = match &old {
            Some(old) => match serde_json::to_value(old.as_ref()).expect("serializes") {
                Value::Object(m) => m,
                _ => unreachable!(),
            },
            None => Map::new(),
        };
        merged.extend(map);
        let mut c = parse(merged).map_err(|e| {
            ApiError::bad_request(format!("{failed} Collection {i}: {}", e.message))
        })?;
        match &old {
            Some(old) => c.id = old.id.clone(),
            None if c.id.is_empty() => {
                c.id = format!("pbc_{}", crate::ids::random_id(10));
            }
            None => {}
        }
        pairs.push((old, c));
    }

    let imported_ids: Vec<String> = pairs.iter().map(|(_, c)| c.id.clone()).collect();
    let deleted: Vec<Arc<Collection>> = if delete_missing {
        registry
            .all()
            .iter()
            .filter(|c| !c.system && !imported_ids.contains(&c.id))
            .cloned()
            .collect()
    } else {
        vec![]
    };
    // each collection is validated against all the others as they'll be
    let validate_all = |pairs: &mut [(Option<Arc<Collection>>, Collection)],
                        current: &Registry,
                        which: &dyn Fn(&Collection) -> bool|
     -> crate::validation::Errors {
        let mut target: Vec<Collection> = current
            .all()
            .iter()
            .filter(|c| !deleted.iter().any(|d| d.id == c.id))
            .filter(|c| !imported_ids.contains(&c.id))
            .map(|c| c.as_ref().clone())
            .collect();
        target.extend(pairs.iter().map(|(_, c)| c.clone()));
        let mut errors = crate::validation::Errors::new();
        for (i, (old, c)) in pairs.iter_mut().enumerate() {
            if !which(c) {
                continue;
            }
            let others = Registry::new(target.iter().filter(|o| o.id != c.id).cloned().collect());
            let found = validate::prepare(c, old.as_deref(), &others);
            if single {
                errors.extend(found);
            } else {
                errors.extend_prefixed(&format!("collections.{i}"), found);
            }
        }
        errors
    };
    let changes_of = |pairs: &[(Option<Arc<Collection>>, Collection)], views: bool| {
        pairs
            .iter()
            .filter(|(_, c)| c.is_view() == views)
            .filter_map(|(old, new)| match old {
                Some(old) if old.as_ref() == new => None,
                Some(old) => Some((Some(old.clone()), new.clone())),
                None => Some((None, new.clone())),
            })
            .collect::<Vec<_>>()
    };
    let apply = async |changes: Vec<Change<'_>>| -> Result<Registry, ApiError> {
        if changes.is_empty() {
            store::load(tx).await.map_err(ApiError::internal)
        } else {
            store::apply_in(tx, changes)
                .await
                .map_err(|e| write_error(e, failed))
        }
    };

    // the tables (and the deletions)
    let errors = validate_all(&mut pairs, registry, &|c| !c.is_view());
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(failed, &errors));
    }
    let tables = changes_of(&pairs, false);
    let mut changes: Vec<Change> = deleted.iter().map(|c| Change::Delete(c)).collect();
    changes.extend(tables.iter().map(|(old, new)| match old {
        Some(old) => Change::Update { old, new },
        None => Change::Create(new),
    }));
    let mut current = apply(changes).await?;

    // then the views, inferred from their queries
    if pairs.iter().any(|(_, c)| c.is_view()) {
        for (i, (old, c)) in pairs.iter_mut().enumerate() {
            infer_view(tx, c, old.as_deref(), &prefix(i), failed).await?;
        }
        let errors = validate_all(&mut pairs, &current, &|c| c.is_view());
        if !errors.is_empty() {
            return Err(ApiError::invalid_data(failed, &errors));
        }
        let views = changes_of(&pairs, true);
        let changes: Vec<Change> = views
            .iter()
            .map(|(old, new)| match old {
                Some(old) => Change::Update { old, new },
                None => Change::Create(new),
            })
            .collect();
        current = apply(changes).await?;
    }

    let saved = pairs
        .iter()
        .filter_map(|(_, c)| current.by_id(&c.id).cloned())
        .collect();
    Ok(Imported {
        registry: current,
        saved,
        deleted,
    })
}

/// Deletes a collection, unless it's a system collection or other collections refer to it.
pub async fn delete(
    tx: &Transaction<'_>,
    registry: &Registry,
    id_or_name: &str,
) -> Result<(Registry, Arc<Collection>), ApiError> {
    let failed = "Failed to delete collection.";
    let c = registry.get(id_or_name).cloned().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::NOT_FOUND,
            format!("Collection {id_or_name:?} not found."),
        )
    })?;
    if c.system {
        return Err(ApiError::bad_request(
            "System collections can't be deleted.",
        ));
    }
    let referencing: Vec<String> = registry
        .references_to(&c.id)
        .into_iter()
        .filter(|(other, _)| other.id != c.id)
        .map(|(other, field)| format!("{}.{field}", other.name))
        .collect();
    if !referencing.is_empty() {
        return Err(ApiError::bad_request(format!(
            "The collection is referenced by other collections ({}); remove those relation fields first.",
            referencing.join(", ")
        )));
    }
    let registry = store::apply_in(tx, vec![Change::Delete(&c)])
        .await
        .map_err(|e| write_error(e, failed))?;
    Ok((registry, c))
}
