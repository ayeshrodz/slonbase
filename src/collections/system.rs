//! System collections, as in PocketBase: `_superusers` (an auth collection) and
//! `_externalAuths` (accounts of OAuth2 providers linked to records).
//!
//! They're created after the SQL migrations, once per installation. Installations from
//! before they existed kept superusers and linked accounts in private tables, which are
//! moved into the collections (in the same transaction) and dropped.

use anyhow::{Context, Result};
use deadpool_postgres::Pool;
use serde_json::json;

use super::model::{
    Collection, FIELD_PASSWORD, FieldKind, SUPERUSERS_COLLECTION_ID, SUPERUSERS_COLLECTION_NAME,
};
use super::{schema, store, validate};
use crate::db::describe_pool_error;
use crate::migrations::SCHEMA_LOCK_KEY;

pub const EXTERNAL_AUTHS_COLLECTION_ID: &str = "pbc_2281828961";
pub const EXTERNAL_AUTHS_COLLECTION_NAME: &str = "_externalAuths";
pub const EXTERNAL_AUTHS_TABLE: &str = r#"slonbase."_externalAuths""#;

/// Superuser tokens are valid for 1 day (PocketBase's default).
const SUPERUSER_TOKEN_DURATION: i64 = 86_400;
const SUPERUSER_PASSWORD_MIN: usize = 10;
const SUPERUSER_PASSWORD_COST: u32 = 12;

fn autodate(name: &str, on_update: bool) -> serde_json::Value {
    json!({ "name": name, "type": "autodate", "system": true, "onCreate": true, "onUpdate": on_update })
}

fn superusers() -> Collection {
    let mut c: Collection = serde_json::from_value(json!({
        "id": SUPERUSERS_COLLECTION_ID,
        "name": SUPERUSERS_COLLECTION_NAME,
        "type": "auth",
        "fields": [autodate("created", false), autodate("updated", true)],
        "listRule": null, "viewRule": null, "createRule": null,
        "updateRule": null, "deleteRule": null,
        "authRule": "", "manageRule": null,
        "passwordAuth": { "enabled": true, "identityFields": ["email"] },
        "authToken": { "duration": SUPERUSER_TOKEN_DURATION },
    }))
    .expect("valid superusers definition");
    c.system = true;
    c
}

fn external_auths() -> Collection {
    let text =
        |name: &str| json!({ "name": name, "type": "text", "required": true, "system": true });
    let own = "@request.auth.id != '' && recordRef = @request.auth.id && collectionRef = @request.auth.collectionId";
    serde_json::from_value(json!({
        "id": EXTERNAL_AUTHS_COLLECTION_ID,
        "name": EXTERNAL_AUTHS_COLLECTION_NAME,
        "type": "base",
        "fields": [
            text("collectionRef"), text("recordRef"), text("provider"), text("providerId"),
            autodate("created", false), autodate("updated", true),
        ],
        "indexes": [
            "CREATE UNIQUE INDEX idx_externalAuths_record_provider ON _externalAuths (collectionRef, recordRef, provider)",
            "CREATE UNIQUE INDEX idx_externalAuths_collection_provider ON _externalAuths (collectionRef, provider, providerId)",
        ],
        "listRule": own, "viewRule": own, "createRule": null, "updateRule": null, "deleteRule": own,
    }))
    .expect("valid external auths definition")
}

/// The legacy table of a system collection and the statement moving its rows.
fn legacy(id: &str) -> Option<(&'static str, &'static str)> {
    match id {
        SUPERUSERS_COLLECTION_ID => Some((
            "superusers",
            // new token keys: the old ones don't fit the field (this signs superusers out once)
            r#"INSERT INTO slonbase."_superusers"
                   (id, email, password, "tokenKey", "emailVisibility", verified, created, updated)
               SELECT id, email, password_hash,
                      substr(md5(random()::text) || md5(random()::text), 1, 50),
                      FALSE, TRUE, created, updated
               FROM _slonbase.superusers"#,
        )),
        EXTERNAL_AUTHS_COLLECTION_ID => Some((
            "external_auths",
            r#"INSERT INTO slonbase."_externalAuths"
                   (id, "collectionRef", "recordRef", provider, "providerId", created, updated)
               SELECT id, collection_id, record_id, provider, provider_id, created, updated
               FROM _slonbase.external_auths"#,
        )),
        _ => None,
    }
}

/// Creates the missing system collections (moving rows from legacy tables).
pub async fn ensure(pool: &Pool) -> Result<()> {
    let mut client = pool.get().await.map_err(describe_pool_error)?;
    let tx = client.transaction().await?;
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&SCHEMA_LOCK_KEY])
        .await?;
    let mut registry = store::load(&tx).await?;

    let mut created = Vec::new();
    for mut c in [superusers(), external_auths()] {
        if registry.by_id(&c.id).is_some() {
            continue;
        }
        let errors = validate::prepare_system(&mut c, &registry);
        anyhow::ensure!(
            errors.is_empty(),
            "invalid system collection {}: {errors:?}",
            c.name
        );
        if c.id == SUPERUSERS_COLLECTION_ID {
            if let Some(FieldKind::Password(o)) = c
                .fields
                .iter_mut()
                .find(|f| f.name == FIELD_PASSWORD)
                .map(|f| &mut f.kind)
            {
                o.min = SUPERUSER_PASSWORD_MIN;
                o.cost = SUPERUSER_PASSWORD_COST;
            }
        }
        for statement in schema::create(&c) {
            tx.batch_execute(&statement)
                .await
                .with_context(|| format!("failed to create {}", c.name))?;
        }
        store::insert_metadata(&tx, &c).await?;

        if let Some((table, copy)) = legacy(&c.id) {
            let exists: bool = tx
                .query_one(
                    "SELECT to_regclass($1) IS NOT NULL",
                    &[&format!("_slonbase.{table}")],
                )
                .await?
                .get(0);
            if exists {
                let moved = tx.execute(copy, &[]).await?;
                tx.batch_execute(&format!("DROP TABLE _slonbase.{table}"))
                    .await?;
                tracing::info!(collection = %c.name, rows = moved, "moved rows into a system collection");
            }
        }
        created.push(c.name.clone());
        registry = store::load(&tx).await?;
    }

    if !created.is_empty() {
        crate::realtime::notify(&tx, "collections").await?;
        tx.commit().await?;
        tracing::info!(?created, "created system collections");
    }
    Ok(())
}
