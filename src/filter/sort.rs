//! The `sort` query parameter: `-created,title,@random`.

use super::compile::{CompileError, quote_ident};
use crate::collections::model::{Collection, FIELD_EMAIL, FIELD_EMAIL_VISIBILITY, Storage};

const MAX_SORT_FIELDS: usize = 8;

/// Returns the `ORDER BY` items. Always ends with the insertion order, so that
/// pagination is stable even when the sorted values repeat.
pub fn compile_sort(
    raw: &str,
    collection: &Collection,
    alias: &str,
    allow_hidden: bool,
) -> Result<Vec<String>, CompileError> {
    let mut items = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if items.len() >= MAX_SORT_FIELDS {
            return Err(CompileError(format!(
                "too many sort fields (the limit is {MAX_SORT_FIELDS})"
            )));
        }
        let (direction, name) = match part.as_bytes()[0] {
            b'-' => ("DESC", &part[1..]),
            b'+' => ("ASC", &part[1..]),
            _ => ("ASC", part),
        };
        let item = match name {
            "@random" => "random()".to_string(),
            "@rowid" => format!(
                "{} {direction}",
                crate::records::store::row_order(collection, alias)
            ),
            _ => {
                let field = collection
                    .field(name)
                    .filter(|f| !f.is_secret() && (!f.hidden || allow_hidden))
                    .ok_or_else(|| CompileError(format!("unknown sort field {name:?}")))?;
                if field.storage() == Storage::Json {
                    return Err(CompileError(format!(
                        "can't sort by the JSON field {name:?}"
                    )));
                }
                let column = format!("{alias}.{}", quote_ident(&field.name));
                // a client sorting by email would learn the hidden ones' order
                if !allow_hidden && collection.is_auth() && field.name == FIELD_EMAIL {
                    let visible = quote_ident(FIELD_EMAIL_VISIBILITY);
                    format!("(CASE WHEN {alias}.{visible} THEN {column} END) {direction}")
                } else {
                    format!("{column} {direction}")
                }
            }
        };
        items.push(item);
    }
    items.push(format!(
        "{} ASC",
        crate::records::store::row_order(collection, alias)
    ));
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::model::default_id_field;

    #[test]
    fn sort() {
        let c: Collection = serde_json::from_value(serde_json::json!({
            "name": "posts",
            "fields": [
                default_id_field(),
                { "id": "f1", "name": "title", "type": "text" },
                { "id": "f2", "name": "secret", "type": "text", "hidden": true },
                { "id": "f3", "name": "meta", "type": "json" }
            ]
        }))
        .unwrap();

        assert_eq!(
            compile_sort("-title, +id,@random", &c, "t", false).unwrap(),
            vec![
                r#"t."title" DESC"#,
                r#"t."id" ASC"#,
                "random()",
                "t._seq ASC"
            ]
        );
        assert_eq!(
            compile_sort("", &c, "t", false).unwrap(),
            vec!["t._seq ASC"]
        );
        assert_eq!(
            compile_sort("-@rowid", &c, "t", false).unwrap()[0],
            "t._seq DESC"
        );
        assert!(compile_sort("secret", &c, "t", false).is_err());
        assert!(compile_sort("secret", &c, "t", true).is_ok());
        assert!(compile_sort("nope", &c, "t", true).is_err());
        assert!(compile_sort("meta", &c, "t", true).is_err());
        assert!(compile_sort("title;drop", &c, "t", true).is_err());
    }
}
