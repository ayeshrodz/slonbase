//! Collection validation and normalization before a save.

use std::collections::HashSet;

use super::indexes::Index;
use super::model::*;
use super::store::Registry;
use crate::filter::{Params, RequestInfo, Scope, to_sql};
use crate::validation::Errors;
use crate::{datetime, ids};

pub const MAX_FIELDS: usize = 500;
/// PostgreSQL identifiers are limited to 63 bytes.
pub const MAX_NAME_LENGTH: usize = 63;
const RESERVED_FIELD_NAMES: &[&str] = &[
    "collectionid",
    "collectionname",
    "expand",
    "null",
    "true",
    "false",
    // request body keys of auth collections
    "passwordconfirm",
    "oldpassword",
];

/// Token durations must be between 10 seconds and 1 year.
const MIN_TOKEN_DURATION: i64 = 10;
const MAX_TOKEN_DURATION: i64 = 365 * 86_400;

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LENGTH
        && !name.starts_with('_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Normalizes `c` (ids, defaults, canonical indexes) and validates it.
/// `old` is the stored version when updating.
pub fn prepare(c: &mut Collection, old: Option<&Collection>, registry: &Registry) -> Errors {
    prepare_inner(c, old, registry, false)
}

/// Like [`prepare`], for a new system collection (whose name starts with `_`).
pub fn prepare_system(c: &mut Collection, registry: &Registry) -> Errors {
    prepare_inner(c, None, registry, true)
}

fn prepare_inner(
    c: &mut Collection,
    old: Option<&Collection>,
    registry: &Registry,
    new_system: bool,
) -> Errors {
    let mut errors = Errors::new();

    // identity
    match old {
        Some(old) => {
            c.id = old.id.clone();
            c.system = old.system;
            c.created = old.created.clone();
        }
        None => {
            if c.id.is_empty() {
                c.id = format!("pbc_{}", ids::random_id(10));
            } else if c.id.len() > 100
                || !c
                    .id
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                errors.invalid("id", "Must contain only letters, digits and underscores.");
            } else if registry.by_id(&c.id).is_some() {
                errors.invalid("id", "The collection id is already in use.");
            }
            c.system = new_system;
        }
    }

    c.name = c.name.trim().to_string();
    let system_name = c.system && old.is_none_or(|old| old.name == c.name);
    if old.is_some_and(|old| old.system && old.name != c.name) {
        errors.invalid("name", "System collections can't be renamed.");
    } else if c.name.is_empty() {
        errors.add("name", "validation_required", "Cannot be blank.");
    } else if !system_name && !valid_name(&c.name) {
        errors.invalid(
            "name",
            format!(
                "Must contain only letters, digits and underscores, must not start with \
                 an underscore and must be at most {MAX_NAME_LENGTH} characters."
            ),
        );
    } else if registry
        .all()
        .iter()
        .any(|other| other.id != c.id && other.name.eq_ignore_ascii_case(&c.name))
    {
        errors.invalid(
            "name",
            "The collection name must be unique (case insensitive).",
        );
    }

    if let Some(old) = old {
        if old.kind != c.kind {
            errors.invalid("type", "The collection type can't be changed.");
        }
    }
    if c.is_view() {
        // fields come from the query (see `view::infer_fields`)
        if let Err(message) = super::view::check_query(c.view_query.as_deref().unwrap_or_default())
        {
            errors.invalid("viewQuery", message);
        }
        c.create_rule = None;
        c.update_rule = None;
        c.delete_rule = None;
        if !c.indexes.is_empty() {
            errors.invalid("indexes", "View collections can't have indexes.");
        }
    } else {
        c.view_query = None;
    }
    prepare_auth(c, &mut errors);
    keep_client_secrets(c, old);
    prepare_fields(c, old, registry, &mut errors);
    validate_rules(c, registry, &mut errors);
    prepare_indexes(c, old, registry, &mut errors);
    validate_auth_options(c, registry, &mut errors);

    errors
}

/// OAuth2 client secrets aren't returned by the API, so an empty one on update keeps the
/// stored secret of the provider with the same name.
fn keep_client_secrets(c: &mut Collection, old: Option<&Collection>) {
    let (Some(auth), Some(old)) = (c.auth.as_mut(), old.and_then(|o| o.auth_options())) else {
        return;
    };
    for provider in &mut auth.oauth2.providers {
        if provider.client_secret.is_empty() {
            if let Some(stored) = old
                .oauth2
                .providers
                .iter()
                .find(|p| p.name == provider.name)
            {
                provider.client_secret = stored.client_secret.clone();
            }
        }
    }
}

/// Adds the auth system fields and default options to auth collections.
fn prepare_auth(c: &mut Collection, errors: &mut Errors) {
    if !c.is_auth() {
        c.auth = None;
        if c.fields
            .iter()
            .any(|f| matches!(f.kind, FieldKind::Password(_)))
        {
            errors.invalid(
                "fields",
                "Password fields can only be used in auth collections.",
            );
        }
        return;
    }
    if c.auth.is_none() {
        c.auth = Some(AuthOptions::default());
    }
    if c.field("id").is_none() {
        c.fields.insert(0, default_id_field());
    }
    for default in default_auth_fields() {
        match c
            .fields
            .iter_mut()
            .find(|f| f.name.eq_ignore_ascii_case(&default.name))
        {
            None => c.fields.push(default),
            Some(existing) => {
                if existing.type_name() != default.type_name() {
                    errors.invalid(
                        "fields",
                        format!(
                            "The {:?} field must be of type {}.",
                            default.name,
                            default.type_name()
                        ),
                    );
                    continue;
                }
                existing.name = default.name.clone();
                existing.system = true;
                existing.hidden = default.hidden;
                match &mut existing.kind {
                    FieldKind::Password(o) => o.required = true,
                    FieldKind::Text(o) if existing.name == FIELD_TOKEN_KEY => {
                        o.required = true;
                        if o.autogenerate_pattern.is_empty() {
                            o.autogenerate_pattern = "[a-zA-Z0-9]{50}".into();
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let passwords = c
        .fields
        .iter()
        .filter(|f| matches!(f.kind, FieldKind::Password(_)))
        .count();
    if passwords > 1 {
        errors.invalid(
            "fields",
            "Auth collections can only have the system password field.",
        );
    }
}

fn validate_auth_options(c: &Collection, registry: &Registry, errors: &mut Errors) {
    let Some(auth) = c.auth_options() else { return };

    let tokens = [
        ("authToken", &auth.auth_token),
        ("passwordResetToken", &auth.password_reset_token),
        ("emailChangeToken", &auth.email_change_token),
        ("verificationToken", &auth.verification_token),
        ("fileToken", &auth.file_token),
    ];
    for (name, token) in tokens {
        if !(MIN_TOKEN_DURATION..=MAX_TOKEN_DURATION).contains(&token.duration) {
            errors.invalid(
                format!("{name}.duration"),
                format!("Must be between {MIN_TOKEN_DURATION} and {MAX_TOKEN_DURATION} seconds."),
            );
        }
    }

    // identity fields must be unique, otherwise a login could match several records
    let unique_columns: Vec<String> = c
        .indexes
        .iter()
        .filter_map(|raw| Index::parse(raw).ok())
        .filter(|i| i.unique && i.columns.len() == 1)
        .map(|i| i.columns[0].0.to_ascii_lowercase())
        .collect();
    if auth.password_auth.identity_fields.is_empty() && auth.password_auth.enabled {
        errors.add(
            "passwordAuth.identityFields",
            "validation_required",
            "Cannot be blank.",
        );
    }
    for name in &auth.password_auth.identity_fields {
        let ok = match c.field(name) {
            Some(f) if f.name == FIELD_EMAIL => true,
            Some(f) if f.storage() == Storage::Text && !f.is_secret() => {
                unique_columns.contains(&f.name.to_ascii_lowercase())
            }
            _ => false,
        };
        if !ok {
            errors.invalid(
                "passwordAuth.identityFields",
                format!("{name:?} must be a text field with a unique index."),
            );
        }
    }

    let mut request = RequestInfo::new("GET");
    request.registry = Some(std::sync::Arc::new(registry.clone()));
    let scope = Scope {
        collection: c,
        alias: "t",
        request: &request,
        allow_hidden: true,
    };
    for (key, rule) in [
        ("authRule", auth.auth_rule.as_deref()),
        ("manageRule", auth.manage_rule.as_deref()),
        ("mfa.rule", Some(auth.mfa.rule.as_str())),
    ] {
        if let Some(rule) = rule.filter(|r| !r.trim().is_empty()) {
            if let Err(e) = to_sql(rule, &scope, &mut Params::default()) {
                errors.invalid(key, format!("Invalid rule: {e}."));
            }
        }
    }

    validate_auth_methods(c, auth, errors);
}

const MIN_AUTH_METHOD_DURATION: i64 = 10;
const MAX_AUTH_METHOD_DURATION: i64 = 86_400;

fn valid_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url.contains(char::is_whitespace)
}

fn validate_auth_methods(c: &Collection, auth: &AuthOptions, errors: &mut Errors) {
    let duration_range = MIN_AUTH_METHOD_DURATION..=MAX_AUTH_METHOD_DURATION;
    let duration_message = format!(
        "Must be between {MIN_AUTH_METHOD_DURATION} and {MAX_AUTH_METHOD_DURATION} seconds."
    );

    if auth.otp.enabled {
        if !duration_range.contains(&auth.otp.duration) {
            errors.invalid("otp.duration", duration_message.clone());
        }
        if !(4..=32).contains(&auth.otp.length) {
            errors.invalid("otp.length", "Must be between 4 and 32.");
        }
        if auth.otp.email_template.subject.trim().is_empty() {
            errors.add(
                "otp.emailTemplate.subject",
                "validation_required",
                "Cannot be blank.",
            );
        }
        if !auth.otp.email_template.body.contains("{OTP}") {
            errors.invalid(
                "otp.emailTemplate.body",
                "Must contain the {OTP} placeholder.",
            );
        }
    }

    if auth.auth_alert.enabled {
        let t = &auth.auth_alert.email_template;
        for (key, value) in [("subject", &t.subject), ("body", &t.body)] {
            if value.trim().is_empty() {
                errors.add(
                    format!("authAlert.emailTemplate.{key}"),
                    "validation_required",
                    "Cannot be blank.",
                );
            }
        }
    }

    if auth.mfa.enabled {
        if !duration_range.contains(&auth.mfa.duration) {
            errors.invalid("mfa.duration", duration_message);
        }
        let methods = [
            auth.password_auth.enabled,
            auth.oauth2.enabled,
            auth.otp.enabled,
        ];
        if methods.iter().filter(|enabled| **enabled).count() < 2 {
            errors.invalid(
                "mfa.enabled",
                "MFA requires at least 2 enabled authentication methods.",
            );
        }
    }

    let oauth2 = &auth.oauth2;
    let mut names = HashSet::new();
    for (i, provider) in oauth2.providers.iter().enumerate() {
        let key = |name: &str| format!("oauth2.providers.{i}.{name}");
        if !crate::oauth2::is_known(&provider.name) {
            errors.invalid(
                key("name"),
                format!("Unknown provider {:?}.", provider.name),
            );
            continue;
        }
        if !names.insert(provider.name.as_str()) {
            errors.invalid(key("name"), "Each provider can only be added once.");
        }
        if provider.client_id.trim().is_empty() {
            errors.add(key("clientId"), "validation_required", "Cannot be blank.");
        }
        if provider.client_secret.trim().is_empty() {
            errors.add(
                key("clientSecret"),
                "validation_required",
                "Cannot be blank.",
            );
        }
        for (name, url) in [
            ("authURL", &provider.auth_url),
            ("tokenURL", &provider.token_url),
            ("userInfoURL", &provider.user_info_url),
        ] {
            if url.is_empty() {
                if crate::oauth2::needs_urls(&provider.name) {
                    errors.add(key(name), "validation_required", "Cannot be blank.");
                }
            } else if !valid_url(url) {
                errors.invalid(key(name), "Must be an http(s) URL.");
            }
        }
    }
    if oauth2.enabled && oauth2.providers.is_empty() {
        errors.add(
            "oauth2.providers",
            "validation_required",
            "Cannot be blank.",
        );
    }
    let mapped = &oauth2.mapped_fields;
    for (name, field) in [
        ("id", &mapped.id),
        ("name", &mapped.name),
        ("username", &mapped.username),
        ("avatarURL", &mapped.avatar_url),
    ] {
        let ok = field.is_empty()
            || c.field(field).is_some_and(|f| {
                !f.system
                    && match &f.kind {
                        FieldKind::File(_) => name == "avatarURL",
                        kind => {
                            f.storage() == Storage::Text && !matches!(kind, FieldKind::Password(_))
                        }
                    }
            });
        if !ok {
            errors.invalid(
                format!("oauth2.mappedFields.{name}"),
                format!("{field:?} must be a non-system text field of the collection."),
            );
        }
    }
}

fn prepare_fields(
    c: &mut Collection,
    old: Option<&Collection>,
    registry: &Registry,
    errors: &mut Errors,
) {
    if old.is_none() && c.field("id").is_none() {
        c.fields.insert(0, default_id_field());
    }
    if c.fields.len() > MAX_FIELDS {
        errors.invalid(
            "fields",
            format!("A collection can have at most {MAX_FIELDS} fields."),
        );
        return;
    }

    let mut names = HashSet::new();
    let mut field_ids = HashSet::new();
    let collection_id = c.id.clone();

    for (i, field) in c.fields.iter_mut().enumerate() {
        let path = |key: &str| format!("fields.{i}.{key}");

        if field.id.is_empty() {
            field.id = format!("{}{}", field.type_name(), ids::random_id(10));
        } else if field.id.len() > 100
            || !field
                .id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            errors.invalid(
                path("id"),
                "Must contain only letters, digits and underscores.",
            );
        }
        if !field_ids.insert(field.id.clone()) {
            errors.invalid(path("id"), "Duplicated field id.");
        }

        field.name = field.name.trim().to_string();
        let is_id_field = field.name == "id";
        if field.name.is_empty() {
            errors.add(path("name"), "validation_required", "Cannot be blank.");
        } else if !valid_name(&field.name) {
            errors.invalid(
                path("name"),
                "Must contain only letters, digits and underscores and must not start with an underscore.",
            );
        } else if RESERVED_FIELD_NAMES.contains(&field.name.to_ascii_lowercase().as_str()) {
            errors.invalid(path("name"), "The name is reserved.");
        } else if !names.insert(field.name.to_ascii_lowercase()) {
            errors.invalid(path("name"), "Duplicated field name.");
        }

        // the primary key is the "id" text field and nothing else
        if field.is_primary_key() != is_id_field {
            if is_id_field {
                errors.invalid(
                    path("type"),
                    "The id field must be a text field with primaryKey enabled.",
                );
            } else {
                errors.invalid(
                    path("primaryKey"),
                    "Only the id field can be the primary key.",
                );
            }
        }
        if is_id_field {
            field.system = true;
            field.hidden = false;
            if let FieldKind::Text(o) = &mut field.kind {
                o.required = true;
            }
        }

        validate_options(field, &collection_id, registry, &path, errors);

        // existing fields keep their type
        if let Some(previous) = old.and_then(|o| o.field_by_id(&field.id)) {
            if previous.type_name() != field.type_name() {
                errors.invalid(
                    path("type"),
                    "The type of an existing field can't be changed.",
                );
            } else if let (FieldKind::Relation(a), FieldKind::Relation(b)) =
                (&previous.kind, &field.kind)
            {
                if a.collection_id != b.collection_id {
                    errors.invalid(
                        path("collectionId"),
                        "The related collection of an existing field can't be changed.",
                    );
                }
            }
            if previous.system && previous.name != field.name {
                errors.invalid(path("name"), "System fields can't be renamed.");
            }
        }
    }

    if let Some(old) = old {
        for previous in old.fields.iter().filter(|f| f.system) {
            if c.field_by_id(&previous.id).is_none() {
                errors.invalid(
                    "fields",
                    format!("The system field {:?} can't be removed.", previous.name),
                );
            }
        }
    }
    if !c
        .fields
        .iter()
        .any(|f| f.name == "id" && f.is_primary_key())
    {
        errors.invalid(
            "fields",
            "The collection must have an \"id\" text primary key field.",
        );
    }
}

fn validate_options(
    field: &mut Field,
    collection_id: &str,
    registry: &Registry,
    path: &dyn Fn(&str) -> String,
    errors: &mut Errors,
) {
    match &mut field.kind {
        FieldKind::Text(o) => {
            if o.max > 0 && o.min > o.max {
                errors.invalid(path("min"), "Must be less than or equal to max.");
            }
            if !o.pattern.is_empty() && regex::Regex::new(&o.pattern).is_err() {
                errors.invalid(path("pattern"), "Invalid regular expression.");
            }
            if !o.autogenerate_pattern.is_empty()
                && !ids::is_supported_pattern(&o.autogenerate_pattern)
            {
                errors.invalid(
                    path("autogeneratePattern"),
                    "Only patterns like [a-z0-9]{15} (a character class and a length) are supported.",
                );
            }
        }
        FieldKind::Editor(o) => {
            if o.max_size < 0 {
                errors.invalid(path("maxSize"), "Must be 0 or more.");
            }
        }
        FieldKind::Json(o) => {
            if o.max_size < 0 {
                errors.invalid(path("maxSize"), "Must be 0 or more.");
            }
        }
        FieldKind::Number(o) => {
            if let (Some(min), Some(max)) = (o.min, o.max) {
                if min > max {
                    errors.invalid(path("min"), "Must be less than or equal to max.");
                }
            }
            if o.min.is_some_and(|v| !v.is_finite()) || o.max.is_some_and(|v| !v.is_finite()) {
                errors.invalid(path("min"), "Must be a finite number.");
            }
        }
        FieldKind::Email(o) | FieldKind::Url(o) => {
            if !o.except_domains.is_empty() && !o.only_domains.is_empty() {
                errors.invalid(
                    path("onlyDomains"),
                    "Can't be used together with exceptDomains.",
                );
            }
            if o.except_domains
                .iter()
                .chain(&o.only_domains)
                .any(|d| d.trim().is_empty())
            {
                errors.invalid(path("onlyDomains"), "Domains can't be blank.");
            }
        }
        FieldKind::Date(o) => {
            let mut parsed = [None, None];
            for (i, (key, value)) in [("min", &mut o.min), ("max", &mut o.max)]
                .into_iter()
                .enumerate()
            {
                if value.trim().is_empty() {
                    value.clear();
                    continue;
                }
                match datetime::parse(value) {
                    Some(dt) => {
                        *value = datetime::format(&dt);
                        parsed[i] = Some(dt);
                    }
                    None => errors.invalid(path(key), "Must be a valid date."),
                }
            }
            if let [Some(min), Some(max)] = parsed {
                if min > max {
                    errors.invalid(path("min"), "Must be before max.");
                }
            }
        }
        FieldKind::Select(o) => {
            if o.max_select == 0 {
                o.max_select = 1;
            }
            if o.values.is_empty() {
                errors.add(path("values"), "validation_required", "Cannot be blank.");
            }
            let mut seen = HashSet::new();
            for v in &o.values {
                if v.is_empty() || !seen.insert(v.as_str()) {
                    errors.invalid(path("values"), "Values must be unique and non-empty.");
                    break;
                }
            }
            if !o.values.is_empty() && o.max_select > o.values.len() {
                errors.invalid(
                    path("maxSelect"),
                    "Must be less than or equal to the number of values.",
                );
            }
        }
        FieldKind::Relation(o) => {
            if o.max_select == 0 {
                o.max_select = 1;
            }
            if o.min_select > o.max_select {
                errors.invalid(
                    path("minSelect"),
                    "Must be less than or equal to maxSelect.",
                );
            }
            if o.collection_id.is_empty() {
                errors.add(
                    path("collectionId"),
                    "validation_required",
                    "Cannot be blank.",
                );
            } else if o.collection_id != collection_id && registry.by_id(&o.collection_id).is_none()
            {
                errors.invalid(
                    path("collectionId"),
                    "The related collection doesn't exist.",
                );
            }
        }
        FieldKind::Password(o) => {
            if o.max > 0 && o.min > o.max {
                errors.invalid(path("min"), "Must be less than or equal to max.");
            }
            if o.max > 71 {
                errors.invalid(path("max"), "Must be at most 71 (the bcrypt limit).");
            }
            if o.cost != 0 && !(4..=31).contains(&o.cost) {
                errors.invalid(path("cost"), "Must be between 4 and 31.");
            }
            if !o.pattern.is_empty() && regex::Regex::new(&o.pattern).is_err() {
                errors.invalid(path("pattern"), "Invalid regular expression.");
            }
        }
        FieldKind::File(o) => {
            if o.max_select == 0 {
                o.max_select = 1;
            }
            if o.max_size < 0 {
                errors.invalid(path("maxSize"), "Must be 0 or more.");
            }
            if o.mime_types.iter().any(|m| !m.contains('/')) {
                errors.invalid(path("mimeTypes"), "Must be MIME types like image/png.");
            }
            if let Some(bad) = o
                .thumbs
                .iter()
                .find(|t| crate::records::files::ThumbSize::parse(t).is_none())
            {
                errors.invalid(
                    path("thumbs"),
                    format!("Invalid thumb size {bad:?} (e.g. 100x100, 0x300, 200x100f)."),
                );
            }
        }
        FieldKind::Bool(_) | FieldKind::Autodate(_) | FieldKind::GeoPoint(_) => {}
    }
}

fn validate_rules(c: &Collection, registry: &Registry, errors: &mut Errors) {
    let mut request = RequestInfo::new("GET");
    request.registry = Some(std::sync::Arc::new(registry.clone()));
    let scope = Scope {
        collection: c,
        alias: "t",
        request: &request,
        allow_hidden: true,
    };
    for action in RuleAction::ALL {
        if let Some(rule) = c.rule(action) {
            if rule.trim().is_empty() {
                continue;
            }
            if let Err(e) = to_sql(rule, &scope, &mut Params::default()) {
                errors.invalid(action.json_name(), format!("Invalid rule: {e}."));
            }
        }
    }
}

fn prepare_indexes(
    c: &mut Collection,
    old: Option<&Collection>,
    registry: &Registry,
    errors: &mut Errors,
) {
    let mut names = HashSet::new();
    let mut canonical = Vec::with_capacity(c.indexes.len());

    for (i, raw) in c.indexes.iter().enumerate() {
        let path = format!("indexes.{i}");
        let index = match Index::parse(raw) {
            Ok(index) => index,
            Err(e) => {
                errors.invalid(path, format!("Invalid index: {e}."));
                continue;
            }
        };

        // the table must be this collection (the old name is accepted on rename)
        let table = Index::table_of(raw).unwrap_or_default();
        let table_ok = table.eq_ignore_ascii_case(&c.name)
            || old.is_some_and(|o| table.eq_ignore_ascii_case(&o.name));
        if !table_ok {
            errors.invalid(
                path,
                format!("The index must be on the {:?} table.", c.name),
            );
            continue;
        }

        if index.name.starts_with('_') {
            errors.invalid(path, "Index names must not start with an underscore.");
            continue;
        }
        let lower = index.name.to_ascii_lowercase();
        let taken_elsewhere = registry.all().iter().filter(|o| o.id != c.id).any(|o| {
            o.name.eq_ignore_ascii_case(&index.name)
                || o.indexes.iter().any(|raw| {
                    Index::parse(raw).is_ok_and(|idx| idx.name.eq_ignore_ascii_case(&index.name))
                })
        });
        if !names.insert(lower) || taken_elsewhere || index.name.eq_ignore_ascii_case(&c.name) {
            errors.invalid(path, "The index name must be unique.");
            continue;
        }

        let mut index = index;
        let mut ok = true;
        for (column, _) in index.columns.iter_mut() {
            match c.field(column) {
                Some(f) if f.storage() != Storage::Json => *column = f.name.clone(),
                Some(_) => {
                    errors.invalid(&path, format!("JSON fields can't be indexed ({column:?})."));
                    ok = false;
                }
                None => {
                    errors.invalid(&path, format!("Unknown column {column:?}."));
                    ok = false;
                }
            }
        }
        if ok {
            canonical.push(index.to_pocketbase_sql(&c.name));
        }
    }

    if errors.iter().all(|(p, _)| !p.starts_with("indexes")) {
        c.indexes = canonical;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> Collection {
        serde_json::from_value(value).unwrap()
    }

    fn paths(errors: &Errors) -> Vec<String> {
        errors.iter().map(|(p, _)| p.clone()).collect()
    }

    #[test]
    fn normalizes_a_minimal_collection() {
        let mut c = parse(json!({
            "name": " posts ",
            "fields": [
                { "name": "title", "type": "text" },
                { "name": "tags", "type": "select", "values": ["a", "b"] }
            ],
            "indexes": ["create unique index idx_title on posts (TITLE)"]
        }));
        let errors = prepare(&mut c, None, &Registry::default());
        assert!(errors.is_empty(), "{errors:?}");

        assert!(c.id.starts_with("pbc_"));
        assert_eq!(c.name, "posts");
        assert_eq!(c.fields[0].name, "id");
        assert!(c.fields.iter().all(|f| !f.id.is_empty()));
        assert_eq!(
            c.indexes,
            vec!["CREATE UNIQUE INDEX `idx_title` ON `posts` (`title`)"]
        );
        match &c.fields[2].kind {
            FieldKind::Select(o) => assert_eq!(o.max_select, 1),
            _ => unreachable!(),
        }
    }

    #[test]
    fn reports_invalid_definitions() {
        let mut c = parse(json!({
            "name": "_bad name",
            "fields": [
                { "name": "title", "type": "text", "min": 5, "max": 2, "pattern": "(" },
                { "name": "TITLE", "type": "text" },
                { "name": "expand", "type": "text" },
                { "name": "tags", "type": "select", "values": [] },
                { "name": "owner", "type": "relation", "collectionId": "missing" },
                { "name": "when", "type": "date", "min": "nope" }
            ],
            "listRule": "nope = 1",
            "indexes": ["CREATE INDEX idx ON other (title)", "CREATE INDEX idx2 ON _bad_name (missing)"]
        }));
        let errors = prepare(&mut c, None, &Registry::default());
        let p = paths(&errors);
        for expected in [
            "name",
            "fields.1.min",
            "fields.1.pattern",
            "fields.2.name",
            "fields.3.name",
            "fields.4.values",
            "fields.5.collectionId",
            "fields.6.min",
            "listRule",
            "indexes.0",
        ] {
            assert!(
                p.contains(&expected.to_string()),
                "missing {expected} in {p:?}"
            );
        }
    }

    #[test]
    fn update_rules() {
        let mut original = parse(json!({
            "name": "posts",
            "fields": [{ "name": "title", "type": "text" }]
        }));
        assert!(prepare(&mut original, None, &Registry::default()).is_empty());
        let registry = Registry::new(vec![original.clone()]);

        // changing a field type is rejected
        let mut changed = original.clone();
        changed.fields[1].kind = FieldKind::Number(NumberOptions::default());
        let errors = prepare(&mut changed, Some(&original), &registry);
        assert!(errors.contains("fields.1.type"), "{errors:?}");

        // removing the id field is rejected
        let mut removed = original.clone();
        removed.fields.remove(0);
        let errors = prepare(&mut removed, Some(&original), &registry);
        assert!(!errors.is_empty());

        // renaming is fine and keeps the identity
        let mut renamed = original.clone();
        renamed.name = "articles".into();
        renamed.id = "something-else".into();
        assert!(prepare(&mut renamed, Some(&original), &registry).is_empty());
        assert_eq!(renamed.id, original.id);

        // another collection can't take the name
        let mut other = parse(json!({ "name": "POSTS" }));
        assert!(prepare(&mut other, None, &registry).contains("name"));
    }
}
