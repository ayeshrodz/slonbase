//! View collections: read-only records from a SQL query. The fields come from the query's
//! columns: a column taken from a collection's field is a copy of that field (as in
//! PocketBase), others get a field for their PostgreSQL type. The collection's PostgreSQL view
//! casts each column to its field's storage, so records read the same way as those of other
//! collections.

use deadpool_postgres::GenericClient;
use tokio_postgres::types::{Kind, Type};

use super::model::{Collection, Field, FieldKind, NumberOptions, RelationOptions, Storage};
use super::schema::table_ref;
use crate::filter::quote_ident;

/// Longest view query.
pub const MAX_QUERY_LENGTH: usize = 50_000;

/// Checks the shape of a view query: one `SELECT` (or `WITH`) statement.
pub fn check_query(query: &str) -> Result<(), String> {
    let trimmed = query.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err("Cannot be blank.".into());
    }
    if trimmed.len() > MAX_QUERY_LENGTH {
        return Err(format!("Must be at most {MAX_QUERY_LENGTH} characters."));
    }
    let upper = trimmed.to_ascii_uppercase();
    if !(upper.starts_with("SELECT") || upper.starts_with("WITH") || upper.starts_with("(")) {
        return Err("Must be a SELECT query.".into());
    }
    if trimmed.contains(';') {
        return Err("Must be a single statement.".into());
    }
    Ok(())
}

fn normalized(query: &str) -> &str {
    query.trim().trim_end_matches(';').trim()
}

/// The field type for a column type.
fn field_for(name: &str, ty: &Type) -> FieldKind {
    let json = || FieldKind::Json(Default::default());
    if matches!(ty.kind(), Kind::Array(_)) {
        return json();
    }
    match *ty {
        // whole numbers (`count(*)`, `::int`) as PocketBase types them
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID => FieldKind::Number(NumberOptions {
            only_int: true,
            ..Default::default()
        }),
        Type::FLOAT4 | Type::FLOAT8 | Type::NUMERIC => FieldKind::Number(Default::default()),
        Type::BOOL => FieldKind::Bool(Default::default()),
        Type::TIMESTAMPTZ | Type::TIMESTAMP | Type::DATE => FieldKind::Date(Default::default()),
        Type::JSON | Type::JSONB => json(),
        _ => {
            let _ = name;
            FieldKind::Text(Default::default())
        }
    }
}

/// Fields for the query's columns. Fields of `previous` with the same name keep their id
/// and display options (and their type, when it's stored the same way).
pub async fn infer_fields(
    client: &impl GenericClient,
    query: &str,
    previous: &[Field],
) -> Result<Vec<Field>, String> {
    check_query(query)?;
    let statement = client
        .prepare(&format!("SELECT * FROM ({}) _q LIMIT 0", normalized(query)))
        .await
        .map_err(|e| {
            e.as_db_error()
                .map(|db| db.message().to_string())
                .unwrap_or_else(|| e.to_string())
        })?;
    let sources = column_sources(client, statement.columns()).await?;
    let mut fields: Vec<Field> = Vec::new();
    for (column, source) in statement.columns().iter().zip(sources) {
        let name = column.name().to_string();
        if name.is_empty() || name == "?column?" {
            return Err("Every column needs a name (use AS).".into());
        }
        if fields.iter().any(|f| f.name == name) {
            return Err(format!("The column {name:?} appears more than once."));
        }
        if name.starts_with('_') {
            return Err(format!("Column names can't start with _ ({name:?})."));
        }
        let mut field = Field {
            id: String::new(),
            name: name.clone(),
            system: false,
            hidden: false,
            presentable: false,
            help: String::new(),
            kind: field_for(&name, column.type_()),
        };
        let sourced = name != "id" && source.is_some();
        if name == "id" {
            field.kind = FieldKind::Text(super::model::TextOptions {
                primary_key: true,
                required: true,
                pattern: "^[a-z0-9]+$".into(),
                ..Default::default()
            });
            field.system = true;
        } else if let Some(source) = source {
            field.system = source.system;
            field.hidden = source.hidden;
            field.presentable = source.presentable;
            field.help = source.help;
            field.kind = source.kind;
        }
        if let Some(old) = previous.iter().find(|f| f.name == name) {
            // how the view shows the field is the view's; what it is comes from its source
            field.id = old.id.clone();
            // (a source that's hidden stays hidden: a view saved before its fields were copied
            // may show a password hash or a hidden field as plain text)
            field.hidden = old.hidden || (sourced && field.hidden);
            field.presentable = old.presentable;
            field.help = old.help.clone();
            // e.g. a text column shown as a select or an email
            if !sourced && old.storage() == field.storage() && name != "id" {
                field.kind = old.kind.clone();
            }
        }
        if field.id.is_empty() {
            field.id = format!("{}{}", field.type_name(), crate::ids::random_id(10));
        }
        fields.push(field);
    }
    if !fields.iter().any(|f| f.name == "id") {
        return Err("The query must return an id column.".into());
    }
    Ok(fields)
}

/// For each column of a view query, the field it's a copy of when it's taken from a
/// collection's column (PostgreSQL reports where each column comes from, through joins,
/// subqueries and CTEs; expressions come from nowhere): the field itself, or, for a
/// collection's `id`, a relation to that collection, as in PocketBase. A source that can't be
/// a view's field (an auth collection's password hash) becomes hidden text, which only
/// superusers see.
async fn column_sources(
    client: &impl GenericClient,
    columns: &[tokio_postgres::Column],
) -> Result<Vec<Option<Field>>, String> {
    let origins: Vec<Option<(u32, i16)>> = columns
        .iter()
        .map(|c| {
            c.table_oid()
                .zip(c.column_id())
                .filter(|(t, a)| *t != 0 && *a > 0)
        })
        .collect();
    if origins.iter().all(Option::is_none) {
        return Ok(vec![None; columns.len()]);
    }
    let tables: Vec<u32> = origins.iter().flatten().map(|(t, _)| *t).collect();
    let attnums: Vec<i16> = origins.iter().flatten().map(|(_, a)| *a).collect();
    // the collection (by its table) and the column name of each origin
    let rows = client
        .query(
            "SELECT o.t, o.a, col.id, col.data -> 'fields', att.attname
             FROM unnest($1::oid[], $2::int2[]) AS o(t, a)
             JOIN pg_class rel ON rel.oid = o.t
             JOIN pg_namespace ns ON ns.oid = rel.relnamespace AND ns.nspname = 'slonbase'
             JOIN pg_attribute att ON att.attrelid = o.t AND att.attnum = o.a
             JOIN _slonbase.collections col ON col.name = rel.relname",
            &[&tables, &attnums],
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut found = std::collections::HashMap::new();
    for row in rows {
        let (table, attnum): (u32, i16) = (row.get(0), row.get(1));
        let collection_id: String = row.get(2);
        let fields: Option<serde_json::Value> = row.get(3);
        let column: String = row.get(4);
        let source = if column == "id" {
            Some(Field {
                id: String::new(),
                name: String::new(),
                system: false,
                hidden: false,
                presentable: false,
                help: String::new(),
                kind: FieldKind::Relation(RelationOptions {
                    collection_id,
                    max_select: 1,
                    ..Default::default()
                }),
            })
        } else {
            fields
                .and_then(|f| serde_json::from_value::<Vec<Field>>(f).ok())
                .and_then(|f| f.into_iter().find(|f| f.name == column))
                .map(|mut f| {
                    if matches!(f.kind, FieldKind::Password(_)) {
                        f.kind = FieldKind::Text(Default::default());
                        f.hidden = true;
                    }
                    f
                })
        };
        found.insert((table, attnum), source);
    }
    Ok(origins
        .into_iter()
        .map(|o| o.and_then(|o| found.get(&o).cloned().flatten()))
        .collect())
}

/// The collection (id) and column a view's `field` is taken from, if it's a collection's
/// column: what serves a view record's files, which are stored with the source record.
pub async fn field_source(
    client: &mut deadpool_postgres::Client,
    view: &Collection,
    field: &str,
) -> Result<Option<(String, String)>, String> {
    let query = view.view_query.as_deref().unwrap_or_default();
    check_query(query)?;
    let tx = client.transaction().await.map_err(|e| e.to_string())?;
    tx.batch_execute(SEARCH_PATH)
        .await
        .map_err(|e| e.to_string())?;
    let statement = tx
        .prepare(&format!("SELECT * FROM ({}) _q LIMIT 0", normalized(query)))
        .await
        .map_err(|e| e.to_string())?;
    let Some(origin) = statement
        .columns()
        .iter()
        .find(|c| c.name() == field)
        .and_then(|c| c.table_oid().zip(c.column_id()))
    else {
        return Ok(None);
    };
    let row = tx
        .query_opt(
            "SELECT col.id, att.attname::text
             FROM pg_class rel
             JOIN pg_namespace ns ON ns.oid = rel.relnamespace AND ns.nspname = 'slonbase'
             JOIN pg_attribute att ON att.attrelid = rel.oid AND att.attnum = $2
             JOIN _slonbase.collections col ON col.name = rel.relname
             WHERE rel.oid = $1",
            &[&origin.0, &origin.1],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(row.map(|r| (r.get(0), r.get(1))))
}

/// The column expression for a field: the query's column cast to the field's storage.
fn cast(field: &Field) -> String {
    let column = format!("_q.{}", quote_ident(&field.name));
    let expr = match field.storage() {
        Storage::Text => format!("COALESCE(({column})::text, '')"),
        Storage::Number => format!("COALESCE(({column})::double precision, 0)"),
        Storage::Bool => format!("COALESCE(({column})::boolean, FALSE)"),
        Storage::Date => format!("({column})::timestamptz"),
        Storage::Multi => format!("COALESCE(to_jsonb({column}), '[]'::jsonb)"),
        Storage::Json => format!("to_jsonb({column})"),
    };
    format!("{expr} AS {}", quote_ident(&field.name))
}

/// The query reading a sample of a view query's rows, as the view would give them (the
/// columns cast to their fields' storage), for [`crate::records::store::row_values`].
pub fn sample(query: &str, fields: &[Field], limit: usize) -> String {
    let columns: Vec<String> = fields.iter().map(cast).collect();
    let refs: Vec<&Field> = fields.iter().collect();
    format!(
        "SELECT {} FROM (SELECT {} FROM ({}) _q LIMIT {limit}) {}",
        crate::records::store::select_list(&refs),
        columns.join(", "),
        normalized(query),
        crate::records::store::ALIAS
    )
}

/// Unqualified table names in view queries are collections (`FROM posts`, as in
/// PocketBase). Runs in the transaction that creates or inspects the view.
pub const SEARCH_PATH: &str = "SET LOCAL search_path = slonbase, public";

pub fn create(c: &Collection) -> Vec<String> {
    let columns: Vec<String> = c.fields.iter().map(cast).collect();
    vec![
        SEARCH_PATH.to_string(),
        format!(
            "CREATE VIEW {} AS SELECT {} FROM ({}) _q",
            table_ref(&c.name),
            columns.join(", "),
            normalized(c.view_query.as_deref().unwrap_or_default())
        ),
    ]
}

pub fn drop(c: &Collection) -> Vec<String> {
    vec![format!("DROP VIEW IF EXISTS {}", table_ref(&c.name))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries() {
        assert!(check_query("SELECT id FROM posts;").is_ok());
        assert!(check_query("with a as (select 1) select * from a").is_ok());
        assert!(check_query("").is_err());
        assert!(check_query("DELETE FROM posts").is_err());
        assert!(check_query("SELECT 1; DROP TABLE x").is_err());
    }
}
