//! Record access for trusted code (hooks, custom routes and cron jobs): finding, saving
//! and deleting records without API rules. Writes go through the same validation, file
//! handling, realtime events and after-success hooks as API requests (but not the request
//! hooks).
//!
//! Records are JSON objects in the API's shape (`collectionId`, `collectionName` and the
//! fields), with hidden fields included and secrets (password hashes, token keys) left out.

use std::sync::Arc;

use serde_json::{Map, Value};

use super::api::{FileEffects, WriteContext, create_in, delete_in, update_in};
use super::store::{self, ALIAS, record_json, row_values, select_list, visible_fields};
use crate::auth::RequestAuth;
use crate::collections::model::{Collection, FIELD_PASSWORD};
use crate::collections::schema::table_ref;
use crate::core::Core;
use crate::db::describe_pool_error;
use crate::error::ApiError;
use crate::filter::{Params, RequestInfo, Scope, compile_sort, to_sql_with_params};

pub type Record = Map<String, Value>;

/// Keys of API records that aren't fields.
const NON_FIELD_KEYS: [&str; 3] = ["collectionId", "collectionName", "expand"];

fn db_error(err: tokio_postgres::Error) -> ApiError {
    ApiError::internal(err.into())
}

fn into_record(record: Value) -> Record {
    match record {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

impl Core {
    /// A collection by id or name.
    pub fn collection(&self, id_or_name: &str) -> Result<Arc<Collection>, ApiError> {
        self.registry().get(id_or_name).cloned().ok_or_else(|| {
            ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "Missing collection context.",
            )
        })
    }

    fn system_request(&self, method: &str) -> RequestInfo {
        let mut request = RequestInfo::new(method);
        request.registry = Some(self.registry());
        request
    }

    async fn client(&self) -> Result<deadpool_postgres::Client, ApiError> {
        self.pool
            .get()
            .await
            .map_err(|e| ApiError::internal(describe_pool_error(e)))
    }

    /// A record by id, or `None`.
    pub async fn find_record_by_id(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<Option<Record>, ApiError> {
        let c = self.collection(collection)?;
        let client = self.client().await?;
        let fields = visible_fields(&c, true);
        let row = client
            .query_opt(
                &format!(
                    "SELECT {} FROM {} {ALIAS} WHERE {ALIAS}.\"id\" = $1::text LIMIT 1",
                    select_list(&fields),
                    table_ref(&c.name)
                ),
                &[&id],
            )
            .await
            .map_err(db_error)?;
        Ok(row.map(|row| into_record(record_json(&c, row_values(&fields, &row)))))
    }

    /// Records matching a filter (empty for all), with `{:name}` placeholders replaced by
    /// `params`. `sort` uses the API syntax (`-created,title`); a `limit` of 0 means no limit.
    pub async fn find_records_by_filter(
        &self,
        collection: &str,
        filter: &str,
        sort: &str,
        limit: i64,
        offset: i64,
        params: &Map<String, Value>,
    ) -> Result<Vec<Record>, ApiError> {
        let c = self.collection(collection)?;
        let request = self.system_request("GET");
        let mut sql_params = Params::default();
        let where_sql = self.filter_sql(&c, filter, params, &request, &mut sql_params)?;
        let order = compile_sort(sort, &c, ALIAS, true)
            .map_err(|e| ApiError::bad_request(format!("Invalid sort: {e}.")))?;
        let fields = visible_fields(&c, true);
        let mut sql = format!(
            "SELECT {} FROM {} {ALIAS} WHERE {where_sql} ORDER BY {}",
            select_list(&fields),
            table_ref(&c.name),
            order.join(", ")
        );
        if limit > 0 {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        if offset > 0 {
            sql.push_str(&format!(" OFFSET {offset}"));
        }
        let client = self.client().await?;
        let rows = client
            .query(&sql, &sql_params.as_refs())
            .await
            .map_err(db_error)?;
        Ok(rows
            .iter()
            .map(|row| into_record(record_json(&c, row_values(&fields, row))))
            .collect())
    }

    /// The first record matching a filter, or `None`.
    pub async fn find_first_record_by_filter(
        &self,
        collection: &str,
        filter: &str,
        params: &Map<String, Value>,
    ) -> Result<Option<Record>, ApiError> {
        Ok(self
            .find_records_by_filter(collection, filter, "", 1, 0, params)
            .await?
            .into_iter()
            .next())
    }

    /// The number of records matching a filter (empty for all).
    pub async fn count_records(
        &self,
        collection: &str,
        filter: &str,
        params: &Map<String, Value>,
    ) -> Result<i64, ApiError> {
        let c = self.collection(collection)?;
        let request = self.system_request("GET");
        let mut sql_params = Params::default();
        let where_sql = self.filter_sql(&c, filter, params, &request, &mut sql_params)?;
        let client = self.client().await?;
        let row = client
            .query_one(
                &format!(
                    "SELECT count(*) FROM {} {ALIAS} WHERE {where_sql}",
                    table_ref(&c.name)
                ),
                &sql_params.as_refs(),
            )
            .await
            .map_err(db_error)?;
        Ok(row.get(0))
    }

    fn filter_sql(
        &self,
        c: &Collection,
        filter: &str,
        values: &Map<String, Value>,
        request: &RequestInfo,
        params: &mut Params,
    ) -> Result<String, ApiError> {
        if filter.trim().is_empty() {
            return Ok("TRUE".into());
        }
        let scope = Scope {
            collection: c,
            alias: ALIAS,
            request,
            allow_hidden: true,
        };
        to_sql_with_params(filter, values, &scope, params)
            .map_err(|e| ApiError::bad_request(format!("Invalid filter: {e}.")))
    }

    /// Creates a record, or updates it when `data` has the id of an existing one (then
    /// only the given fields change). Returns the saved record.
    pub async fn save_record(
        self: &Arc<Self>,
        collection: &str,
        mut data: Map<String, Value>,
    ) -> Result<Record, ApiError> {
        let c = self.collection(collection)?;
        for key in NON_FIELD_KEYS {
            data.remove(key);
        }
        if let Some(password) = data.get(FIELD_PASSWORD).cloned() {
            data.entry("passwordConfirm").or_insert(password);
        }
        let registry = self.registry();
        let auth = RequestAuth::system(self);
        let mut request = self.system_request("POST");
        request.auth = auth.info();
        let ctx = WriteContext {
            core: self,
            registry: &registry,
            auth: &auth,
            request: &request,
        };
        let id = data
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut effects = FileEffects::default();
        let outcome = async {
            let mut client = self.client().await?;
            let tx = client.transaction().await.map_err(db_error)?;
            let exists = !id.is_empty()
                && store::find_by_id(&tx, &c, &id)
                    .await
                    .map_err(db_error)?
                    .is_some();
            let record = if exists {
                data.remove("id");
                update_in(&ctx, &tx, &c, &id, data, Vec::new(), &mut effects).await?
            } else {
                create_in(&ctx, &tx, &c, data, Vec::new(), &mut effects).await?
            };
            tx.commit().await.map_err(db_error)?;
            Ok(record)
        }
        .await;
        let record = effects.settle(self, outcome).await?;
        Ok(into_record(record))
    }

    /// Deletes a record (with its cascades and files).
    pub async fn delete_record(
        self: &Arc<Self>,
        collection: &str,
        id: &str,
    ) -> Result<(), ApiError> {
        let c = self.collection(collection)?;
        let registry = self.registry();
        let auth = RequestAuth::system(self);
        let request = self.system_request("DELETE");
        let ctx = WriteContext {
            core: self,
            registry: &registry,
            auth: &auth,
            request: &request,
        };
        let mut effects = FileEffects::default();
        let outcome = async {
            let mut client = self.client().await?;
            let tx = client.transaction().await.map_err(db_error)?;
            delete_in(&ctx, &tx, &c, id, &mut effects).await?;
            tx.commit().await.map_err(db_error)?;
            Ok(())
        }
        .await;
        effects.settle(self, outcome).await
    }
}
