//! State shared by request handlers once the app is ready.

use std::sync::{Arc, RwLock};

use anyhow::Result;
use deadpool_postgres::Pool;

use crate::collections::store::{self, Registry};
use crate::db::describe_pool_error;
use crate::ids;

pub struct Core {
    pub pool: Pool,
    registry: RwLock<Arc<Registry>>,
    auth_secret: Vec<u8>,
    /// Serializes collection changes within this instance (the database
    /// advisory lock serializes them across instances).
    pub(crate) schema_lock: tokio::sync::Mutex<()>,
}

impl Core {
    pub async fn load(pool: Pool) -> Result<Arc<Self>> {
        let client = pool.get().await.map_err(describe_pool_error)?;

        // created once per installation and shared by all instances
        client
            .execute(
                "INSERT INTO _slonbase.params (key, value) VALUES ('auth_secret', to_jsonb($1::text))
                 ON CONFLICT (key) DO NOTHING",
                &[&ids::random_secret(32)],
            )
            .await?;
        let secret: String = client
            .query_one(
                "SELECT value #>> '{}' FROM _slonbase.params WHERE key = 'auth_secret'",
                &[],
            )
            .await?
            .get(0);

        let registry = store::load(&client).await?;
        drop(client);

        Ok(Arc::new(Self {
            pool,
            registry: RwLock::new(Arc::new(registry)),
            auth_secret: secret.into_bytes(),
            schema_lock: tokio::sync::Mutex::new(()),
        }))
    }

    pub fn registry(&self) -> Arc<Registry> {
        self.registry.read().expect("registry lock").clone()
    }

    pub(crate) fn set_registry(&self, registry: Registry) {
        *self.registry.write().expect("registry lock") = Arc::new(registry);
    }

    pub(crate) fn auth_secret(&self) -> &[u8] {
        &self.auth_secret
    }
}
