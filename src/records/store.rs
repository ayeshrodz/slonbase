//! Reading records from their tables.

use deadpool_postgres::GenericClient;
use serde_json::{Map, Value};
use tokio_postgres::Row;

use super::values::{self, Values};
use crate::collections::model::{Collection, Field, Storage};
use crate::collections::schema::table_ref;
use crate::datetime;
use crate::filter::quote_ident;

/// SQL alias of the collection table in record queries.
pub const ALIAS: &str = "t";

/// The insertion order column (`_seq`); views have none, so they're ordered by id.
pub fn row_order(c: &Collection, alias: &str) -> String {
    if c.is_view() {
        format!("{alias}.\"id\"")
    } else {
        format!("{alias}._seq")
    }
}

/// Fields returned to clients, in field order. Secret fields (password hashes,
/// token keys) are never included.
pub fn visible_fields(c: &Collection, include_hidden: bool) -> Vec<&Field> {
    c.fields
        .iter()
        .filter(|f| !f.is_secret() && (include_hidden || !f.hidden))
        .collect()
}

/// Every field, including secrets (for internal use only).
pub fn all_fields(c: &Collection) -> Vec<&Field> {
    c.fields.iter().collect()
}

pub fn select_list(fields: &[&Field]) -> String {
    fields
        .iter()
        .map(|f| {
            let column = format!("{ALIAS}.{}", quote_ident(&f.name));
            let expr = match f.storage() {
                Storage::Date => datetime::sql_format(&column),
                _ => column,
            };
            format!("{expr} AS {}", quote_ident(&f.name))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reads normalized values from a row selected with [`select_list`].
pub fn row_values(fields: &[&Field], row: &Row) -> Values {
    let mut map = Map::with_capacity(fields.len());
    for (i, f) in fields.iter().enumerate() {
        let value = match f.storage() {
            Storage::Text => Value::String(row.get(i)),
            Storage::Number => values::number(row.get(i)),
            Storage::Bool => Value::Bool(row.get(i)),
            Storage::Date => Value::String(row.get::<_, Option<String>>(i).unwrap_or_default()),
            Storage::Multi => row.get(i),
            Storage::Json => row.get::<_, Option<Value>>(i).unwrap_or(Value::Null),
        };
        map.insert(f.name.clone(), value);
    }
    map
}

/// The API representation of a record.
pub fn record_json(c: &Collection, values: Values) -> Value {
    let mut out = Map::with_capacity(values.len() + 2);
    out.insert("collectionId".into(), Value::String(c.id.clone()));
    out.insert("collectionName".into(), Value::String(c.name.clone()));
    out.extend(values);
    Value::Object(out)
}

/// Loads all values of a record matching `column = value` (e.g. `"id"`), or `None`.
pub async fn find_by(
    client: &impl GenericClient,
    c: &Collection,
    condition_sql: &str,
    value: &str,
) -> Result<Option<Values>, tokio_postgres::Error> {
    let fields = all_fields(c);
    let sql = format!(
        "SELECT {} FROM {} {ALIAS} WHERE {condition_sql} LIMIT 1",
        select_list(&fields),
        table_ref(&c.name)
    );
    let row = client.query_opt(&sql, &[&value]).await?;
    Ok(row.map(|r| row_values(&fields, &r)))
}

pub async fn find_by_id(
    client: &impl GenericClient,
    c: &Collection,
    id: &str,
) -> Result<Option<Values>, tokio_postgres::Error> {
    find_by(client, c, &format!("{ALIAS}.\"id\" = $1::text"), id).await
}
