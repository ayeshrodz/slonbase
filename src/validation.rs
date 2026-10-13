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
            let mut node = &mut root;
            for part in &parts[..parts.len() - 1] {
                let entry = node
                    .entry(part.to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                match entry.as_object_mut() {
                    Some(child) => node = child,
                    // an error already exists at a parent path, keep that one
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
