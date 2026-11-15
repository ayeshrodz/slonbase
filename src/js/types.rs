//! The type definitions of what hook files can use (`types.d.ts`): the hook API, and a type
//! for the records of each collection, so an editor completes field names and checks them.
//!
//! The file is written next to the data (`slonbase_data/types.d.ts` by default) by
//! `slonbase types`, and kept up to date by a server started with `--dev`.

use std::path::{Path, PathBuf};

use crate::collections::model::{Collection, FieldKind, Storage};
use crate::collections::store::Registry;

const API: &str = include_str!("types.d.ts");

/// Where the definitions go unless told otherwise: next to the storage directory.
pub fn default_path(storage: &Path) -> PathBuf {
    match storage.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("types.d.ts"),
        _ => storage.join("types.d.ts"),
    }
}

/// A name as a TypeScript property: as it is, or quoted.
fn property(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    match plain {
        true => name.to_string(),
        false => serde_json::to_string(name).unwrap_or_default(),
    }
}

fn field_type(field: &crate::collections::model::Field) -> String {
    match (&field.kind, field.storage()) {
        (FieldKind::Select(options), storage) => {
            let values: Vec<String> = options
                .values
                .iter()
                .map(|v| serde_json::to_string(v).unwrap_or_default())
                .collect();
            let one = match values.is_empty() {
                true => "string".to_string(),
                false => values.join(" | "),
            };
            match storage {
                Storage::Multi => format!("Array<{one}>"),
                _ => format!("{one} | \"\""),
            }
        }
        (_, Storage::Text | Storage::Date) => "string".into(),
        (_, Storage::Number) => "number".into(),
        (_, Storage::Bool) => "boolean".into(),
        (_, Storage::Multi) => "string[]".into(),
        (_, Storage::Json) if field.type_name() == "geoPoint" => {
            "{ lon: number; lat: number }".into()
        }
        (_, Storage::Json) => "any".into(),
    }
}

fn record_type(c: &Collection) -> String {
    let mut out = format!(
        "  /** A record of {}. */\n  {}: {{\n",
        c.name,
        property(&c.name)
    );
    for field in &c.fields {
        // (a password is written, never read)
        if matches!(field.kind, FieldKind::Password(_)) {
            continue;
        }
        if !field.help.trim().is_empty() {
            let help = field.help.replace("*/", "* /").replace('\n', " ");
            out.push_str(&format!("    /** {help} */\n"));
        }
        out.push_str(&format!(
            "    {}: {};\n",
            property(&field.name),
            field_type(field)
        ));
    }
    out.push_str("  };\n");
    out
}

/// The definitions for these collections.
pub fn render(registry: &Registry) -> String {
    let mut collections: Vec<_> = registry.all().iter().collect();
    collections.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::from(API);
    out.push_str(
        "\n// ------------------------------------------------------------------ collections\n\n\
         /** The fields of each collection's records, by the collection's name. */\n\
         interface Collections {\n",
    );
    for c in collections {
        out.push_str(&record_type(c));
    }
    out.push_str(
        "}\n\n/** The names of the collections. */\ntype CollectionName = keyof Collections & string;\n",
    );
    out
}

/// Writes the definitions to `path` when they differ from what is there. Returns whether the
/// file was written.
pub fn write(registry: &Registry, path: &Path) -> anyhow::Result<bool> {
    let text = render(registry);
    if std::fs::read_to_string(path).is_ok_and(|current| current == text) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // (whole or not at all: an editor may read it at any moment)
    // (a file of this write's own: two writers at once each rename their own)
    let part = path.with_extension(format!("d.ts.{}.part", crate::ids::random_id(8)));
    std::fs::write(&part, text)?;
    if let Err(err) = std::fs::rename(&part, path) {
        let _ = std::fs::remove_file(&part);
        return Err(err.into());
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_properties() {
        assert_eq!(property("title"), "title");
        assert_eq!(property("_x1"), "_x1");
        assert_eq!(property("my-field"), "\"my-field\"");
        assert_eq!(property("1st"), "\"1st\"");
    }

    #[test]
    fn the_api_is_declared_once() {
        // every hook of the prelude has a declaration, and none is declared twice
        let prelude = include_str!("prelude.js");
        let mut declared: Vec<&str> = API
            .lines()
            .filter_map(|l| l.strip_prefix("declare function "))
            .filter_map(|l| l.split('(').next())
            .collect();
        let count = declared.len();
        declared.sort_unstable();
        declared.dedup();
        assert_eq!(declared.len(), count, "a function is declared twice");
        let mut word = String::new();
        let mut hooks = std::collections::BTreeSet::new();
        for c in prelude.chars().chain(std::iter::once(' ')) {
            if c.is_ascii_alphanumeric() {
                word.push(c);
            } else {
                let named = word.len() > 2
                    && word.starts_with("on")
                    && word.as_bytes()[2].is_ascii_uppercase();
                if named && word != "onModel" && word != "onModelAfter" {
                    hooks.insert(std::mem::take(&mut word));
                }
                word.clear();
            }
        }
        for hook in &hooks {
            // (a name the prelude only mentions for PocketBase's sake)
            if hook == "onBootstrapClear" {
                continue;
            }
            assert!(declared.contains(&hook.as_str()), "{hook} is not declared");
        }
        for global in [
            "routerAdd",
            "routerUse",
            "cronAdd",
            "cronRemove",
            "automationStep",
            "settingsAdd",
            "migrate",
        ] {
            assert!(declared.contains(&global), "{global} is not declared");
        }
    }
}
