//! The `expand` query parameter (`expand=author,comments_via_post.author`), compatible with
//! PocketBase: related records are added under each record's `expand` key.
//!
//! - `field` expands a relation field: an object for single relations, a list for multiple
//!   ones (in the stored order, missing or forbidden records left out).
//! - `{collection}_via_{field}` expands back-relations: the records of `collection` whose
//!   relation `field` points to the record, as a list.
//!
//! Related records are checked against their collection's `viewRule` (`listRule` for
//! back-relations) for the caller, and returned with the same field visibility as a
//! direct request. Unknown or forbidden paths are skipped, as in PocketBase.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Map, Value};

use super::access::{email_visible_sql, output_record, output_select, rule_sql};
use super::store::{ALIAS, visible_fields};
use super::values;
use crate::auth::RequestAuth;
use crate::collections::model::{Collection, Field, FieldKind, RuleAction};
use crate::collections::schema::table_ref;
use crate::core::Core;
use crate::db::TextQuery;
use crate::db::describe_pool_error;
use crate::error::ApiError;
use crate::filter::{Params, RequestInfo, quote_ident};

/// Maximum nesting of expanded relations (same as PocketBase).
pub const MAX_DEPTH: usize = 6;
/// Maximum records loaded per expanded path.
const MAX_RELATED: usize = 5000;

/// A parsed `expand` parameter.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Tree(BTreeMap<String, Tree>);

impl Tree {
    pub fn parse(raw: &str) -> Self {
        let mut tree = Tree::default();
        for path in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let mut node = &mut tree;
            for part in path.split('.').take(MAX_DEPTH) {
                if part.is_empty() {
                    break;
                }
                node = node.0.entry(part.to_string()).or_default();
            }
        }
        tree
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub struct Context<'a> {
    pub core: &'a std::sync::Arc<Core>,
    pub auth: &'a RequestAuth,
    pub request: &'a RequestInfo,
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Adds the expanded relations to `records` (API representations of records of `c`).
pub fn apply<'a>(
    ctx: &'a Context<'a>,
    c: Arc<Collection>,
    records: &'a mut [Value],
    tree: &'a Tree,
) -> BoxFuture<'a, Result<(), ApiError>> {
    Box::pin(async move {
        if records.is_empty() || tree.is_empty() {
            return Ok(());
        }
        let registry = ctx.core.registry();

        for (key, children) in &tree.0 {
            // forward relation
            if let Some(field) = visible_relation(&c, key, ctx.auth) {
                let FieldKind::Relation(options) = &field.kind else {
                    continue;
                };
                let target = if options.collection_id == c.id {
                    c.clone()
                } else {
                    match registry.by_id(&options.collection_id) {
                        Some(t) => t.clone(),
                        None => continue,
                    }
                };
                let mut ids: Vec<String> = Vec::new();
                for record in records.iter() {
                    for id in values::as_strings(record.get(&field.name).unwrap_or(&Value::Null)) {
                        if !ids.contains(&id) {
                            ids.push(id);
                        }
                    }
                }
                if ids.is_empty() {
                    continue;
                }

                let mut related = fetch(ctx, &target, RuleAction::View, |params| {
                    format!(
                        "{ALIAS}.\"id\" IN (SELECT jsonb_array_elements_text(({})::jsonb))",
                        params.push(json_list(&ids))
                    )
                })
                .await?;
                apply(ctx, target.clone(), &mut related, children).await?;
                let by_id: HashMap<String, Value> = related
                    .into_iter()
                    .filter_map(|r| Some((r.get("id")?.as_str()?.to_string(), r)))
                    .collect();

                for record in records.iter_mut() {
                    let refs = values::as_strings(record.get(&field.name).unwrap_or(&Value::Null));
                    let value = if field.is_multi() {
                        Value::Array(
                            refs.iter()
                                .filter_map(|id| by_id.get(id).cloned())
                                .collect(),
                        )
                    } else {
                        match refs.first().and_then(|id| by_id.get(id)) {
                            Some(r) => r.clone(),
                            None => continue,
                        }
                    };
                    expand_map(record).insert(key.clone(), value);
                }
                continue;
            }

            // back-relation: {collection}_via_{field}
            let Some((collection_name, field_name)) = key.rsplit_once("_via_") else {
                continue;
            };
            let Some(source) = registry.get(collection_name).cloned() else {
                continue;
            };
            let Some(field) = visible_relation(&source, field_name, ctx.auth) else {
                continue;
            };
            let points_here =
                matches!(&field.kind, FieldKind::Relation(o) if o.collection_id == c.id);
            if !points_here {
                continue;
            }
            let ids: Vec<String> = records
                .iter()
                .filter_map(|r| r.get("id").and_then(Value::as_str).map(str::to_string))
                .collect();
            let column = format!("{ALIAS}.{}", quote_ident(&field.name));
            let multi = field.is_multi();
            let mut related = fetch(ctx, &source, RuleAction::List, |params| {
                let list = format!("(SELECT jsonb_array_elements_text(({})::jsonb))", params.push(json_list(&ids)));
                if multi {
                    format!("EXISTS (SELECT 1 FROM jsonb_array_elements_text({column}) AS __b(v) WHERE __b.v IN {list})")
                } else {
                    format!("{column} IN {list}")
                }
            })
            .await?;
            apply(ctx, source.clone(), &mut related, children).await?;

            let mut grouped: HashMap<String, Vec<Value>> = HashMap::new();
            for r in related {
                for id in values::as_strings(r.get(&field.name).unwrap_or(&Value::Null)) {
                    grouped.entry(id).or_default().push(r.clone());
                }
            }
            for record in records.iter_mut() {
                let id = record
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(list) = grouped.remove(&id) {
                    expand_map(record).insert(key.clone(), Value::Array(list));
                }
            }
        }
        Ok(())
    })
}

/// A relation field the caller may see.
fn visible_relation<'c>(c: &'c Collection, name: &str, auth: &RequestAuth) -> Option<&'c Field> {
    c.field(name)
        .filter(|f| matches!(f.kind, FieldKind::Relation(_)))
        .filter(|f| !f.hidden || auth.is_superuser())
}

fn json_list(ids: &[String]) -> String {
    Value::Array(ids.iter().cloned().map(Value::String).collect()).to_string()
}

fn expand_map(record: &mut Value) -> &mut Map<String, Value> {
    let obj = record.as_object_mut().expect("records are objects");
    obj.entry("expand")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("expand is an object")
}

/// Loads the records of `c` matching `condition` that the caller may access with `action`.
async fn fetch(
    ctx: &Context<'_>,
    c: &Collection,
    action: RuleAction,
    condition: impl FnOnce(&mut Params) -> String,
) -> Result<Vec<Value>, ApiError> {
    let mut params = Params::default();
    let mut where_sql = condition(&mut params);
    match rule_sql(c, action, ctx.auth, ctx.request, &mut params) {
        Ok(Some(rule)) => where_sql = format!("{where_sql} AND {rule}"),
        Ok(None) => {}
        Err(_) => return Ok(vec![]), // superusers only
    }
    let email_visible = email_visible_sql(c, ctx.auth, ctx.request, &mut params)?;
    let fields = visible_fields(c, ctx.auth.is_superuser());
    let sql = format!(
        "SELECT {} FROM {} {ALIAS} WHERE {where_sql} ORDER BY {} LIMIT {MAX_RELATED}",
        output_select(&fields, &email_visible),
        table_ref(&c.name),
        super::store::row_order(c, ALIAS)
    );
    let client = ctx
        .core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(describe_pool_error(e)))?;
    let rows = client
        .query_text(&sql, &params)
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    let mut records: Vec<Value> = rows
        .iter()
        .map(|row| output_record(c, &fields, row, &email_visible))
        .collect();
    super::api::enrich(ctx.core, ctx.request, c, &mut records).await?;
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_expand() {
        let tree = Tree::parse("author, comments_via_post.author,author.org , ,x..y");
        let keys: Vec<&String> = tree.0.keys().collect();
        assert_eq!(keys, ["author", "comments_via_post", "x"]);
        assert!(tree.0["author"].0.contains_key("org"));
        assert!(tree.0["comments_via_post"].0.contains_key("author"));
        assert!(tree.0["x"].is_empty(), "stops at an empty part");

        let deep = Tree::parse("a.b.c.d.e.f.g.h");
        let mut depth = 0;
        let mut node = &deep;
        while let Some((_, child)) = node.0.iter().next() {
            depth += 1;
            node = child;
        }
        assert_eq!(depth, MAX_DEPTH);
    }
}
