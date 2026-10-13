//! Record field values: normalizing request input, validating it, and converting
//! between JSON and SQL parameters.
//!
//! Values are kept as JSON in a normalized form per storage type:
//! text → string, number → number, bool → bool, date → PocketBase formatted string
//! (`""` when empty), multi → array of strings, json → any value.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::{Map, Number, Value};

use crate::collections::model::{Collection, Field, FieldKind, Storage};
use crate::validation::Errors;
use crate::{datetime, ids};

pub type Values = Map<String, Value>;

/// Defaults applied when an option is 0 (same as PocketBase).
const DEFAULT_TEXT_MAX: usize = 5000;
const DEFAULT_EDITOR_MAX_SIZE: i64 = 5 << 20;
const DEFAULT_JSON_MAX_SIZE: i64 = 1 << 20;

pub fn default_value(field: &Field) -> Value {
    if matches!(field.kind, FieldKind::GeoPoint(_)) {
        return serde_json::json!({ "lon": 0, "lat": 0 });
    }
    match field.storage() {
        Storage::Text | Storage::Date => Value::String(String::new()),
        Storage::Number => Value::from(0),
        Storage::Bool => Value::Bool(false),
        Storage::Multi => Value::Array(vec![]),
        Storage::Json => Value::Null,
    }
}

pub fn defaults(c: &Collection) -> Values {
    c.fields
        .iter()
        .map(|f| (f.name.clone(), default_value(f)))
        .collect()
}

enum Modifier {
    Set,
    Append,
    Prepend,
    Remove,
}

/// Applies a request body to `values`. Unknown keys are ignored (like PocketBase).
/// Supports the `field+` / `+field` / `field-` modifiers for multi-valued and number fields.
/// What a request body changed, beyond the values themselves.
#[derive(Debug, Default)]
pub struct Changes {
    /// Plaintext passwords by field name. They never enter [`Values`]: they're validated
    /// here and hashed by the caller.
    pub passwords: Vec<(String, String)>,
    /// Names of the fields the body set.
    pub fields: std::collections::HashSet<String>,
}

pub fn apply_body(
    c: &Collection,
    values: &mut Values,
    body: &Map<String, Value>,
    is_create: bool,
    errors: &mut Errors,
) -> Changes {
    let mut changes = Changes::default();
    // plain values first, then modifiers, regardless of key order
    let mut keys: Vec<(&String, &Value)> = body.iter().collect();
    keys.sort_by_key(|(k, _)| k.starts_with('+') || k.ends_with('+') || k.ends_with('-'));

    for (key, raw) in keys {
        let (name, modifier) = if let Some(n) = key.strip_prefix('+') {
            (n, Modifier::Prepend)
        } else if let Some(n) = key.strip_suffix('+') {
            (n, Modifier::Append)
        } else if let Some(n) = key.strip_suffix('-') {
            (n, Modifier::Remove)
        } else {
            (key.as_str(), Modifier::Set)
        };

        let Some(field) = c.field(name) else { continue };
        if matches!(field.kind, FieldKind::Autodate(_)) || (field.is_primary_key() && !is_create) {
            continue;
        }
        // token keys are managed by slonbase
        if field.is_secret() && !matches!(field.kind, FieldKind::Password(_)) {
            continue;
        }
        // file fields are changed through `records::files`
        if let FieldKind::File(_) = field.kind {
            continue;
        }
        if let FieldKind::Password(_) = field.kind {
            match raw {
                Value::String(plain) if !plain.is_empty() => {
                    changes.passwords.push((field.name.clone(), plain.clone()));
                    changes.fields.insert(field.name.clone());
                }
                Value::String(_) | Value::Null => {}
                _ => errors.invalid(&field.name, "Must be a string."),
            }
            continue;
        }
        changes.fields.insert(field.name.clone());

        let incoming = match normalize(field, raw) {
            Ok(v) => v,
            Err(message) => {
                errors.invalid(&field.name, message);
                continue;
            }
        };
        let current = values
            .get(&field.name)
            .cloned()
            .unwrap_or_else(|| default_value(field));

        let next = match (&modifier, field.storage()) {
            (Modifier::Set, _) => incoming,
            (m, Storage::Multi) => {
                let mut list = as_strings(&current);
                let items = as_strings(&incoming);
                match m {
                    Modifier::Append => list.extend(items),
                    Modifier::Prepend => list.splice(0..0, items).for_each(drop),
                    Modifier::Remove => list.retain(|v| !items.contains(v)),
                    Modifier::Set => unreachable!(),
                }
                Value::Array(dedupe(list).into_iter().map(Value::String).collect())
            }
            (Modifier::Append | Modifier::Remove, Storage::Number) => {
                let a = current.as_f64().unwrap_or(0.0);
                let b = incoming.as_f64().unwrap_or(0.0);
                number(if matches!(modifier, Modifier::Append) {
                    a + b
                } else {
                    a - b
                })
            }
            _ => {
                errors.invalid(&field.name, "This modifier is not supported for the field.");
                continue;
            }
        };
        values.insert(field.name.clone(), next);
    }
    changes
}

/// Converts loosely typed input into the field's normalized JSON form.
pub fn normalize(field: &Field, raw: &Value) -> Result<Value, String> {
    if matches!(field.kind, FieldKind::GeoPoint(_)) {
        return geo_point(raw);
    }
    match field.storage() {
        Storage::Text => match raw {
            Value::Null => Ok(Value::String(String::new())),
            Value::String(_) => Ok(raw.clone()),
            Value::Number(n) => Ok(Value::String(n.to_string())),
            Value::Bool(b) => Ok(Value::String(b.to_string())),
            // a single select/relation keeps the last item of a list
            Value::Array(items)
                if matches!(field.kind, FieldKind::Select(_) | FieldKind::Relation(_)) =>
            {
                match items.last() {
                    None => Ok(Value::String(String::new())),
                    Some(Value::String(s)) => Ok(Value::String(s.clone())),
                    Some(_) => Err("Must be a string.".into()),
                }
            }
            _ => Err("Must be a string.".into()),
        },
        Storage::Number => {
            let n = match raw {
                Value::Null => 0.0,
                Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
                Value::String(s) if s.trim().is_empty() => 0.0,
                Value::String(s) => s.trim().parse::<f64>().map_err(|_| "Must be a number.")?,
                Value::Bool(b) => f64::from(u8::from(*b)),
                _ => return Err("Must be a number.".into()),
            };
            if !n.is_finite() {
                return Err("Must be a finite number.".into());
            }
            Ok(number(n))
        }
        Storage::Bool => match raw {
            Value::Null => Ok(Value::Bool(false)),
            Value::Bool(_) => Ok(raw.clone()),
            Value::Number(n) if n.as_f64() == Some(1.0) => Ok(Value::Bool(true)),
            Value::Number(n) if n.as_f64() == Some(0.0) => Ok(Value::Bool(false)),
            Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "1" => Ok(Value::Bool(true)),
                "false" | "0" | "" => Ok(Value::Bool(false)),
                _ => Err("Must be a boolean.".into()),
            },
            _ => Err("Must be a boolean.".into()),
        },
        Storage::Date => match raw {
            Value::Null => Ok(Value::String(String::new())),
            Value::String(s) if s.trim().is_empty() => Ok(Value::String(String::new())),
            Value::String(s) => datetime::parse(s)
                .map(|d| Value::String(datetime::format(&d)))
                .ok_or_else(|| "Must be a valid date.".into()),
            _ => Err("Must be a valid date.".into()),
        },
        Storage::Multi => {
            let items = match raw {
                Value::Null => vec![],
                Value::String(s) if s.is_empty() => vec![],
                Value::String(s) => vec![s.clone()],
                Value::Array(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        match item {
                            Value::String(s) if !s.is_empty() => out.push(s.clone()),
                            Value::String(_) => {}
                            _ => return Err("Must be a list of strings.".into()),
                        }
                    }
                    out
                }
                _ => return Err("Must be a list of strings.".into()),
            };
            Ok(Value::Array(
                dedupe(items).into_iter().map(Value::String).collect(),
            ))
        }
        Storage::Json => Ok(raw.clone()),
    }
}

/// `{"lon", "lat"}` from an object (numbers or numeric strings) or a JSON string of one.
fn geo_point(raw: &Value) -> Result<Value, String> {
    let invalid = || "Must be an object with lon and lat numbers.".to_string();
    let object = match raw {
        Value::Null => return Ok(serde_json::json!({ "lon": 0, "lat": 0 })),
        Value::String(s) if s.trim().is_empty() => {
            return Ok(serde_json::json!({ "lon": 0, "lat": 0 }));
        }
        Value::String(s) => serde_json::from_str::<Value>(s).map_err(|_| invalid())?,
        other => other.clone(),
    };
    let Value::Object(map) = object else {
        return Err(invalid());
    };
    let coordinate = |key: &str| -> Result<f64, String> {
        match map.get(key) {
            None | Some(Value::Null) => Ok(0.0),
            Some(Value::Number(n)) => n.as_f64().ok_or_else(invalid),
            Some(Value::String(s)) => s.trim().parse::<f64>().map_err(|_| invalid()),
            _ => Err(invalid()),
        }
    };
    let (lon, lat) = (coordinate("lon")?, coordinate("lat")?);
    if !(-180.0..=180.0).contains(&lon) || !lon.is_finite() {
        return Err("The longitude must be between -180 and 180.".into());
    }
    if !(-90.0..=90.0).contains(&lat) || !lat.is_finite() {
        return Err("The latitude must be between -90 and 90.".into());
    }
    Ok(serde_json::json!({ "lon": number(lon), "lat": number(lat) }))
}

/// Validates the final values of a record.
pub fn validate(
    c: &Collection,
    values: &Values,
    changes: &Changes,
    is_create: bool,
    errors: &mut Errors,
) {
    for field in &c.fields {
        if errors.contains(&field.name) {
            continue;
        }
        if let FieldKind::Password(o) = &field.kind {
            let plain = changes
                .passwords
                .iter()
                .find(|(name, _)| name == &field.name);
            match plain {
                Some((_, plain)) => {
                    if let Err((code, message)) = validate_password(o, plain) {
                        errors.add(&field.name, code, message);
                    }
                }
                // an existing record keeps its hash
                None if is_create && o.required => {
                    errors.add(&field.name, "validation_required", "Cannot be blank.")
                }
                None => {}
            }
            continue;
        }
        let value = values
            .get(&field.name)
            .cloned()
            .unwrap_or_else(|| default_value(field));
        if let Err((code, message)) = validate_value(field, &value) {
            errors.add(&field.name, code, message);
        }
    }
}

type Failure = (&'static str, String);

/// Validates a plaintext password against a password field's rules.
pub fn check_password(field: &Field, plain: &str) -> Result<(), String> {
    match &field.kind {
        FieldKind::Password(o) => validate_password(o, plain).map_err(|(_, m)| m),
        _ => Ok(()),
    }
}

/// Validates a single normalized value against its field's rules.
pub fn check_value(field: &Field, value: &Value) -> Result<(), String> {
    validate_value(field, value).map_err(|(_, m)| m)
}

fn validate_password(
    o: &crate::collections::model::PasswordOptions,
    plain: &str,
) -> Result<(), Failure> {
    let max = if o.max == 0 { 71 } else { o.max };
    let len = plain.chars().count();
    if len < o.min {
        return Err(invalid(format!("Must be at least {} character(s).", o.min)));
    }
    if len > max || plain.len() > 71 {
        return Err(invalid(format!("Must be no more than {max} character(s).")));
    }
    if !o.pattern.is_empty() && !pattern_matches(&o.pattern, plain) {
        return Err(invalid("Invalid value format."));
    }
    Ok(())
}

/// Hashes a password with the field's bcrypt cost (off the async threads).
pub async fn hash_password(field: &Field, plain: String) -> anyhow::Result<String> {
    let cost = match &field.kind {
        FieldKind::Password(o) if o.cost != 0 => o.cost,
        _ => 10,
    };
    Ok(tokio::task::spawn_blocking(move || bcrypt::hash(plain, cost)).await??)
}

/// Checks a password against a stored hash (off the async threads).
pub async fn verify_password(plain: String, hash: String) -> bool {
    if hash.is_empty() {
        return false;
    }
    tokio::task::spawn_blocking(move || bcrypt::verify(plain, &hash).unwrap_or(false))
        .await
        .unwrap_or(false)
}

fn required() -> Failure {
    ("validation_required", "Cannot be blank.".into())
}

fn invalid(message: impl Into<String>) -> Failure {
    ("validation_invalid", message.into())
}

fn validate_value(field: &Field, value: &Value) -> Result<(), Failure> {
    match &field.kind {
        FieldKind::Text(o) => {
            let s = value.as_str().unwrap_or_default();
            if s.is_empty() {
                return if o.required { Err(required()) } else { Ok(()) };
            }
            let len = s.chars().count();
            let max = if o.max == 0 { DEFAULT_TEXT_MAX } else { o.max };
            if len < o.min {
                return Err(invalid(format!("Must be at least {} character(s).", o.min)));
            }
            if len > max {
                return Err(invalid(format!("Must be no more than {max} character(s).")));
            }
            if !o.pattern.is_empty() && !pattern_matches(&o.pattern, s) {
                return Err(invalid("Invalid value format."));
            }
            Ok(())
        }
        FieldKind::Editor(o) => {
            let s = value.as_str().unwrap_or_default();
            if s.is_empty() {
                return if o.required { Err(required()) } else { Ok(()) };
            }
            let max = if o.max_size == 0 {
                DEFAULT_EDITOR_MAX_SIZE
            } else {
                o.max_size
            };
            if s.len() as i64 > max {
                return Err(invalid(format!("Must be less than {max} bytes.")));
            }
            Ok(())
        }
        FieldKind::Email(o) | FieldKind::Url(o) => {
            let s = value.as_str().unwrap_or_default();
            if s.is_empty() {
                return if o.required { Err(required()) } else { Ok(()) };
            }
            let is_email = matches!(field.kind, FieldKind::Email(_));
            let domain = if is_email {
                email_domain(s)
            } else {
                url_host(s)
            };
            let Some(domain) = domain else {
                return Err(invalid(if is_email {
                    "Must be a valid email address."
                } else {
                    "Must be a valid url."
                }));
            };
            let matches =
                |list: &[String]| list.iter().any(|d| d.trim().eq_ignore_ascii_case(&domain));
            if !o.only_domains.is_empty() && !matches(&o.only_domains) {
                return Err(invalid("The domain is not allowed."));
            }
            if matches(&o.except_domains) {
                return Err(invalid("The domain is not allowed."));
            }
            Ok(())
        }
        FieldKind::Number(o) => {
            let n = value.as_f64().unwrap_or(0.0);
            if n == 0.0 {
                return if o.required { Err(required()) } else { Ok(()) };
            }
            if o.only_int && n.fract() != 0.0 {
                return Err(invalid("Decimal numbers are not allowed."));
            }
            if o.min.is_some_and(|min| n < min) {
                return Err(invalid(format!("Must be larger than {}.", o.min.unwrap())));
            }
            if o.max.is_some_and(|max| n > max) {
                return Err(invalid(format!("Must be less than {}.", o.max.unwrap())));
            }
            Ok(())
        }
        FieldKind::Bool(o) => {
            if o.required && value != &Value::Bool(true) {
                return Err(required());
            }
            Ok(())
        }
        FieldKind::Date(o) => {
            let s = value.as_str().unwrap_or_default();
            if s.is_empty() {
                return if o.required { Err(required()) } else { Ok(()) };
            }
            let dt = datetime::parse(s).ok_or_else(|| invalid("Must be a valid date."))?;
            if let Some(min) = datetime::parse(&o.min) {
                if dt < min {
                    return Err(invalid(format!("Must be after {}.", o.min)));
                }
            }
            if let Some(max) = datetime::parse(&o.max) {
                if dt > max {
                    return Err(invalid(format!("Must be before {}.", o.max)));
                }
            }
            Ok(())
        }
        FieldKind::Autodate(_) | FieldKind::Password(_) => Ok(()),
        FieldKind::File(o) => {
            let items = as_strings(value);
            if items.is_empty() {
                return if o.required { Err(required()) } else { Ok(()) };
            }
            if items.len() > o.max_select.max(1) {
                return Err(invalid(format!(
                    "Select no more than {} file(s).",
                    o.max_select.max(1)
                )));
            }
            Ok(())
        }
        FieldKind::Select(o) => {
            let items = as_strings(value);
            if items.is_empty() {
                return if o.required { Err(required()) } else { Ok(()) };
            }
            if let Some(bad) = items.iter().find(|v| !o.values.contains(v)) {
                return Err(invalid(format!("Invalid value {bad:?}.")));
            }
            if items.len() > o.max_select.max(1) {
                return Err(invalid(format!("Select no more than {}.", o.max_select)));
            }
            Ok(())
        }
        FieldKind::Relation(o) => {
            let items = as_strings(value);
            if items.is_empty() {
                return if o.required { Err(required()) } else { Ok(()) };
            }
            if items.len() < o.min_select {
                return Err(invalid(format!("Select at least {}.", o.min_select)));
            }
            if items.len() > o.max_select.max(1) {
                return Err(invalid(format!("Select no more than {}.", o.max_select)));
            }
            Ok(())
        }
        FieldKind::GeoPoint(o) => {
            let zero = value.get("lon").and_then(Value::as_f64) == Some(0.0)
                && value.get("lat").and_then(Value::as_f64) == Some(0.0);
            if o.required && zero {
                return Err(required());
            }
            Ok(())
        }
        FieldKind::Json(o) => {
            let empty = match value {
                Value::Null => true,
                Value::String(s) => s.is_empty(),
                Value::Array(a) => a.is_empty(),
                Value::Object(m) => m.is_empty(),
                _ => false,
            };
            if empty && o.required {
                return Err(required());
            }
            let max = if o.max_size == 0 {
                DEFAULT_JSON_MAX_SIZE
            } else {
                o.max_size
            };
            if value.to_string().len() as i64 > max {
                return Err(invalid(format!("The data must be less than {max} bytes.")));
            }
            Ok(())
        }
    }
}

/// Sets system managed values: the generated id and autodate fields.
pub fn finalize(
    c: &Collection,
    values: &mut Values,
    is_create: bool,
    now: &DateTime<Utc>,
    errors: &mut Errors,
) {
    let now_text = Value::String(datetime::format(now));
    for field in &c.fields {
        match &field.kind {
            FieldKind::Text(o) if is_create => {
                let empty = values
                    .get(&field.name)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .is_empty();
                if empty && !o.autogenerate_pattern.is_empty() {
                    if let Some(v) = ids::generate_from_pattern(&o.autogenerate_pattern) {
                        values.insert(field.name.clone(), Value::String(v));
                    }
                } else if empty && field.is_primary_key() {
                    errors.add(&field.name, "validation_required", "Cannot be blank.");
                }
            }
            FieldKind::Autodate(o) if (is_create && o.on_create) || (!is_create && o.on_update) => {
                values.insert(field.name.clone(), now_text.clone());
            }
            _ => {}
        }
    }
}

/// The SQL cast applied to a `$n::text` parameter for a field.
pub fn sql_cast(field: &Field) -> &'static str {
    match field.storage() {
        Storage::Text => "",
        Storage::Number => "::double precision",
        Storage::Bool => "::boolean",
        Storage::Date => "::timestamptz",
        Storage::Multi | Storage::Json => "::jsonb",
    }
}

/// The text parameter for a normalized value (`None` binds SQL NULL).
pub fn to_param(field: &Field, value: &Value) -> Option<String> {
    match field.storage() {
        Storage::Text => Some(value.as_str().unwrap_or_default().to_string()),
        Storage::Number => Some(value.as_f64().unwrap_or(0.0).to_string()),
        Storage::Bool => Some(value.as_bool().unwrap_or(false).to_string()),
        Storage::Date => value.as_str().filter(|s| !s.is_empty()).map(str::to_string),
        Storage::Multi => Some(
            Value::Array(as_strings(value).into_iter().map(Value::String).collect()).to_string(),
        ),
        Storage::Json => match value {
            Value::Null => None,
            v => Some(v.to_string()),
        },
    }
}

pub fn as_strings(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Value::String(s) if !s.is_empty() => vec![s.clone()],
        _ => vec![],
    }
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|v| seen.insert(v.clone()))
        .collect()
}

/// JSON number, written as an integer when it has no fractional part (like Go/PocketBase).
pub fn number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        Value::Number(Number::from(n as i64))
    } else {
        Number::from_f64(n)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

fn email_domain(s: &str) -> Option<String> {
    let (local, domain) = s.rsplit_once('@')?;
    let valid = !local.is_empty()
        && local.len() <= 64
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
        && !local.contains('@')
        && domain
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-');
    valid.then(|| domain.to_ascii_lowercase())
}

fn url_host(s: &str) -> Option<String> {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))?;
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host_port.starts_with('[') {
        host_port.split(']').next()?.trim_start_matches('[')
    } else {
        host_port.split(':').next()?
    };
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | ':'));
    valid.then(|| host.to_ascii_lowercase())
}

/// Matches a field pattern, caching compiled regexes.
fn pattern_matches(pattern: &str, value: &str) -> bool {
    static CACHE: LazyLock<Mutex<HashMap<String, Regex>>> = LazyLock::new(Default::default);
    let mut cache = CACHE.lock().expect("regex cache");
    if !cache.contains_key(pattern) {
        let Ok(re) = Regex::new(pattern) else {
            return false;
        };
        if cache.len() > 500 {
            cache.clear();
        }
        cache.insert(pattern.to_string(), re);
    }
    cache[pattern].is_match(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::model::default_id_field;
    use serde_json::json;

    fn collection() -> Collection {
        serde_json::from_value(json!({
            "id": "pbc_1",
            "name": "posts",
            "fields": [
                default_id_field(),
                { "id": "a", "name": "title", "type": "text", "required": true, "max": 10, "pattern": "^[a-z ]+$" },
                { "id": "b", "name": "views", "type": "number", "onlyInt": true, "min": 0 },
                { "id": "c", "name": "active", "type": "bool" },
                { "id": "d", "name": "when", "type": "date" },
                { "id": "e", "name": "tags", "type": "select", "values": ["a", "b", "c"], "maxSelect": 2 },
                { "id": "f", "name": "email", "type": "email", "onlyDomains": ["example.com"] },
                { "id": "g", "name": "site", "type": "url" },
                { "id": "h", "name": "created", "type": "autodate", "onCreate": true },
                { "id": "i", "name": "updated", "type": "autodate", "onCreate": true, "onUpdate": true }
            ]
        }))
        .unwrap()
    }

    fn run(body: Value, existing: Option<Values>) -> (Values, Errors) {
        let c = collection();
        let is_create = existing.is_none();
        let mut values = existing.unwrap_or_else(|| defaults(&c));
        let mut errors = Errors::new();
        let changes = apply_body(
            &c,
            &mut values,
            body.as_object().unwrap(),
            is_create,
            &mut errors,
        );
        finalize(&c, &mut values, is_create, &Utc::now(), &mut errors);
        validate(&c, &values, &changes, is_create, &mut errors);
        (values, errors)
    }

    #[test]
    fn normalizes_loose_input() {
        let (v, errors) = run(
            json!({
                "title": "hello",
                "views": "12",
                "active": "true",
                "when": "2024-01-02T03:04:05Z",
                "tags": "a",
                "email": "Me@Example.com",
                "site": "https://example.com/x",
                "created": "2000-01-01",
                "unknown": 1
            }),
            None,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(v["views"], json!(12));
        assert_eq!(v["active"], json!(true));
        assert_eq!(v["when"], json!("2024-01-02 03:04:05.000Z"));
        assert_eq!(v["tags"], json!(["a"]));
        assert_eq!(v["id"].as_str().unwrap().len(), 15);
        // autodate is set by the server, not the client
        assert_ne!(v["created"], json!("2000-01-01"));
        assert_eq!(v["created"], v["updated"]);
        assert!(!v.contains_key("unknown"));
    }

    #[test]
    fn validation_errors() {
        let (_, errors) = run(
            json!({
                "title": "UPPER",
                "views": 1.5,
                "tags": ["a", "x"],
                "email": "me@other.com",
                "site": "ftp://x",
                "when": "not a date"
            }),
            None,
        );
        for field in ["title", "views", "tags", "email", "site", "when"] {
            assert!(
                errors.contains(field),
                "expected an error for {field}: {errors:?}"
            );
        }

        let (_, errors) = run(json!({}), None);
        assert!(errors.contains("title"), "required");

        let (_, errors) = run(json!({ "title": "a very long title" }), None);
        assert!(errors.contains("title"), "max length");

        let (_, errors) = run(json!({ "title": "ok", "tags": ["a", "b", "c"] }), None);
        assert!(errors.contains("tags"), "maxSelect");
    }

    #[test]
    fn modifiers_and_updates() {
        let (created, errors) = run(json!({ "title": "x", "tags": ["a"], "views": 5 }), None);
        assert!(errors.is_empty(), "{errors:?}");
        let id = created["id"].clone();

        let (updated, errors) = run(
            json!({ "tags+": "b", "views+": 2, "id": "changed", "created": "2000-01-01" }),
            Some(created.clone()),
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(updated["tags"], json!(["a", "b"]));
        assert_eq!(updated["views"], json!(7));
        assert_eq!(updated["id"], id, "the id can't change");
        assert_eq!(updated["created"], created["created"]);

        let (updated, _) = run(
            json!({ "tags-": ["a"], "views-": 10, "+tags": "c" }),
            Some(created),
        );
        assert_eq!(updated["tags"], json!(["c"]));
        assert!(updated["views"].as_f64().unwrap() < 0.0);
    }

    #[test]
    fn params() {
        let c = collection();
        let f = |name: &str| c.field(name).unwrap().clone();
        assert_eq!(to_param(&f("views"), &json!(1.5)).as_deref(), Some("1.5"));
        assert_eq!(to_param(&f("when"), &json!("")), None);
        assert_eq!(
            to_param(&f("tags"), &json!(["a"])).as_deref(),
            Some(r#"["a"]"#)
        );
        assert_eq!(number(3.0), json!(3));
        assert_eq!(number(0.5), json!(0.5));
    }
}
