//! Compiles parsed filters into parameterized PostgreSQL conditions.
//!
//! SQLite (and so PocketBase) compares values of different types loosely; PostgreSQL
//! rejects such comparisons. Every comparison is therefore given a single type here:
//! literals are converted to the column's type when possible, and otherwise both sides
//! are compared as text. All literals are bound as `text` parameters, never inlined.
//!
//! Multi-valued fields (multi select/relation) follow PocketBase: plain operators must
//! match **all** items, `?`-prefixed operators match **any** item, and an empty list
//! behaves like a single empty string.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc};
use serde_json::Value;

use super::parser::{Cmp, Expr, Op, Operand};
use crate::collections::model::{Collection, FieldKind, Storage};
use crate::datetime;

/// Request data available to rules as `@request.*`.
#[derive(Debug, Clone)]
pub struct RequestInfo {
    pub method: String,
    /// The authenticated caller, `None` for guests.
    pub auth: Option<AuthInfo>,
    pub query: HashMap<String, String>,
    /// Keys are lowercase with `-` replaced by `_` (`@request.headers.x_api_key`).
    pub headers: HashMap<String, String>,
    pub body: serde_json::Map<String, Value>,
    pub now: DateTime<Utc>,
}

impl RequestInfo {
    pub fn new(method: &str) -> Self {
        Self {
            method: method.to_string(),
            auth: None,
            query: HashMap::new(),
            headers: HashMap::new(),
            body: serde_json::Map::new(),
            now: Utc::now(),
        }
    }
}

/// The caller as seen by rules: `@request.auth.id`, `.collectionId`, `.collectionName`
/// and the record's fields.
#[derive(Debug, Clone)]
pub struct AuthInfo {
    pub id: String,
    pub collection_id: String,
    pub collection_name: String,
    pub values: serde_json::Map<String, Value>,
}

/// Positional query parameters. Every parameter is sent as text and cast in SQL.
#[derive(Debug, Default, Clone)]
pub struct Params(pub Vec<Option<String>>);

impl Params {
    /// Adds a parameter and returns its placeholder (`$n::text`).
    pub fn push(&mut self, value: impl Into<String>) -> String {
        self.0.push(Some(value.into()));
        format!("${}::text", self.0.len())
    }

    pub fn push_null(&mut self) -> String {
        self.0.push(None);
        format!("${}::text", self.0.len())
    }

    pub fn as_refs(&self) -> Vec<&(dyn tokio_postgres::types::ToSql + Sync)> {
        self.0
            .iter()
            .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect()
    }
}

/// What an expression may reference.
pub struct Scope<'a> {
    pub collection: &'a Collection,
    /// SQL alias of the collection table.
    pub alias: &'a str,
    pub request: &'a RequestInfo,
    /// Hidden fields may be used in rules and by superusers, not in client filters.
    pub allow_hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError(pub String);

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CompileError {}

fn fail<T>(msg: impl Into<String>) -> Result<T, CompileError> {
    Err(CompileError(msg.into()))
}

pub fn compile(expr: &Expr, scope: &Scope, params: &mut Params) -> Result<String, CompileError> {
    let mut compiler = Compiler {
        scope,
        params,
        aliases: 0,
    };
    compiler.expr(expr)
}

pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Text,
    Number,
    Bool,
    Date,
    /// Text that may hold anything (JSON values); compared as a number when needed.
    Dyn,
}

#[derive(Debug, Clone)]
enum Lit {
    Text(String),
    Num(f64),
    Bool(bool),
    Date(DateTime<Utc>),
}

#[derive(Debug, Clone)]
enum Val {
    Null,
    Lit(Lit),
    Scalar {
        sql: String,
        ty: Ty,
        nullable: bool,
    },
    /// SQL evaluating to a JSONB array of strings.
    Multi {
        source: String,
    },
    /// Literal list (from `@request.body`), kept as JSON text.
    List(String),
}

struct Compiler<'a, 'p> {
    scope: &'a Scope<'a>,
    params: &'p mut Params,
    aliases: usize,
}

impl Compiler<'_, '_> {
    fn expr(&mut self, expr: &Expr) -> Result<String, CompileError> {
        match expr {
            Expr::And(a, b) => Ok(format!("({} AND {})", self.expr(a)?, self.expr(b)?)),
            Expr::Or(a, b) => Ok(format!("({} OR {})", self.expr(a)?, self.expr(b)?)),
            Expr::Cmp(cmp) => self.cmp(cmp),
        }
    }

    fn cmp(&mut self, cmp: &Cmp) -> Result<String, CompileError> {
        let left = self.operand(&cmp.left)?;
        let right = self.operand(&cmp.right)?;
        self.compare(left, cmp.op, cmp.any, right)
    }

    fn compare(
        &mut self,
        left: Val,
        op: Op,
        any: bool,
        right: Val,
    ) -> Result<String, CompileError> {
        // multi-valued operands are compared item by item
        let left_multi = self.multi_source(&left);
        let right_multi = self.multi_source(&right);
        match (left_multi, right_multi) {
            (Some(_), Some(_)) => fail("both operands can't be multi-valued"),
            (Some(source), None) => self.compare_items(source, op, any, right, false),
            (None, Some(source)) => self.compare_items(source, op, any, left, true),
            (None, None) => self.compare_scalars(left, op, right),
        }
    }

    fn multi_source(&mut self, val: &Val) -> Option<String> {
        let source = match val {
            Val::Multi { source } => source.clone(),
            Val::List(json) => format!("({})::jsonb", self.params.push(json.clone())),
            _ => return None,
        };
        // an empty list behaves like [""] (so "tags = ''" matches empty lists)
        Some(format!(
            "(CASE WHEN jsonb_array_length({source}) = 0 THEN '[\"\"]'::jsonb ELSE {source} END)"
        ))
    }

    fn compare_items(
        &mut self,
        source: String,
        op: Op,
        any: bool,
        other: Val,
        items_on_right: bool,
    ) -> Result<String, CompileError> {
        self.aliases += 1;
        let alias = format!("__e{}", self.aliases);
        let item = Val::Scalar {
            sql: format!("{alias}.v"),
            ty: Ty::Text,
            nullable: false,
        };
        let other = match other {
            Val::Null => Val::Lit(Lit::Text(String::new())),
            v => v,
        };
        let condition = if items_on_right {
            self.compare_scalars(other, op, item)?
        } else {
            self.compare_scalars(item, op, other)?
        };
        let from = format!("jsonb_array_elements_text({source}) AS {alias}(v)");
        Ok(if any {
            format!("EXISTS (SELECT 1 FROM {from} WHERE {condition})")
        } else {
            format!("NOT EXISTS (SELECT 1 FROM {from} WHERE ({condition}) IS NOT TRUE)")
        })
    }

    fn compare_scalars(&mut self, left: Val, op: Op, right: Val) -> Result<String, CompileError> {
        if matches!(op, Op::Like | Op::NotLike) {
            return self.like(left, op, right);
        }

        let (left, right) = match (left, right) {
            (Val::Null, Val::Null) => {
                return Ok(match op {
                    Op::Eq | Op::Lte | Op::Gte => "TRUE",
                    _ => "FALSE",
                }
                .into());
            }
            (Val::Null, other) | (other, Val::Null) if matches!(op, Op::Eq | Op::Neq) => {
                let empty = self.is_empty(&other);
                return Ok(if op == Op::Eq {
                    empty
                } else {
                    format!("NOT {empty}")
                });
            }
            (Val::Null, other) => (Val::Lit(Lit::Text(String::new())), other),
            (other, Val::Null) => (other, Val::Lit(Lit::Text(String::new()))),
            pair => pair,
        };

        let (l, r, ty) = self.unify(left, right)?;
        let sql_op = match op {
            Op::Eq => "=",
            Op::Neq => "IS DISTINCT FROM",
            Op::Lt => "<",
            Op::Lte => "<=",
            Op::Gt => ">",
            Op::Gte => ">=",
            Op::Like | Op::NotLike => unreachable!(),
        };
        // byte order for text (like SQLite), independent of the database collation
        let collate = if ty == Ty::Text && !matches!(op, Op::Eq | Op::Neq) {
            " COLLATE \"C\""
        } else {
            ""
        };
        Ok(format!("({l}{collate} {sql_op} {r})"))
    }

    fn like(&mut self, left: Val, op: Op, right: Val) -> Result<String, CompileError> {
        let text_of = |c: &mut Self, v: Val| -> Result<String, CompileError> {
            match v {
                Val::Null => Ok(c.params.push("")),
                v => c.as_text(v),
            }
        };
        let left = text_of(self, left)?;
        let pattern = match right {
            Val::Lit(lit) => {
                let raw = lit_text(&lit);
                let pattern = if raw.contains('%') {
                    raw
                } else {
                    format!("%{}%", escape_like(&raw))
                };
                self.params.push(pattern)
            }
            Val::Null => self.params.push("%"),
            v => format!("('%' || {} || '%')", self.as_text(v)?),
        };
        let like = format!("({left} ILIKE {pattern} ESCAPE '\\')");
        Ok(if op == Op::Like {
            like
        } else {
            format!("({like} IS NOT TRUE)")
        })
    }

    /// Converts both sides to one type. Returns their SQL and the chosen type.
    fn unify(&mut self, left: Val, right: Val) -> Result<(String, String, Ty), CompileError> {
        match (left, right) {
            (Val::Lit(a), Val::Lit(b)) => {
                let ty = match (&a, &b) {
                    (Lit::Num(_), Lit::Num(_)) => Ty::Number,
                    (Lit::Date(_), _) | (_, Lit::Date(_)) => Ty::Date,
                    (Lit::Bool(_), _) | (_, Lit::Bool(_)) => Ty::Bool,
                    _ => Ty::Text,
                };
                if convert(&a, ty).is_some() && convert(&b, ty).is_some() {
                    let l = self.lit_as(&a, ty).expect("convertible");
                    let r = self.lit_as(&b, ty).expect("convertible");
                    Ok((l, r, ty))
                } else {
                    let l = self.params.push(lit_text(&a));
                    let r = self.params.push(lit_text(&b));
                    Ok((l, r, Ty::Text))
                }
            }
            (Val::Scalar { sql, ty, .. }, Val::Lit(lit)) => {
                let (s, l, t) = self.scalar_vs_lit(sql, ty, &lit);
                Ok((s, l, t))
            }
            (Val::Lit(lit), Val::Scalar { sql, ty, .. }) => {
                let (s, l, t) = self.scalar_vs_lit(sql, ty, &lit);
                Ok((l, s, t))
            }
            (Val::Scalar { sql: a, ty: ta, .. }, Val::Scalar { sql: b, ty: tb, .. }) => {
                Ok(match (ta, tb) {
                    (x, y) if x == y => (a, b, if x == Ty::Dyn { Ty::Text } else { x }),
                    (Ty::Number, Ty::Dyn) => (a, to_num(&b), Ty::Number),
                    (Ty::Dyn, Ty::Number) => (to_num(&a), b, Ty::Number),
                    _ => (scalar_text(&a, ta), scalar_text(&b, tb), Ty::Text),
                })
            }
            _ => fail("invalid comparison operands"),
        }
    }

    fn scalar_vs_lit(&mut self, sql: String, ty: Ty, lit: &Lit) -> (String, String, Ty) {
        let target = match (ty, lit) {
            (Ty::Dyn, Lit::Num(_)) => Ty::Number,
            (Ty::Dyn, _) => Ty::Text,
            (t, _) => t,
        };
        let column = if ty == Ty::Dyn && target == Ty::Number {
            to_num(&sql)
        } else {
            sql.clone()
        };
        match self.lit_as(lit, target) {
            Some(value) => (column, value, target),
            // the literal can't be converted: compare the text forms
            None => (
                scalar_text(&sql, ty),
                self.params.push(lit_text(lit)),
                Ty::Text,
            ),
        }
    }

    /// SQL for a literal as the given type, `None` if it can't be converted.
    /// Nothing is bound unless the conversion succeeds.
    fn lit_as(&mut self, lit: &Lit, ty: Ty) -> Option<String> {
        Some(match convert(lit, ty)? {
            Converted::Param(value, "") => self.params.push(value),
            Converted::Param(value, cast) => format!("({}){cast}", self.params.push(value)),
            Converted::Inline(sql) => sql.to_string(),
        })
    }

    fn as_text(&mut self, val: Val) -> Result<String, CompileError> {
        match val {
            Val::Lit(lit) => Ok(self.params.push(lit_text(&lit))),
            Val::Scalar { sql, ty, .. } => Ok(scalar_text(&sql, ty)),
            Val::Null => Ok(self.params.push("")),
            Val::Multi { source } => Ok(format!("({source})::text")),
            Val::List(json) => Ok(self.params.push(json)),
        }
    }

    fn is_empty(&mut self, val: &Val) -> String {
        match val {
            Val::Null => "TRUE".into(),
            Val::Lit(Lit::Text(s)) => if s.is_empty() { "TRUE" } else { "FALSE" }.into(),
            Val::Lit(_) => "FALSE".into(),
            Val::Scalar { sql, ty, nullable } => match (ty, nullable) {
                (Ty::Text | Ty::Dyn, true) => format!("({sql} IS NULL OR {sql} = '')"),
                (Ty::Text | Ty::Dyn, false) => format!("({sql} = '')"),
                (_, true) => format!("({sql} IS NULL)"),
                (_, false) => "FALSE".into(),
            },
            Val::Multi { source } => format!("(jsonb_array_length({source}) = 0)"),
            Val::List(json) => if json == "[]" { "TRUE" } else { "FALSE" }.into(),
        }
    }

    fn operand(&mut self, operand: &Operand) -> Result<Val, CompileError> {
        match operand {
            Operand::Text(s) => Ok(Val::Lit(Lit::Text(s.clone()))),
            Operand::Number(n) => match n.parse::<f64>() {
                Ok(n) if n.is_finite() => Ok(Val::Lit(Lit::Num(n))),
                _ => fail(format!("invalid number {n}")),
            },
            Operand::Ident(ident) => self.ident(ident),
        }
    }

    fn ident(&mut self, ident: &str) -> Result<Val, CompileError> {
        let (path, modifier) = match ident.split_once(':') {
            Some((path, modifier)) => {
                if modifier.contains(':') {
                    return fail(format!("only one modifier is allowed ({ident})"));
                }
                (path, Some(modifier))
            }
            None => (ident, None),
        };

        match path {
            "null" if modifier.is_none() => return Ok(Val::Null),
            "true" if modifier.is_none() => return Ok(Val::Lit(Lit::Bool(true))),
            "false" if modifier.is_none() => return Ok(Val::Lit(Lit::Bool(false))),
            _ => {}
        }

        if let Some(rest) = path.strip_prefix("@request.") {
            return self.request_value(rest, modifier);
        }
        if path.starts_with("@collection.") {
            return fail("@collection references are not supported yet");
        }
        if path.starts_with('@') {
            if modifier.is_some() {
                return fail(format!("modifiers can't be used with {path}"));
            }
            return self.macro_value(path);
        }
        self.field_value(path, modifier)
    }

    fn field_value(&mut self, path: &str, modifier: Option<&str>) -> Result<Val, CompileError> {
        let mut parts = path.split('.');
        let name = parts.next().unwrap_or_default();
        let rest: Vec<&str> = parts.collect();

        let collection = self.scope.collection;
        let field = match collection.field(name) {
            Some(f) if !f.is_secret() && (!f.hidden || self.scope.allow_hidden) => f,
            _ => return fail(format!("unknown field {name:?}")),
        };
        let column = format!("{}.{}", self.scope.alias, quote_ident(&field.name));

        if !rest.is_empty() {
            return match field.kind {
                FieldKind::Json(_) => {
                    for key in &rest {
                        if key.is_empty() || !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                            return fail(format!("invalid JSON path {path:?}"));
                        }
                    }
                    let json_path = format!("{{{}}}", rest.join(","));
                    let sql = format!("({column} #>> ({})::text[])", self.params.push(json_path));
                    self.apply_modifier(
                        Val::Scalar {
                            sql,
                            ty: Ty::Dyn,
                            nullable: true,
                        },
                        modifier,
                    )
                }
                FieldKind::Relation(_) => fail("relation field paths are not supported yet"),
                _ => fail(format!("{name:?} has no nested fields")),
            };
        }

        let val = match field.storage() {
            Storage::Multi => {
                return match modifier {
                    None | Some("each") => Ok(Val::Multi { source: column }),
                    Some("length") => Ok(Val::Scalar {
                        sql: format!("jsonb_array_length({column})"),
                        ty: Ty::Number,
                        nullable: false,
                    }),
                    Some(m) => fail(format!("the :{m} modifier is not supported for {name:?}")),
                };
            }
            Storage::Text => Val::Scalar {
                sql: column,
                ty: Ty::Text,
                nullable: false,
            },
            Storage::Number => Val::Scalar {
                sql: column,
                ty: Ty::Number,
                nullable: false,
            },
            Storage::Bool => Val::Scalar {
                sql: column,
                ty: Ty::Bool,
                nullable: false,
            },
            Storage::Date => Val::Scalar {
                sql: column,
                ty: Ty::Date,
                nullable: true,
            },
            Storage::Json => Val::Scalar {
                sql: format!("({column} #>> '{{}}')"),
                ty: Ty::Dyn,
                nullable: true,
            },
        };
        if modifier == Some("each") {
            return fail(format!(
                "the :each modifier requires a multi-valued field ({name:?})"
            ));
        }
        self.apply_modifier(val, modifier)
    }

    fn apply_modifier(&mut self, val: Val, modifier: Option<&str>) -> Result<Val, CompileError> {
        match (modifier, val) {
            (None, v) => Ok(v),
            (
                Some("lower"),
                Val::Scalar {
                    sql,
                    ty: Ty::Text | Ty::Dyn,
                    nullable,
                },
            ) => Ok(Val::Scalar {
                sql: format!("lower({sql})"),
                ty: Ty::Text,
                nullable,
            }),
            (Some("lower"), Val::Lit(Lit::Text(s))) => Ok(Val::Lit(Lit::Text(s.to_lowercase()))),
            (
                Some("length"),
                Val::Scalar {
                    sql,
                    ty: Ty::Text | Ty::Dyn,
                    ..
                },
            ) => Ok(Val::Scalar {
                sql: format!("COALESCE(char_length({sql}), 0)"),
                ty: Ty::Number,
                nullable: false,
            }),
            (Some(m), _) => fail(format!("the :{m} modifier is not supported here")),
        }
    }

    fn request_value(&mut self, path: &str, modifier: Option<&str>) -> Result<Val, CompileError> {
        let req = self.scope.request;
        let (section, key) = match path.split_once('.') {
            Some((s, k)) => (s, Some(k)),
            None => (path, None),
        };

        let val = match (section, key) {
            ("method", None) => Val::Lit(Lit::Text(req.method.clone())),
            ("context", None) => Val::Lit(Lit::Text("default".into())),
            ("auth", Some(key)) => {
                if key.contains('.') {
                    return fail("@request.auth relation paths are not supported yet");
                }
                match &req.auth {
                    None => Val::Null,
                    Some(auth) => match key {
                        "id" => Val::Lit(Lit::Text(auth.id.clone())),
                        "collectionId" => Val::Lit(Lit::Text(auth.collection_id.clone())),
                        "collectionName" => Val::Lit(Lit::Text(auth.collection_name.clone())),
                        // secrets are never exposed to rules
                        "password" | "tokenKey" => Val::Null,
                        _ => json_val(auth.values.get(key)),
                    },
                }
            }
            ("query", Some(key)) => text_or_null(req.query.get(key)),
            ("headers", Some(key)) => text_or_null(req.headers.get(&key.to_ascii_lowercase())),
            ("body", Some(key)) => {
                let mut parts = key.split('.');
                let first = parts.next().unwrap_or_default();
                let mut value = req.body.get(first);
                for part in parts {
                    value = value.and_then(|v| v.get(part));
                }
                match modifier {
                    Some("isset") => return Ok(Val::Lit(Lit::Bool(value.is_some()))),
                    Some("length") => {
                        let len = match value {
                            Some(Value::Array(items)) => items.len(),
                            Some(Value::String(s)) if !s.is_empty() => 1,
                            Some(Value::Null) | None => 0,
                            Some(_) => 1,
                        };
                        return Ok(Val::Lit(Lit::Num(len as f64)));
                    }
                    _ => {}
                }
                json_val(value)
            }
            _ => return fail(format!("unknown request field @request.{path}")),
        };

        if matches!(modifier, Some("isset") | Some("length")) {
            return fail(format!(
                "the :{} modifier can only be used with @request.body",
                modifier.unwrap()
            ));
        }
        self.apply_modifier(val, modifier)
    }

    fn macro_value(&mut self, name: &str) -> Result<Val, CompileError> {
        let now = self.scope.request.now;
        let day_start = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
        let month_start = NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let next_month = if now.month() == 12 {
            NaiveDate::from_ymd_opt(now.year() + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(now.year(), now.month() + 1, 1)
        }
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
        let year_start = NaiveDate::from_ymd_opt(now.year(), 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let next_year = NaiveDate::from_ymd_opt(now.year() + 1, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let ms = Duration::milliseconds(1);

        let lit = match name {
            "@now" => Lit::Date(now),
            "@yesterday" => Lit::Date(now - Duration::days(1)),
            "@tomorrow" => Lit::Date(now + Duration::days(1)),
            "@todayStart" => Lit::Date(day_start),
            "@todayEnd" => Lit::Date(day_start + Duration::days(1) - ms),
            "@monthStart" => Lit::Date(month_start),
            "@monthEnd" => Lit::Date(next_month - ms),
            "@yearStart" => Lit::Date(year_start),
            "@yearEnd" => Lit::Date(next_year - ms),
            "@second" => Lit::Num(now.second().into()),
            "@minute" => Lit::Num(now.minute().into()),
            "@hour" => Lit::Num(now.hour().into()),
            "@weekday" => Lit::Num(now.weekday().num_days_from_sunday().into()),
            "@day" => Lit::Num(now.day().into()),
            "@month" => Lit::Num(now.month().into()),
            "@year" => Lit::Num(now.year().into()),
            _ => return fail(format!("unknown macro {name}")),
        };
        Ok(Val::Lit(lit))
    }
}

enum Converted {
    /// A text parameter and the cast applied to it.
    Param(String, &'static str),
    Inline(&'static str),
}

fn convert(lit: &Lit, ty: Ty) -> Option<Converted> {
    match ty {
        Ty::Text | Ty::Dyn => Some(Converted::Param(lit_text(lit), "")),
        Ty::Number => {
            let n = match lit {
                Lit::Num(n) => *n,
                Lit::Text(s) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite())?,
                Lit::Bool(b) => f64::from(u8::from(*b)),
                Lit::Date(_) => return None,
            };
            Some(Converted::Param(n.to_string(), "::double precision"))
        }
        Ty::Bool => {
            let b = match lit {
                Lit::Bool(b) => *b,
                Lit::Num(n) if *n == 1.0 => true,
                Lit::Num(n) if *n == 0.0 => false,
                Lit::Text(s) => match s.trim().to_ascii_lowercase().as_str() {
                    "true" | "1" => true,
                    "false" | "0" | "" => false,
                    _ => return None,
                },
                _ => return None,
            };
            Some(Converted::Inline(if b { "TRUE" } else { "FALSE" }))
        }
        Ty::Date => {
            let dt = match lit {
                Lit::Date(d) => *d,
                Lit::Text(s) => datetime::parse(s)?,
                _ => return None,
            };
            Some(Converted::Param(dt.to_rfc3339(), "::timestamptz"))
        }
    }
}

fn text_or_null(value: Option<&String>) -> Val {
    match value {
        Some(s) => Val::Lit(Lit::Text(s.clone())),
        None => Val::Null,
    }
}

fn json_val(value: Option<&Value>) -> Val {
    match value {
        None | Some(Value::Null) => Val::Null,
        Some(Value::Bool(b)) => Val::Lit(Lit::Bool(*b)),
        Some(Value::Number(n)) => Val::Lit(Lit::Num(n.as_f64().unwrap_or(0.0))),
        Some(Value::String(s)) => Val::Lit(Lit::Text(s.clone())),
        Some(Value::Array(items)) => {
            let texts: Vec<Value> = items
                .iter()
                .map(|v| match v {
                    Value::String(s) => Value::String(s.clone()),
                    other => Value::String(other.to_string()),
                })
                .collect();
            Val::List(Value::Array(texts).to_string())
        }
        Some(obj @ Value::Object(_)) => Val::Lit(Lit::Text(obj.to_string())),
    }
}

fn lit_text(lit: &Lit) -> String {
    match lit {
        Lit::Text(s) => s.clone(),
        Lit::Num(n) => n.to_string(),
        Lit::Bool(b) => b.to_string(),
        Lit::Date(d) => datetime::format(d),
    }
}

fn scalar_text(sql: &str, ty: Ty) -> String {
    match ty {
        Ty::Text | Ty::Dyn => sql.to_string(),
        Ty::Number | Ty::Bool => format!("({sql})::text"),
        Ty::Date => datetime::sql_format(sql),
    }
}

fn to_num(sql: &str) -> String {
    format!("_slonbase.to_num({sql})")
}

fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::model::*;
    use crate::filter::parse;

    fn collection() -> Collection {
        serde_json::from_value(serde_json::json!({
            "id": "pbc_test",
            "name": "posts",
            "fields": [
                default_id_field(),
                { "id": "f1", "name": "title", "type": "text" },
                { "id": "f2", "name": "views", "type": "number" },
                { "id": "f3", "name": "active", "type": "bool" },
                { "id": "f4", "name": "published", "type": "date" },
                { "id": "f5", "name": "tags", "type": "select", "values": ["a", "b"], "maxSelect": 3 },
                { "id": "f6", "name": "meta", "type": "json" },
                { "id": "f7", "name": "secret", "type": "text", "hidden": true }
            ]
        }))
        .unwrap()
    }

    fn compile_str(
        filter: &str,
        allow_hidden: bool,
    ) -> Result<(String, Vec<Option<String>>), CompileError> {
        let c = collection();
        let mut req = RequestInfo::new("GET");
        req.body.insert("title".into(), "x".into());
        req.body.insert("ids".into(), serde_json::json!(["a", "b"]));
        req.headers.insert("x_token".into(), "abc".into());
        let scope = Scope {
            collection: &c,
            alias: "t",
            request: &req,
            allow_hidden,
        };
        let mut params = Params::default();
        let expr = parse(filter).map_err(|e| CompileError(e.0))?;
        let sql = compile(&expr, &scope, &mut params)?;
        Ok((sql, params.0))
    }

    fn sql(filter: &str) -> String {
        compile_str(filter, false).unwrap().0
    }

    #[test]
    fn typed_comparisons() {
        assert_eq!(sql("title = 'abc'"), r#"(t."title" = $1::text)"#);
        assert_eq!(
            sql("views > 10"),
            r#"(t."views" > ($1::text)::double precision)"#
        );
        // a numeric string compares as a number
        assert_eq!(
            sql("views = '10'"),
            r#"(t."views" = ($1::text)::double precision)"#
        );
        // a non-numeric string falls back to text
        assert_eq!(sql("views = 'abc'"), r#"((t."views")::text = $1::text)"#);
        assert_eq!(sql("active = true"), r#"(t."active" = TRUE)"#);
        assert_eq!(sql("active = 1"), r#"(t."active" = TRUE)"#);
        assert_eq!(
            sql("published >= '2024-01-01'"),
            r#"(t."published" >= ($1::text)::timestamptz)"#
        );
        assert_eq!(
            sql("title != 'x'"),
            r#"(t."title" IS DISTINCT FROM $1::text)"#
        );
        assert_eq!(sql("title < 'x'"), r#"(t."title" COLLATE "C" < $1::text)"#);
    }

    #[test]
    fn literals_on_the_left_and_both_sides() {
        assert_eq!(
            sql("10 < views"),
            r#"(($1::text)::double precision < t."views")"#
        );
        assert_eq!(
            sql("1 = 1"),
            "(($1::text)::double precision = ($2::text)::double precision)"
        );
        assert_eq!(sql("views = title"), r#"((t."views")::text = t."title")"#);
    }

    #[test]
    fn null_comparisons() {
        assert_eq!(sql("title = null"), r#"(t."title" = '')"#);
        assert_eq!(sql("title != null"), r#"NOT (t."title" = '')"#);
        assert_eq!(sql("published = null"), r#"(t."published" IS NULL)"#);
        assert_eq!(sql("views = null"), "FALSE");
        assert_eq!(sql("null = null"), "TRUE");
        assert_eq!(sql("tags = null"), sql("tags = ''"));
    }

    #[test]
    fn like() {
        let (sql, params) = compile_str("title ~ 'a_b%'", false).unwrap();
        assert_eq!(sql, r#"(t."title" ILIKE $1::text ESCAPE '\')"#);
        assert_eq!(params, vec![Some("a_b%".into())]);

        let (_, params) = compile_str("title ~ 'a_b'", false).unwrap();
        assert_eq!(params, vec![Some(r"%a\_b%".into())]);

        assert_eq!(
            compile_str("title !~ 'x'", false).unwrap().0,
            r#"((t."title" ILIKE $1::text ESCAPE '\') IS NOT TRUE)"#
        );
    }

    #[test]
    fn multi_valued_fields() {
        let all = sql("tags = 'a'");
        assert!(
            all.starts_with("NOT EXISTS (SELECT 1 FROM jsonb_array_elements_text("),
            "{all}"
        );
        assert!(
            all.ends_with(r#"WHERE ((__e1.v = $1::text)) IS NOT TRUE)"#),
            "{all}"
        );

        let any = sql("tags ?= 'a'");
        assert!(any.starts_with("EXISTS (SELECT 1 FROM"), "{any}");
        assert!(any.ends_with("WHERE (__e1.v = $1::text))"), "{any}");

        assert_eq!(
            sql("tags:length > 1"),
            r#"(jsonb_array_length(t."tags") > ($1::text)::double precision)"#
        );

        // literal lists from the request body
        let (list, params) = compile_str("@request.body.ids ?= id", false).unwrap();
        assert!(list.contains("($1::text)::jsonb"), "{list}");
        assert_eq!(params[0].as_deref(), Some(r#"["a","b"]"#));
    }

    #[test]
    fn json_paths() {
        let (sql, params) = compile_str("meta.a.b = 5", false).unwrap();
        assert_eq!(
            sql,
            r#"(_slonbase.to_num((t."meta" #>> ($1::text)::text[])) = ($2::text)::double precision)"#
        );
        assert_eq!(params[0].as_deref(), Some("{a,b}"));
        assert!(compile_str("meta.a-b = 1", false).is_err());
    }

    #[test]
    fn request_values() {
        let (sql, params) = compile_str("@request.auth.id != ''", false).unwrap();
        assert_eq!(sql, "NOT TRUE");
        assert!(params.is_empty());

        let (_, params) = compile_str("title = @request.body.title", false).unwrap();
        assert_eq!(params, vec![Some("x".into())]);

        let (sql, _) = compile_str("@request.body.title:isset = true", false).unwrap();
        assert_eq!(sql, "(TRUE = TRUE)");
        let (sql, _) = compile_str("@request.body.missing:isset = true", false).unwrap();
        assert_eq!(sql, "(FALSE = TRUE)");

        let (_, params) = compile_str("@request.headers.x_token = 'abc'", false).unwrap();
        assert_eq!(params, vec![Some("abc".into()), Some("abc".into())]);

        let (_, params) = compile_str("@request.method = 'GET'", false).unwrap();
        assert_eq!(params[0].as_deref(), Some("GET"));
    }

    #[test]
    fn macros() {
        assert!(sql("published > @now").contains("::timestamptz"));
        assert!(sql("views = @year").contains("::double precision"));
        assert!(compile_str("published > @nope", false).is_err());
    }

    #[test]
    fn hidden_and_unknown_fields() {
        assert!(compile_str("secret = 'x'", false).is_err());
        assert!(compile_str("secret = 'x'", true).is_ok());
        assert!(compile_str("nope = 'x'", true).is_err());
        assert!(compile_str("title.x = 'x'", true).is_err());
        assert!(compile_str("@collection.users.id = 'x'", true).is_err());
        assert!(compile_str("title:each = 'x'", true).is_err());
        assert!(compile_str("tags:lower = 'x'", true).is_err());
    }

    #[test]
    fn values_are_never_inlined() {
        let (sql, params) = compile_str(r#"title = "x'; DROP TABLE posts; --""#, false).unwrap();
        assert!(!sql.contains("DROP"));
        assert_eq!(params[0].as_deref(), Some("x'; DROP TABLE posts; --"));
    }
}
