//! Access checks shared by the records API and realtime: API rules, the manage rule,
//! email visibility and the `fields` parameter.

use std::collections::HashSet;

use serde_json::Value;

use tokio_postgres::Row;

use super::store::{ALIAS, record_json, row_values, select_list};
use crate::auth::RequestAuth;
use crate::collections::model::{Collection, FIELD_EMAIL_VISIBILITY, RuleAction};
use crate::collections::model::{FIELD_EMAIL, Field};
use crate::error::ApiError;
use crate::filter::{Params, RequestInfo, Scope, quote_ident, to_sql};

/// The rule condition for an action, `None` when nothing needs checking.
pub fn rule_sql(
    c: &Collection,
    action: RuleAction,
    auth: &RequestAuth,
    request: &RequestInfo,
    params: &mut Params,
) -> Result<Option<String>, ApiError> {
    if auth.is_superuser() {
        return Ok(None);
    }
    match c.rule(action) {
        None => Err(ApiError::superusers_only()),
        Some(rule) if rule.trim().is_empty() => Ok(None),
        Some(rule) => {
            let scope = Scope {
                collection: c,
                alias: ALIAS,
                request,
                allow_hidden: true,
            };
            to_sql(rule, &scope, params)
                .map(Some)
                .map_err(|e| ApiError::internal(anyhow::anyhow!("invalid stored rule: {e}")))
        }
    }
}

/// Applies the `fields` query parameter (`id,title`; `*` keeps everything).
pub fn pick_fields(record: Value, fields: Option<&String>) -> Value {
    let Some(raw) = fields.map(|f| f.trim()).filter(|f| !f.is_empty()) else {
        return record;
    };
    let wanted: HashSet<&str> = raw.split(',').map(str::trim).collect();
    if wanted.contains("*") {
        return record;
    }
    // "expand" or "expand.<anything>" keeps the expanded records
    let keep_expand = wanted
        .iter()
        .any(|w| *w == "expand" || w.starts_with("expand."));
    match record {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(k, _)| wanted.contains(k.as_str()) || (k == "expand" && keep_expand))
                .collect(),
        ),
        other => other,
    }
}

/// The `manageRule` condition of an auth collection: `None` when nobody manages it.
pub fn manage_rule_sql(
    c: &Collection,
    request: &RequestInfo,
    params: &mut Params,
) -> Result<Option<String>, ApiError> {
    let Some(rule) = c.auth_options().and_then(|a| a.manage_rule.as_deref()) else {
        return Ok(None);
    };
    if rule.trim().is_empty() {
        return Ok(Some("TRUE".into()));
    }
    let scope = Scope {
        collection: c,
        alias: ALIAS,
        request,
        allow_hidden: true,
    };
    to_sql(rule, &scope, params)
        .map(Some)
        .map_err(|e| ApiError::internal(anyhow::anyhow!("invalid stored manage rule: {e}")))
}

/// For auth collections, a boolean SQL expression telling whether the caller may see a
/// record's email: superusers, the record itself, managers, or `emailVisibility`.
pub fn email_visible_sql(
    c: &Collection,
    auth: &RequestAuth,
    request: &RequestInfo,
    params: &mut Params,
) -> Result<Option<String>, ApiError> {
    if !c.is_auth() {
        return Ok(None);
    }
    if auth.is_superuser() {
        return Ok(Some("TRUE".into()));
    }
    let mut parts = vec![format!("{ALIAS}.{}", quote_ident(FIELD_EMAIL_VISIBILITY))];
    if let Some(record) = auth.record.as_ref().filter(|r| r.collection.id == c.id) {
        parts.push(format!("{ALIAS}.\"id\" = {}", params.push(record.id())));
    }
    if let Some(manage) = manage_rule_sql(c, request, params)? {
        parts.push(manage);
    }
    Ok(Some(format!("({})", parts.join(" OR "))))
}

/// Select list for client responses: the visible fields, then the email visibility flag.
pub fn output_select(fields: &[&Field], email_visible: &Option<String>) -> String {
    match email_visible {
        Some(sql) => format!("{}, {sql} AS _email_visible", select_list(fields)),
        None => select_list(fields),
    }
}

/// Builds the response for a row selected with [`output_select`].
pub fn output_record(
    c: &Collection,
    fields: &[&Field],
    row: &Row,
    email_visible: &Option<String>,
) -> Value {
    let mut values = row_values(fields, row);
    if email_visible.is_some() && !row.get::<_, bool>(fields.len()) {
        values.remove(FIELD_EMAIL);
    }
    record_json(c, values)
}
