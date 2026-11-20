//! Validation errors keyed by path (`title`, `fields.2.name`), rendered as nested
//! objects like PocketBase: `{"fields": {"2": {"name": {"code": "...", "message": "..."}}}}`.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Errors(BTreeMap<String, Issue>);

impl Errors {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an error; only the first error per path is kept.
    pub fn add(&mut self, path: impl Into<String>, code: &'static str, message: impl Into<String>) {
        self.0.entry(path.into()).or_insert(Issue {
            code,
            message: message.into(),
        });
    }

    pub fn invalid(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.add(path, "validation_invalid", message);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Adds another set of errors (keeping the ones already there).
    pub fn extend(&mut self, other: Errors) {
        for (path, issue) in other.0 {
            self.0.entry(path).or_insert(issue);
        }
    }

    /// Adds another set of errors under `prefix` (e.g. `collections.2`).
    pub fn extend_prefixed(&mut self, prefix: &str, other: Errors) {
        for (path, issue) in other.0 {
            self.0.entry(format!("{prefix}.{path}")).or_insert(issue);
        }
    }

    pub fn contains(&self, path: &str) -> bool {
        self.0.contains_key(path)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Issue)> {
        self.0.iter()
    }

    pub fn to_json(&self) -> Map<String, Value> {
        let mut root = Map::new();
        'issues: for (path, issue) in &self.0 {
            let parts: Vec<&str> = path.split('.').collect();
            // an error already exists at a parent path (`fields`, for `fields.1.name`): keep that
            // one, the leaf made for it can't hold the nested ones
            if (1..parts.len()).any(|n| self.0.contains_key(&parts[..n].join("."))) {
                continue;
            }
            let mut node = &mut root;
            for part in &parts[..parts.len() - 1] {
                let entry = node
                    .entry(part.to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                match entry.as_object_mut() {
                    Some(child) => node = child,
                    None => continue 'issues,
                }
            }
            node.entry(parts[parts.len() - 1].to_string())
                .or_insert_with(|| json!({ "code": issue.code, "message": issue.message }));
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_at_a_parent_path_is_kept_whole() {
        let mut e = Errors::new();
        e.invalid("fields", "top");
        e.invalid("fields.1.name", "nested");
        e.invalid("other.x", "fine");
        assert_eq!(
            Value::Object(e.to_json()),
            json!({
                "fields": { "code": "validation_invalid", "message": "top" },
                "other": { "x": { "code": "validation_invalid", "message": "fine" } },
            })
        );
    }

    #[test]
    fn nests_paths() {
        let mut e = Errors::new();
        e.invalid("name", "bad name");
        e.add("fields.2.name", "validation_required", "Cannot be blank.");
        e.invalid("fields.2.name", "ignored, only the first error is kept");
        e.invalid("fields.3.type", "bad type");

        assert_eq!(
            Value::Object(e.to_json()),
            json!({
                "name": { "code": "validation_invalid", "message": "bad name" },
                "fields": {
                    "2": { "name": { "code": "validation_required", "message": "Cannot be blank." } },
                    "3": { "type": { "code": "validation_invalid", "message": "bad type" } }
                }
            })
        );
    }
}
