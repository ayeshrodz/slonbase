//! Collection and field definitions, (de)serialized in PocketBase's JSON format.

use serde::{Deserialize, Serialize};

/// Id PocketBase uses for its superusers collection (kept for SDK compatibility).
pub const SUPERUSERS_COLLECTION_ID: &str = "pbc_3142635823";
pub const SUPERUSERS_COLLECTION_NAME: &str = "_superusers";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: CollectionKind,
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub indexes: Vec<String>,
    #[serde(default)]
    pub list_rule: Option<String>,
    #[serde(default)]
    pub view_rule: Option<String>,
    #[serde(default)]
    pub create_rule: Option<String>,
    #[serde(default)]
    pub update_rule: Option<String>,
    #[serde(default)]
    pub delete_rule: Option<String>,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub updated: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollectionKind {
    #[default]
    Base,
}

impl Collection {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
    }

    pub fn field_by_id(&self, id: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    pub fn rule(&self, action: RuleAction) -> Option<&str> {
        match action {
            RuleAction::List => self.list_rule.as_deref(),
            RuleAction::View => self.view_rule.as_deref(),
            RuleAction::Create => self.create_rule.as_deref(),
            RuleAction::Update => self.update_rule.as_deref(),
            RuleAction::Delete => self.delete_rule.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAction {
    List,
    View,
    Create,
    Update,
    Delete,
}

impl RuleAction {
    pub const ALL: [RuleAction; 5] = [
        RuleAction::List,
        RuleAction::View,
        RuleAction::Create,
        RuleAction::Update,
        RuleAction::Delete,
    ];

    pub fn json_name(self) -> &'static str {
        match self {
            RuleAction::List => "listRule",
            RuleAction::View => "viewRule",
            RuleAction::Create => "createRule",
            RuleAction::Update => "updateRule",
            RuleAction::Delete => "deleteRule",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub presentable: bool,
    #[serde(default)]
    pub help: String,
    #[serde(flatten)]
    pub kind: FieldKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum FieldKind {
    Text(TextOptions),
    Editor(EditorOptions),
    Number(NumberOptions),
    Bool(BoolOptions),
    Email(DomainOptions),
    Url(DomainOptions),
    Date(DateOptions),
    Autodate(AutodateOptions),
    Select(SelectOptions),
    Json(JsonOptions),
    Relation(RelationOptions),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TextOptions {
    pub min: usize,
    /// 0 means no limit.
    pub max: usize,
    pub pattern: String,
    pub autogenerate_pattern: String,
    pub required: bool,
    pub primary_key: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EditorOptions {
    /// Max size in bytes, 0 means the default limit.
    pub max_size: i64,
    #[serde(rename = "convertURLs")]
    pub convert_urls: bool,
    pub required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NumberOptions {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub only_int: bool,
    pub required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BoolOptions {
    pub required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DomainOptions {
    pub except_domains: Vec<String>,
    pub only_domains: Vec<String>,
    pub required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DateOptions {
    /// Empty or a date string.
    pub min: String,
    pub max: String,
    pub required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AutodateOptions {
    pub on_create: bool,
    pub on_update: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SelectOptions {
    pub values: Vec<String>,
    pub max_select: usize,
    pub required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JsonOptions {
    /// Max size in bytes, 0 means the default limit.
    pub max_size: i64,
    pub required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RelationOptions {
    pub collection_id: String,
    pub cascade_delete: bool,
    pub min_select: usize,
    pub max_select: usize,
    pub required: bool,
}

/// How a field is stored and compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// `TEXT NOT NULL DEFAULT ''`
    Text,
    /// `DOUBLE PRECISION NOT NULL DEFAULT 0`
    Number,
    /// `BOOLEAN NOT NULL DEFAULT FALSE`
    Bool,
    /// `TIMESTAMPTZ`, NULL when empty
    Date,
    /// `JSONB NOT NULL DEFAULT '[]'`, an array of strings
    Multi,
    /// `JSONB`, any JSON value or NULL
    Json,
}

impl Field {
    pub fn type_name(&self) -> &'static str {
        match self.kind {
            FieldKind::Text(_) => "text",
            FieldKind::Editor(_) => "editor",
            FieldKind::Number(_) => "number",
            FieldKind::Bool(_) => "bool",
            FieldKind::Email(_) => "email",
            FieldKind::Url(_) => "url",
            FieldKind::Date(_) => "date",
            FieldKind::Autodate(_) => "autodate",
            FieldKind::Select(_) => "select",
            FieldKind::Json(_) => "json",
            FieldKind::Relation(_) => "relation",
        }
    }

    pub fn is_primary_key(&self) -> bool {
        matches!(&self.kind, FieldKind::Text(o) if o.primary_key)
    }

    pub fn is_multi(&self) -> bool {
        match &self.kind {
            FieldKind::Select(o) => o.max_select > 1,
            FieldKind::Relation(o) => o.max_select > 1,
            _ => false,
        }
    }

    pub fn storage(&self) -> Storage {
        match &self.kind {
            FieldKind::Text(_) | FieldKind::Editor(_) | FieldKind::Email(_) | FieldKind::Url(_) => {
                Storage::Text
            }
            FieldKind::Number(_) => Storage::Number,
            FieldKind::Bool(_) => Storage::Bool,
            FieldKind::Date(_) | FieldKind::Autodate(_) => Storage::Date,
            FieldKind::Select(_) | FieldKind::Relation(_) => {
                if self.is_multi() {
                    Storage::Multi
                } else {
                    Storage::Text
                }
            }
            FieldKind::Json(_) => Storage::Json,
        }
    }

    /// The PostgreSQL column definition (without the name).
    pub fn column_type(&self) -> &'static str {
        if self.is_primary_key() {
            return "TEXT PRIMARY KEY";
        }
        // autodate values are set by slonbase (like PocketBase, an update-only
        // autodate field stays empty on create), so they have no default
        match self.storage() {
            Storage::Text => "TEXT NOT NULL DEFAULT ''",
            Storage::Number => "DOUBLE PRECISION NOT NULL DEFAULT 0",
            Storage::Bool => "BOOLEAN NOT NULL DEFAULT FALSE",
            Storage::Date => "TIMESTAMPTZ",
            Storage::Multi => "JSONB NOT NULL DEFAULT '[]'",
            Storage::Json => "JSONB",
        }
    }

    pub fn required(&self) -> bool {
        match &self.kind {
            FieldKind::Text(o) => o.required,
            FieldKind::Editor(o) => o.required,
            FieldKind::Number(o) => o.required,
            FieldKind::Bool(o) => o.required,
            FieldKind::Email(o) | FieldKind::Url(o) => o.required,
            FieldKind::Date(o) => o.required,
            FieldKind::Autodate(_) => false,
            FieldKind::Select(o) => o.required,
            FieldKind::Json(o) => o.required,
            FieldKind::Relation(o) => o.required,
        }
    }
}

/// The default primary key field (matches PocketBase's).
pub fn default_id_field() -> Field {
    Field {
        id: "text3208210256".into(),
        name: "id".into(),
        system: true,
        hidden: false,
        presentable: false,
        help: String::new(),
        kind: FieldKind::Text(TextOptions {
            min: 15,
            max: 15,
            pattern: "^[a-z0-9]+$".into(),
            autogenerate_pattern: "[a-z0-9]{15}".into(),
            required: true,
            primary_key: true,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrips_pocketbase_json() {
        let input = json!({
            "id": "pbc_123",
            "name": "posts",
            "type": "base",
            "system": false,
            "fields": [
                {
                    "id": "text3208210256", "name": "id", "type": "text", "system": true,
                    "hidden": false, "presentable": false, "help": "",
                    "min": 15, "max": 15, "pattern": "^[a-z0-9]+$",
                    "autogeneratePattern": "[a-z0-9]{15}", "required": true, "primaryKey": true
                },
                {
                    "id": "number1", "name": "views", "type": "number", "system": false,
                    "hidden": false, "presentable": false, "help": "",
                    "min": null, "max": 100.0, "onlyInt": true, "required": false
                },
                {
                    "id": "select1", "name": "tags", "type": "select", "system": false,
                    "hidden": false, "presentable": false, "help": "",
                    "values": ["a", "b"], "maxSelect": 2, "required": false
                },
                {
                    "id": "editor1", "name": "body", "type": "editor", "system": false,
                    "hidden": false, "presentable": false, "help": "",
                    "maxSize": 0, "convertURLs": false, "required": false
                }
            ],
            "indexes": [],
            "listRule": "",
            "viewRule": null,
            "createRule": null,
            "updateRule": null,
            "deleteRule": null,
            "created": "",
            "updated": ""
        });

        let collection: Collection = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(collection.fields.len(), 4);
        assert_eq!(collection.list_rule.as_deref(), Some(""));
        assert_eq!(collection.view_rule, None);
        assert!(collection.fields[0].is_primary_key());
        assert!(collection.fields[2].is_multi());
        assert_eq!(collection.fields[2].storage(), Storage::Multi);

        assert_eq!(serde_json::to_value(&collection).unwrap(), input);
    }

    #[test]
    fn minimal_input_uses_defaults() {
        let collection: Collection = serde_json::from_value(json!({
            "name": "notes",
            "fields": [{ "name": "title", "type": "text" }]
        }))
        .unwrap();
        assert_eq!(collection.kind, CollectionKind::Base);
        assert_eq!(collection.list_rule, None);
        assert_eq!(
            collection.fields[0].kind,
            FieldKind::Text(TextOptions::default())
        );
    }

    #[test]
    fn rejects_unknown_types() {
        let result: Result<Collection, _> = serde_json::from_value(json!({
            "name": "x", "type": "view"
        }));
        assert!(result.is_err());
        let result: Result<Field, _> = serde_json::from_value(json!({
            "name": "x", "type": "nope"
        }));
        assert!(result.is_err());
    }
}
