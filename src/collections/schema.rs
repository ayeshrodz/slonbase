//! DDL for collection tables, which live in the `slonbase` schema.

use super::indexes::Index;
use super::model::{Collection, FIELD_EMAIL, FIELD_TOKEN_KEY, Field, Storage};
use crate::filter::quote_ident;

/// Schema holding the user collection tables.
pub const DATA_SCHEMA: &str = "slonbase";

pub fn table_ref(name: &str) -> String {
    format!("{DATA_SCHEMA}.{}", quote_ident(name))
}

/// Internal index on the insertion order column (stable default sort and pagination).
fn seq_index_name(collection_id: &str) -> String {
    format!("_seq_{collection_id}")
}

/// System-managed unique index names of auth collections (not listed in `indexes`).
pub fn email_index_name(collection_id: &str) -> String {
    format!("_email_{collection_id}")
}

pub fn token_key_index_name(collection_id: &str) -> String {
    format!("_tokenkey_{collection_id}")
}

fn column_def(field: &Field) -> String {
    format!("{} {}", quote_ident(&field.name), field.column_type())
}

/// The trigger that writes the realtime events of every change of the table (any writer's;
/// see `migrations/0010_realtime_triggers.sql`), with the collection's id, its secret columns
/// (left out of events) and its date columns (formatted as the API sends them).
pub fn realtime_trigger(c: &Collection) -> String {
    let (secrets, dates) = trigger_columns(c);
    let array = |names: Vec<&str>| {
        let items: Vec<String> = names.iter().map(|n| format!("\"{n}\"")).collect();
        format!("'{{{}}}'", items.join(",").replace('\'', "''"))
    };
    format!(
        "CREATE OR REPLACE TRIGGER _slonbase_realtime AFTER INSERT OR UPDATE OR DELETE ON {} \
         FOR EACH ROW EXECUTE FUNCTION _slonbase.record_change('{}', {}, {})",
        table_ref(&c.name),
        c.id.replace('\'', "''"),
        array(secrets),
        array(dates)
    )
}

fn trigger_columns(c: &Collection) -> (Vec<&str>, Vec<&str>) {
    let mut secrets: Vec<&str> = Vec::new();
    let mut dates: Vec<&str> = Vec::new();
    for f in &c.fields {
        if f.is_secret() {
            secrets.push(&f.name);
        } else if f.storage() == Storage::Date {
            dates.push(&f.name);
        }
    }
    secrets.sort_unstable();
    dates.sort_unstable();
    (secrets, dates)
}

fn parsed_indexes(c: &Collection) -> Vec<Index> {
    // indexes are validated before reaching here
    c.indexes
        .iter()
        .filter_map(|raw| Index::parse(raw).ok())
        .collect()
}

pub fn create(c: &Collection) -> Vec<String> {
    if c.is_view() {
        return super::view::create(c);
    }
    let table = table_ref(&c.name);
    let mut columns: Vec<String> = c.fields.iter().map(column_def).collect();
    columns.push("_seq BIGINT GENERATED ALWAYS AS IDENTITY".into());

    let mut statements = vec![
        format!("CREATE TABLE {table} ({})", columns.join(", ")),
        format!(
            "CREATE INDEX {} ON {table} (_seq)",
            quote_ident(&seq_index_name(&c.id))
        ),
    ];
    if c.is_auth() {
        statements.push(format!(
            "CREATE UNIQUE INDEX {} ON {table} ({})",
            quote_ident(&token_key_index_name(&c.id)),
            quote_ident(FIELD_TOKEN_KEY)
        ));
        // case-insensitive, and empty emails don't conflict
        let email = quote_ident(FIELD_EMAIL);
        statements.push(format!(
            "CREATE UNIQUE INDEX {} ON {table} (lower({email})) WHERE {email} <> ''",
            quote_ident(&email_index_name(&c.id))
        ));
    }
    statements.extend(parsed_indexes(c).iter().map(|i| i.to_postgres_sql(&table)));
    statements.push(realtime_trigger(c));
    statements
}

pub fn drop(c: &Collection) -> Vec<String> {
    if c.is_view() {
        return super::view::drop(c);
    }
    vec![format!("DROP TABLE {}", table_ref(&c.name))]
}

/// Statements turning the `old` table into the `new` one. Fields are matched by id.
pub fn update(old: &Collection, new: &Collection) -> Vec<String> {
    if new.is_view() {
        // views are recreated (their columns may change)
        let mut statements = super::view::drop(old);
        statements.extend(super::view::create(new));
        return statements;
    }
    let mut statements = Vec::new();
    let table = table_ref(&new.name);

    if old.name != new.name {
        statements.push(format!(
            "ALTER TABLE {} RENAME TO {}",
            table_ref(&old.name),
            quote_ident(&new.name)
        ));
    }

    // indexes are compared by definition; changed ones are recreated
    let old_indexes = parsed_indexes(old);
    let new_indexes = parsed_indexes(new);
    for index in old_indexes.iter().filter(|i| !new_indexes.contains(i)) {
        statements.push(format!(
            "DROP INDEX {DATA_SCHEMA}.{}",
            quote_ident(&index.name)
        ));
    }

    let mut alter = Vec::new();

    for field in &old.fields {
        if new.field_by_id(&field.id).is_none() {
            alter.push(format!("DROP COLUMN {}", quote_ident(&field.name)));
        }
    }
    if !alter.is_empty() {
        statements.push(format!("ALTER TABLE {table} {}", alter.join(", ")));
        alter.clear();
    }

    // renames go through temporary names so that fields can swap names
    let renamed: Vec<(&Field, &Field)> = new
        .fields
        .iter()
        .filter_map(|f| old.field_by_id(&f.id).map(|o| (o, f)))
        .filter(|(o, f)| o.name != f.name)
        .collect();
    for (old_field, field) in &renamed {
        statements.push(format!(
            "ALTER TABLE {table} RENAME COLUMN {} TO {}",
            quote_ident(&old_field.name),
            quote_ident(&temp_name(field))
        ));
    }
    for (_, field) in &renamed {
        statements.push(format!(
            "ALTER TABLE {table} RENAME COLUMN {} TO {}",
            quote_ident(&temp_name(field)),
            quote_ident(&field.name)
        ));
    }

    // single <-> multiple value conversions (select and relation)
    for field in &new.fields {
        let Some(old_field) = old.field_by_id(&field.id) else {
            continue;
        };
        let column = quote_ident(&field.name);
        match (old_field.storage(), field.storage()) {
            (Storage::Text, Storage::Multi) => alter.push(format!(
                "ALTER COLUMN {column} DROP DEFAULT, \
                 ALTER COLUMN {column} TYPE JSONB USING \
                 (CASE WHEN {column} = '' THEN '[]'::jsonb ELSE jsonb_build_array({column}) END), \
                 ALTER COLUMN {column} SET DEFAULT '[]'"
            )),
            (Storage::Multi, Storage::Text) => alter.push(format!(
                "ALTER COLUMN {column} DROP DEFAULT, \
                 ALTER COLUMN {column} TYPE TEXT USING COALESCE({column} ->> -1, ''), \
                 ALTER COLUMN {column} SET DEFAULT ''"
            )),
            _ => {}
        }
    }

    for field in &new.fields {
        if old.field_by_id(&field.id).is_none() {
            alter.push(format!("ADD COLUMN {}", column_def(field)));
        }
    }
    if !alter.is_empty() {
        statements.push(format!("ALTER TABLE {table} {}", alter.join(", ")));
    }

    for index in new_indexes.iter().filter(|i| !old_indexes.contains(i)) {
        statements.push(index.to_postgres_sql(&table));
    }
    // the trigger names the date and secret columns
    if old.id != new.id || trigger_columns(old) != trigger_columns(new) {
        statements.push(realtime_trigger(new));
    }

    statements
}

fn temp_name(field: &Field) -> String {
    format!("_tmp_{}", field.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::model::*;
    use serde_json::json;

    fn collection(value: serde_json::Value) -> Collection {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn create_table() {
        let c = collection(json!({
            "id": "pbc_1",
            "name": "posts",
            "fields": [
                default_id_field(),
                { "id": "f1", "name": "title", "type": "text" },
                { "id": "f2", "name": "tags", "type": "select", "values": ["a"], "maxSelect": 2 },
                { "id": "f3", "name": "created", "type": "autodate", "onCreate": true }
            ],
            "indexes": ["CREATE UNIQUE INDEX `idx_title` ON `posts` (`title`)"]
        }));
        assert_eq!(
            create(&c),
            vec![
                "CREATE TABLE slonbase.\"posts\" (\"id\" TEXT PRIMARY KEY, \"title\" TEXT NOT NULL DEFAULT '', \
                 \"tags\" JSONB NOT NULL DEFAULT '[]', \"created\" TIMESTAMPTZ, \
                 _seq BIGINT GENERATED ALWAYS AS IDENTITY)",
                "CREATE INDEX \"_seq_pbc_1\" ON slonbase.\"posts\" (_seq)",
                "CREATE UNIQUE INDEX \"idx_title\" ON slonbase.\"posts\" (\"title\")",
                "CREATE OR REPLACE TRIGGER _slonbase_realtime AFTER INSERT OR UPDATE OR DELETE ON \
                 slonbase.\"posts\" FOR EACH ROW EXECUTE FUNCTION \
                 _slonbase.record_change('pbc_1', '{}', '{\"created\"}')",
            ]
        );
    }

    #[test]
    fn update_table() {
        let old = collection(json!({
            "id": "pbc_1",
            "name": "posts",
            "fields": [
                default_id_field(),
                { "id": "a", "name": "first", "type": "text" },
                { "id": "b", "name": "second", "type": "text" },
                { "id": "c", "name": "gone", "type": "number" },
                { "id": "d", "name": "tag", "type": "select", "values": ["x"], "maxSelect": 1 }
            ],
            "indexes": ["CREATE INDEX `idx_first` ON `posts` (`first`)"]
        }));
        let new = collection(json!({
            "id": "pbc_1",
            "name": "articles",
            "fields": [
                default_id_field(),
                { "id": "a", "name": "second", "type": "text" },
                { "id": "b", "name": "first", "type": "text" },
                { "id": "d", "name": "tag", "type": "select", "values": ["x", "y"], "maxSelect": 2 },
                { "id": "e", "name": "added", "type": "bool" }
            ],
            "indexes": ["CREATE INDEX `idx_added` ON `articles` (`added`)"]
        }));

        let sql = update(&old, &new);
        assert_eq!(
            sql[0],
            "ALTER TABLE slonbase.\"posts\" RENAME TO \"articles\""
        );
        assert_eq!(sql[1], "DROP INDEX slonbase.\"idx_first\"");
        assert_eq!(
            sql[2],
            "ALTER TABLE slonbase.\"articles\" DROP COLUMN \"gone\""
        );
        // swap via temporary names
        assert_eq!(
            sql[3],
            "ALTER TABLE slonbase.\"articles\" RENAME COLUMN \"first\" TO \"_tmp_a\""
        );
        assert_eq!(
            sql[4],
            "ALTER TABLE slonbase.\"articles\" RENAME COLUMN \"second\" TO \"_tmp_b\""
        );
        assert_eq!(
            sql[5],
            "ALTER TABLE slonbase.\"articles\" RENAME COLUMN \"_tmp_a\" TO \"second\""
        );
        assert_eq!(
            sql[6],
            "ALTER TABLE slonbase.\"articles\" RENAME COLUMN \"_tmp_b\" TO \"first\""
        );
        assert!(sql[7].contains("TYPE JSONB USING"), "{}", sql[7]);
        assert!(
            sql[7].ends_with("ADD COLUMN \"added\" BOOLEAN NOT NULL DEFAULT FALSE"),
            "{}",
            sql[7]
        );
        assert_eq!(
            sql[8],
            "CREATE INDEX \"idx_added\" ON slonbase.\"articles\" (\"added\")"
        );
        assert_eq!(sql.len(), 9);

        assert!(update(&new, &new).is_empty());
    }
}
