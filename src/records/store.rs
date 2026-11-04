//! Reading records from their tables.

use deadpool_postgres::GenericClient;
use serde_json::{Map, Value};
use tokio_postgres::Row;

use super::values::{self, Values};
use crate::collections::model::{Collection, Field, Storage};
use crate::collections::schema::table_ref;
use crate::datetime;

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

/// `r."title" AS "title", to_char(r."created" ...) AS "created", ...`: built in one buffer, as
/// it's built for every record read (formatting each piece was an eighth of a single-record
/// read's instructions).
pub fn select_list(fields: &[&Field]) -> String {
    let mut out = String::with_capacity(fields.len() * 40);
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let date = matches!(f.storage(), Storage::Date);
        if date {
            out.push_str(datetime::SQL_FORMAT_PREFIX);
        }
        out.push_str(ALIAS);
        out.push('.');
        push_ident(&mut out, &f.name);
        if date {
            out.push_str(datetime::SQL_FORMAT_SUFFIX);
        }
        out.push_str(" AS ");
        push_ident(&mut out, &f.name);
    }
    out
}

/// Appends `name` quoted as an identifier, as [`quote_ident`] does.
fn push_ident(out: &mut String, name: &str) {
    out.push('"');
    if name.contains('"') {
        out.push_str(&name.replace('"', "\"\""));
    } else {
        out.push_str(name);
    }
    out.push('"');
}

/// The normalized value of field `f`, column `i` of a row selected with [`select_list`].
pub fn field_value(f: &Field, row: &Row, i: usize) -> Value {
    match f.storage() {
        Storage::Text => Value::String(row.get(i)),
        Storage::Number => values::number(row.get(i)),
        Storage::Bool => Value::Bool(row.get(i)),
        Storage::Date => Value::String(row.get::<_, Option<String>>(i).unwrap_or_default()),
        Storage::Multi => row.get(i),
        Storage::Json => row.get::<_, Option<Value>>(i).unwrap_or(Value::Null),
    }
}

/// Reads normalized values from a row selected with [`select_list`].
pub fn row_values(fields: &[&Field], row: &Row) -> Values {
    let mut map = Map::with_capacity(fields.len());
    for (i, f) in fields.iter().enumerate() {
        map.insert(f.name.clone(), field_value(f, row, i));
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
    // prepared once per connection: the SQL depends on the collection only (token checks run
    // it on every signed-in request)
    let statement = client.prepare_cached(&sql).await?;
    let row = client.query_opt(&statement, &[&value]).await?;
    Ok(row.map(|r| row_values(&fields, &r)))
}

pub async fn find_by_id(
    client: &impl GenericClient,
    c: &Collection,
    id: &str,
) -> Result<Option<Values>, tokio_postgres::Error> {
    find_by(client, c, &format!("{ALIAS}.\"id\" = $1::text"), id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::quote_ident;

    #[test]
    fn select_list_matches_the_formatted_one() {
        use crate::collections::model::Collection;
        let c: Collection = serde_json::from_value(serde_json::json!({
            "id": "c1", "name": "posts", "type": "base",
            "fields": [
                { "name": "id", "type": "text", "primaryKey": true },
                { "name": "title", "type": "text" },
                { "name": "we\"ird", "type": "number" },
                { "name": "due", "type": "date" },
                { "name": "created", "type": "autodate", "onCreate": true },
                { "name": "tags", "type": "select", "values": ["a"], "maxSelect": 2 },
                { "name": "meta", "type": "json" },
            ],
        }))
        .unwrap();
        let fields: Vec<&Field> = c.fields.iter().collect();
        let formatted = fields
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
            .join(", ");
        assert_eq!(select_list(&fields), formatted);
        assert!(formatted.contains("\"we\"\"ird\"") && formatted.contains("to_char("));
        assert_eq!(select_list(&[]), "");
    }
}
