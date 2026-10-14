//! Collection and field definitions, (de)serialized in PocketBase's JSON format.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Id PocketBase uses for its superusers collection (kept for SDK compatibility).
pub const SUPERUSERS_COLLECTION_ID: &str = "pbc_3142635823";
pub const SUPERUSERS_COLLECTION_NAME: &str = "_superusers";

/// A collection. Auth collections carry [`AuthOptions`], which PocketBase serializes
/// as top-level keys, so (de)serialization goes through [`RawCollection`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawCollection", into = "RawCollection")]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub kind: CollectionKind,
    pub system: bool,
    pub fields: Vec<Field>,
    pub indexes: Vec<String>,
    pub list_rule: Option<String>,
    pub view_rule: Option<String>,
    pub create_rule: Option<String>,
    pub update_rule: Option<String>,
    pub delete_rule: Option<String>,
    pub created: String,
    pub updated: String,
    /// Set for auth collections (normalized by validation).
    pub auth: Option<AuthOptions>,
    /// The `SELECT` of view collections.
    pub view_query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCollection {
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
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TryFrom<RawCollection> for Collection {
    type Error = serde_json::Error;

    fn try_from(raw: RawCollection) -> Result<Self, Self::Error> {
        let view_query = match raw.kind {
            CollectionKind::View => Some(
                raw.extra
                    .get("viewQuery")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            _ => None,
        };
        let auth = match raw.kind {
            CollectionKind::Auth => Some(serde_json::from_value(Value::Object(raw.extra))?),
            CollectionKind::Base | CollectionKind::View => None,
        };
        Ok(Self {
            id: raw.id,
            name: raw.name,
            kind: raw.kind,
            system: raw.system,
            fields: raw.fields,
            indexes: raw.indexes,
            list_rule: raw.list_rule,
            view_rule: raw.view_rule,
            create_rule: raw.create_rule,
            update_rule: raw.update_rule,
            delete_rule: raw.delete_rule,
            created: raw.created,
            updated: raw.updated,
            auth,
            view_query,
        })
    }
}

impl From<Collection> for RawCollection {
    fn from(c: Collection) -> Self {
        let mut extra = match c.auth {
            Some(auth) => match serde_json::to_value(auth) {
                Ok(Value::Object(map)) => map,
                _ => Map::new(),
            },
            None => Map::new(),
        };
        if let Some(query) = c.view_query {
            extra.insert("viewQuery".into(), Value::String(query));
        }
        Self {
            id: c.id,
            name: c.name,
            kind: c.kind,
            system: c.system,
            fields: c.fields,
            indexes: c.indexes,
            list_rule: c.list_rule,
            view_rule: c.view_rule,
            create_rule: c.create_rule,
            update_rule: c.update_rule,
            delete_rule: c.delete_rule,
            created: c.created,
            updated: c.updated,
            extra,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollectionKind {
    #[default]
    Base,
    Auth,
    /// Read-only records from a SQL query.
    View,
}

/// Options of auth collections (PocketBase's names and defaults).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AuthOptions {
    /// Extra condition for signing in: `null` disables auth, `""` allows any record.
    pub auth_rule: Option<String>,
    /// Who may manage other records of this collection (change their email,
    /// password or verified state without knowing the password).
    pub manage_rule: Option<String>,
    pub password_auth: PasswordAuthOptions,
    pub auth_token: TokenOptions,
    pub password_reset_token: TokenOptions,
    pub email_change_token: TokenOptions,
    pub verification_token: TokenOptions,
    pub file_token: TokenOptions,
    pub verification_template: EmailTemplate,
    pub reset_password_template: EmailTemplate,
    pub confirm_email_change_template: EmailTemplate,
    pub oauth2: OAuth2Options,
    pub mfa: MfaOptions,
    pub otp: OtpOptions,
    /// Emails the record when it signs in from a new device.
    pub auth_alert: AuthAlertOptions,
}

impl Default for AuthOptions {
    fn default() -> Self {
        let duration = |d| TokenOptions { duration: d };
        Self {
            auth_rule: Some(String::new()),
            manage_rule: None,
            password_auth: PasswordAuthOptions::default(),
            auth_token: duration(432_000),
            password_reset_token: duration(1800),
            email_change_token: duration(1800),
            verification_token: duration(86_400),
            file_token: duration(180),
            verification_template: EmailTemplate {
                subject: "Verify your {APP_NAME} email".into(),
                body: "<p>Hello,</p>\n<p>Thank you for joining us at {APP_NAME}.</p>\n\
                       <p>Click on the link below to verify your email address.</p>\n\
                       <p><a href=\"{APP_URL}/_/#/auth/confirm-verification/{TOKEN}\">Verify</a></p>\n\
                       <p>Thanks,<br/>\n{APP_NAME} team</p>"
                    .into(),
            },
            reset_password_template: EmailTemplate {
                subject: "Reset your {APP_NAME} password".into(),
                body: "<p>Hello,</p>\n<p>Click on the link below to reset your password.</p>\n\
                       <p><a href=\"{APP_URL}/_/#/auth/confirm-password-reset/{TOKEN}\">Reset password</a></p>\n\
                       <p><i>If you didn't ask to reset your password, you can ignore this email.</i></p>\n\
                       <p>Thanks,<br/>\n{APP_NAME} team</p>"
                    .into(),
            },
            confirm_email_change_template: EmailTemplate {
                subject: "Confirm your {APP_NAME} new email address".into(),
                body: "<p>Hello,</p>\n<p>Click on the link below to confirm your new email address.</p>\n\
                       <p><a href=\"{APP_URL}/_/#/auth/confirm-email-change/{TOKEN}\">Confirm new email</a></p>\n\
                       <p><i>If you didn't ask to change your email address, you can ignore this email.</i></p>\n\
                       <p>Thanks,<br/>\n{APP_NAME} team</p>"
                    .into(),
            },
            auth_alert: AuthAlertOptions::default(),
            oauth2: OAuth2Options::default(),
            mfa: MfaOptions::default(),
            otp: OtpOptions::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OAuth2Options {
    pub enabled: bool,
    pub providers: Vec<OAuth2Provider>,
    /// Record fields filled from the provider's user on sign-up.
    pub mapped_fields: OAuth2MappedFields,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OAuth2Provider {
    /// A preset (`google`, `github`, `gitlab`, `discord`, `microsoft`, `facebook`) or
    /// `oidc`, `oidc2`, `oidc3` for any OpenID Connect provider.
    pub name: String,
    pub client_id: String,
    /// Never returned by the API; an empty value on update keeps the stored one.
    pub client_secret: String,
    #[serde(rename = "authURL")]
    pub auth_url: String,
    #[serde(rename = "tokenURL")]
    pub token_url: String,
    #[serde(rename = "userInfoURL")]
    pub user_info_url: String,
    pub display_name: String,
    pub pkce: Option<bool>,
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OAuth2MappedFields {
    pub id: String,
    pub name: String,
    pub username: String,
    #[serde(rename = "avatarURL")]
    pub avatar_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MfaOptions {
    pub enabled: bool,
    /// Seconds to complete the second method.
    pub duration: i64,
    /// Records that need MFA (empty: all).
    pub rule: String,
}

impl Default for MfaOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            duration: 600,
            rule: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OtpOptions {
    pub enabled: bool,
    /// Seconds an OTP is valid.
    pub duration: i64,
    /// Number of digits.
    pub length: usize,
    pub email_template: EmailTemplate,
}

impl Default for OtpOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            duration: 180,
            length: 8,
            email_template: EmailTemplate {
                subject: "OTP for {APP_NAME}".into(),
                body: "<p>Hello,</p>\n<p>Your one-time password is: <strong>{OTP}</strong></p>\n\
                       <p><i>If you didn't ask for the one-time password, you can ignore this email.</i></p>\n\
                       <p>Thanks,<br/>\n{APP_NAME} team</p>"
                    .into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AuthAlertOptions {
    pub enabled: bool,
    #[serde(deserialize_with = "alert_template")]
    pub email_template: EmailTemplate,
}

/// A blank alert template (collections stored before alerts were implemented, or imported
/// without one) is the default one.
fn alert_template<'de, D: serde::Deserializer<'de>>(d: D) -> Result<EmailTemplate, D::Error> {
    let t = EmailTemplate::deserialize(d)?;
    if t.subject.trim().is_empty() && t.body.trim().is_empty() {
        return Ok(AuthAlertOptions::default().email_template);
    }
    Ok(t)
}

impl Default for AuthAlertOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            email_template: EmailTemplate {
                subject: "Login from a new location".into(),
                body: "<p>Hello,</p>\n<p>We noticed a login to your {APP_NAME} account from a new location:</p>\n\
                       <p><em>{ALERT_INFO}</em></p>\n\
                       <p><strong>If this wasn't you, you should immediately change your {APP_NAME} account password to revoke access from all other locations.</strong></p>\n\
                       <p>If this was you, you may disregard this email.</p>\n\
                       <p>Thanks,<br/>\n{APP_NAME} team</p>"
                    .into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PasswordAuthOptions {
    pub enabled: bool,
    pub identity_fields: Vec<String>,
}

impl Default for PasswordAuthOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            identity_fields: vec!["email".into()],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TokenOptions {
    /// Validity in seconds.
    pub duration: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EmailTemplate {
    pub subject: String,
    pub body: String,
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
    Password(PasswordOptions),
    File(FileOptions),
    #[serde(rename = "geoPoint")]
    GeoPoint(GeoPointOptions),
}

/// A point as `{"lon": ..., "lat": ...}` (PocketBase's geoPoint field).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeoPointOptions {
    /// Required points can't be `{"lon": 0, "lat": 0}`.
    pub required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FileOptions {
    pub max_select: usize,
    /// Max size per file in bytes, 0 means the default (5 MiB).
    pub max_size: i64,
    /// Allowed MIME types (detected from the content); empty allows any.
    pub mime_types: Vec<String>,
    /// Thumbnail sizes that may be requested: `WxH`, `WxHt` (top), `WxHb` (bottom),
    /// `WxHf` (fit), `0xH` / `Wx0` (keep the aspect ratio).
    pub thumbs: Vec<String>,
    /// Protected files need a file token and the record's view rule.
    pub protected: bool,
    pub required: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PasswordOptions {
    pub min: usize,
    /// 0 means the bcrypt limit (71 bytes).
    pub max: usize,
    pub pattern: String,
    /// bcrypt cost, 0 means the default (10).
    pub cost: u32,
    pub required: bool,
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
            FieldKind::Password(_) => "password",
            FieldKind::File(_) => "file",
            FieldKind::GeoPoint(_) => "geoPoint",
        }
    }

    pub fn is_primary_key(&self) -> bool {
        matches!(&self.kind, FieldKind::Text(o) if o.primary_key)
    }

    pub fn is_multi(&self) -> bool {
        match &self.kind {
            FieldKind::Select(o) => o.max_select > 1,
            FieldKind::Relation(o) => o.max_select > 1,
            FieldKind::File(o) => o.max_select > 1,
            _ => false,
        }
    }

    pub fn storage(&self) -> Storage {
        match &self.kind {
            FieldKind::Text(_)
            | FieldKind::Editor(_)
            | FieldKind::Email(_)
            | FieldKind::Url(_)
            | FieldKind::Password(_) => Storage::Text,
            FieldKind::Number(_) => Storage::Number,
            FieldKind::Bool(_) => Storage::Bool,
            FieldKind::Date(_) | FieldKind::Autodate(_) => Storage::Date,
            FieldKind::Select(_) | FieldKind::Relation(_) | FieldKind::File(_) => {
                if self.is_multi() {
                    Storage::Multi
                } else {
                    Storage::Text
                }
            }
            FieldKind::Json(_) | FieldKind::GeoPoint(_) => Storage::Json,
        }
    }

    /// The PostgreSQL column definition (without the name).
    pub fn column_type(&self) -> &'static str {
        if self.is_primary_key() {
            return "TEXT PRIMARY KEY";
        }
        if matches!(self.kind, FieldKind::GeoPoint(_)) {
            return r#"JSONB NOT NULL DEFAULT '{"lon":0,"lat":0}'"#;
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
            FieldKind::Password(o) => o.required,
            FieldKind::File(o) => o.required,
            FieldKind::GeoPoint(o) => o.required,
        }
    }
}

pub const FIELD_PASSWORD: &str = "password";
pub const FIELD_TOKEN_KEY: &str = "tokenKey";
pub const FIELD_EMAIL: &str = "email";
pub const FIELD_EMAIL_VISIBILITY: &str = "emailVisibility";
pub const FIELD_VERIFIED: &str = "verified";

impl Collection {
    pub fn is_auth(&self) -> bool {
        self.kind == CollectionKind::Auth
    }

    pub fn is_view(&self) -> bool {
        self.kind == CollectionKind::View
    }

    /// The auth options, for auth collections.
    pub fn auth_options(&self) -> Option<&AuthOptions> {
        self.auth.as_ref().filter(|_| self.is_auth())
    }
}

impl Field {
    /// Never returned in API responses, whoever asks (password hashes, token keys).
    pub fn is_secret(&self) -> bool {
        matches!(self.kind, FieldKind::Password(_)) || (self.system && self.name == FIELD_TOKEN_KEY)
    }
}

/// The system fields every auth collection has (PocketBase's definitions).
pub fn default_auth_fields() -> Vec<Field> {
    let field = |name: &str, kind: FieldKind, hidden: bool| Field {
        id: String::new(),
        name: name.into(),
        system: true,
        hidden,
        presentable: false,
        help: String::new(),
        kind,
    };
    vec![
        field(
            FIELD_PASSWORD,
            FieldKind::Password(PasswordOptions {
                min: 8,
                required: true,
                ..Default::default()
            }),
            true,
        ),
        field(
            FIELD_TOKEN_KEY,
            FieldKind::Text(TextOptions {
                min: 30,
                max: 60,
                autogenerate_pattern: "[a-zA-Z0-9]{50}".into(),
                required: true,
                ..Default::default()
            }),
            true,
        ),
        field(
            FIELD_EMAIL,
            FieldKind::Email(DomainOptions {
                required: true,
                ..Default::default()
            }),
            false,
        ),
        field(
            FIELD_EMAIL_VISIBILITY,
            FieldKind::Bool(BoolOptions::default()),
            false,
        ),
        field(
            FIELD_VERIFIED,
            FieldKind::Bool(BoolOptions::default()),
            false,
        ),
    ]
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
            "name": "x", "type": "nope"
        }));
        assert!(result.is_err());
        let view: Collection = serde_json::from_value(json!({
            "name": "x", "type": "view", "viewQuery": "SELECT id FROM posts"
        }))
        .unwrap();
        assert_eq!(view.view_query.as_deref(), Some("SELECT id FROM posts"));
        assert_eq!(
            serde_json::to_value(&view).unwrap()["viewQuery"],
            "SELECT id FROM posts"
        );
        let result: Result<Field, _> = serde_json::from_value(json!({
            "name": "x", "type": "nope"
        }));
        assert!(result.is_err());
    }
}
