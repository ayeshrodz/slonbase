//! View collections: read-only records from a SQL query. The fields come from the query's
//! columns; the collection's PostgreSQL view casts each column to its field's storage, so
//! records read the same way as those of other collections.

use deadpool_postgres::GenericClient;
use tokio_postgres::types::{Kind, Type};

use super::model::{Collection, Field, FieldKind, Storage};
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
        Type::INT2
        | Type::INT4
        | Type::INT8
        | Type::FLOAT4
        | Type::FLOAT8
        | Type::NUMERIC
        | Type::OID => FieldKind::Number(Default::default()),
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
    let mut fields: Vec<Field> = Vec::new();
    for column in statement.columns() {
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
        if name == "id" {
            field.kind = FieldKind::Text(super::model::TextOptions {
                primary_key: true,
                required: true,
                ..Default::default()
            });
            field.system = true;
        }
        if let Some(old) = previous.iter().find(|f| f.name == name) {
            field.id = old.id.clone();
            field.hidden = old.hidden;
            field.presentable = old.presentable;
            field.help = old.help.clone();
            // e.g. a text column shown as a select or an email
            if old.storage() == field.storage() && name != "id" {
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
