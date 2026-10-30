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
use crate::collections::model::{
    Collection, FIELD_EMAIL, FIELD_EMAIL_VISIBILITY, Field, FieldKind, Storage,
};
use crate::collections::schema::table_ref;
use crate::collections::store::Registry;
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
    /// Collections reachable through relation paths (`author.name`). Without it,
    /// relation paths can only point back to the scope's own collection.
    pub registry: Option<std::sync::Arc<Registry>>,
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
            registry: None,
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

    /// The parameters with their (text) type, for `query_typed`.
    pub fn typed(
        &self,
    ) -> Vec<(
        &(dyn tokio_postgres::types::ToSql + Sync),
        tokio_postgres::types::Type,
    )> {
        self.0
            .iter()
            .map(|p| {
                (
                    p as &(dyn tokio_postgres::types::ToSql + Sync),
                    tokio_postgres::types::Type::TEXT,
                )
            })
            .collect()
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

/// Most `@collection` lookups in one expression.
const MAX_LOOKUPS: usize = 6;

fn fail<T>(msg: impl Into<String>) -> Result<T, CompileError> {
    Err(CompileError(msg.into()))
}

pub fn compile(expr: &Expr, scope: &Scope, params: &mut Params) -> Result<String, CompileError> {
    let mut compiler = Compiler {
        scope,
        params,
        aliases: 0,
        lookups: Vec::new(),
    };
    let sql = compiler.expr(expr)?;
    Ok(compiler.finish(sql))
}

/// Splits `@collection.name[:alias].path[:modifier]`.
fn collection_ref(rest: &str) -> Result<(&str, &str, &str), CompileError> {
    let end = rest.find(['.', ':']).unwrap_or(rest.len());
    let name = &rest[..end];
    let mut tail = &rest[end..];
    let mut alias = "";
    if let Some(after) = tail.strip_prefix(':') {
        let dot = after.find('.').unwrap_or(after.len());
        alias = &after[..dot];
        tail = &after[dot..];
    }
    let path = tail
        .strip_prefix('.')
        .filter(|p| !p.is_empty())
        .ok_or_else(|| CompileError(format!("@collection.{rest} needs a field")))?;
    if name.is_empty() {
        return fail("@collection needs a collection name");
    }
    Ok((name, alias, path))
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
    /// Values reached through a multi-valued relation path: a query returning column `v`.
    Set {
        rows: String,
        ty: Ty,
    },
}

struct Compiler<'a, 'p> {
    scope: &'a Scope<'a>,
    params: &'p mut Params,
    aliases: usize,
    /// `@collection` lookups: (collection name, alias) -> (SQL alias, table, join condition)
    lookups: Vec<(String, String, String, String)>,
}

impl Compiler<'_, '_> {
    /// The compiled expression, with its `@collection` lookups joined.
    fn finish(&self, sql: String) -> String {
        if self.lookups.is_empty() {
            return sql;
        }
        // `@collection` references share one row per (collection, alias), like PocketBase's
        // LEFT JOIN: the condition holds when some combination of rows matches it, and
        // parts that don't use a lookup still work when its collection is empty
        let joins: Vec<String> = self
            .lookups
            .iter()
            .map(|(_, sql_alias, table, on)| format!("LEFT JOIN {table} {sql_alias} ON {on}"))
            .collect();
        format!(
            "EXISTS (SELECT 1 FROM (SELECT 1) __one {} WHERE {sql})",
            joins.join(" ")
        )
    }

    /// A client's filter only sees the other collections' records that the client could list:
    /// relation paths and `@collection` lookups get the collection's list rule as a condition
    /// (`FALSE` when only superusers may list it). Rules and superusers see every record.
    fn client_rule(
        &mut self,
        collection: &Collection,
        alias: &str,
    ) -> Result<Option<String>, CompileError> {
        if self.scope.allow_hidden {
            return Ok(None);
        }
        let rule = match collection.list_rule.as_deref().map(str::trim) {
            None => return Ok(Some("FALSE".into())),
            Some("") => return Ok(None),
            Some(rule) => rule,
        };
        let Ok(expr) = super::parser::parse(rule) else {
            return Ok(Some("FALSE".into()));
        };
        let scope = Scope {
            collection,
            alias,
            request: self.scope.request,
            allow_hidden: true,
        };
        let mut inner = Compiler {
            scope: &scope,
            params: &mut *self.params,
            // aliases keep counting, so the rule's never shadow the filter's
            aliases: self.aliases,
            lookups: Vec::new(),
        };
        let compiled = inner.expr(&expr).map(|sql| inner.finish(sql));
        self.aliases = inner.aliases + 1;
        Ok(Some(compiled.unwrap_or_else(|_| "FALSE".into())))
    }

    /// An auth record's email, for a client's filter: only where the record shows it
    /// (`emailVisibility`) or it's the caller's own. Other columns are as they are.
    fn visible_column(
        &mut self,
        collection: &Collection,
        alias: &str,
        field: &Field,
        column: String,
    ) -> String {
        if self.scope.allow_hidden || !collection.is_auth() || field.name != FIELD_EMAIL {
            return column;
        }
        let visible = format!("{alias}.{}", quote_ident(FIELD_EMAIL_VISIBILITY));
        match &self.scope.request.auth {
            Some(auth) if auth.collection_id == collection.id => {
                let me = self.params.push(auth.id.clone());
                format!("(CASE WHEN {visible} OR {alias}.\"id\" = {me} THEN {column} END)")
            }
            _ => format!("(CASE WHEN {visible} THEN {column} END)"),
        }
    }

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
        let left_set = self.set_of(&left);
        let right_set = self.set_of(&right);
        match (left_set, right_set) {
            (Some(_), Some(_)) => fail("both operands can't be multi-valued"),
            (Some((from, item)), None) => self.compare_items(from, item, op, any, right, false),
            (None, Some((from, item))) => self.compare_items(from, item, op, any, left, true),
            (None, None) => self.compare_scalars(left, op, right),
        }
    }

    /// The `FROM` item and the element of a multi-valued operand, `None` for scalars.
    fn set_of(&mut self, val: &Val) -> Option<(String, Val)> {
        let source = match val {
            Val::Multi { source } => source.clone(),
            Val::List(json) => format!("({})::jsonb", self.params.push(json.clone())),
            Val::Set { rows, ty } => {
                self.aliases += 1;
                let alias = format!("__e{}", self.aliases);
                // no related records behave like a single NULL value
                let from = format!(
                    "(SELECT v FROM ({rows}) AS __x UNION ALL SELECT NULL WHERE NOT EXISTS ({rows})) AS {alias}(v)"
                );
                let item = Val::Scalar {
                    sql: format!("{alias}.v"),
                    ty: *ty,
                    nullable: true,
                };
                return Some((from, item));
            }
            _ => return None,
        };
        self.aliases += 1;
        let alias = format!("__e{}", self.aliases);
        // an empty list behaves like [""] (so "tags = ''" matches empty lists)
        let from = format!(
            "jsonb_array_elements_text(CASE WHEN jsonb_array_length({source}) = 0 THEN '[\"\"]'::jsonb ELSE {source} END) AS {alias}(v)"
        );
        let item = Val::Scalar {
            sql: format!("{alias}.v"),
            ty: Ty::Text,
            nullable: false,
        };
        Some((from, item))
    }

    #[allow(clippy::too_many_arguments)]
    fn compare_items(
        &mut self,
        from: String,
        item: Val,
        op: Op,
        any: bool,
        other: Val,
        items_on_right: bool,
    ) -> Result<String, CompileError> {
        let condition = match (&other, op) {
            // "= null" means empty: a missing related record or an empty string
            (Val::Null, Op::Eq) => self.is_empty(&item),
            (Val::Null, Op::Neq) => format!("NOT {}", self.is_empty(&item)),
            _ => {
                let other = match other {
                    Val::Null => Val::Lit(Lit::Text(String::new())),
                    v => v,
                };
                if items_on_right {
                    self.compare_scalars(other, op, item)?
                } else {
                    self.compare_scalars(item, op, other)?
                }
            }
        };
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
            Val::Set { .. } => fail("multi-valued relation paths can't be used here"),
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
            Val::Set { rows, .. } => format!("(NOT EXISTS ({rows}))"),
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
            Operand::Call { name, args } => self.call(name, args),
        }
    }

    fn call(&mut self, name: &str, args: &[Operand]) -> Result<Val, CompileError> {
        match name {
            // the great-circle distance in kilometers (PocketBase's geoDistance)
            "geoDistance" => {
                let [lon1, lat1, lon2, lat2] = args else {
                    return fail("geoDistance() takes 4 arguments: lonA, latA, lonB, latB");
                };
                let mut n = Vec::with_capacity(4);
                for arg in [lon1, lat1, lon2, lat2] {
                    let val = self.operand(arg)?;
                    n.push(self.number_arg(val, name)?);
                }
                let sql = format!(
                    "(2 * 6371 * asin(least(1, sqrt(power(sin(radians({lat2} - {lat1}) / 2), 2) + \
                     cos(radians({lat1})) * cos(radians({lat2})) * power(sin(radians({lon2} - {lon1}) / 2), 2)))))",
                    lon1 = n[0],
                    lat1 = n[1],
                    lon2 = n[2],
                    lat2 = n[3],
                );
                Ok(Val::Scalar {
                    sql,
                    ty: Ty::Number,
                    nullable: false,
                })
            }
            "strftime" => self.strftime(args),
            other => fail(format!("unknown function {other}()")),
        }
    }

    /// `strftime(format, [time-value, modifiers...])`: SQLite's strftime, which PocketBase's
    /// filters expose (`_slonbase.strftime`), with NULL results as `''`. Date fields with the
    /// common formats and modifiers compile to `to_char()` instead, which is much faster.
    fn strftime(&mut self, args: &[Operand]) -> Result<Val, CompileError> {
        let Some(Operand::Text(format_text)) = args.first() else {
            return fail("strftime() takes a format string first");
        };
        if args.len() > 10 {
            return fail("strftime() takes at most 10 arguments");
        }
        let mut modifier_texts = Vec::new();
        for arg in args.iter().skip(2) {
            let Operand::Text(modifier) = arg else {
                return fail("strftime() modifiers must be strings");
            };
            modifier_texts.push(modifier.as_str());
        }
        let format = self.params.push(format_text.clone());
        let modifiers: Vec<String> = modifier_texts
            .iter()
            .map(|m| self.params.push(*m))
            .collect();
        let modifiers = format!("ARRAY[{}]::text[]", modifiers.join(", "));
        let now = format!(
            "({})::bigint",
            self.params
                .push(self.scope.request.now.timestamp_millis().to_string())
        );
        let plan = strftime_plan(format_text, &modifier_texts);
        let call = |c: &mut Self, value: &str, ty: Ty| {
            let slow = format!(
                "coalesce(_slonbase.strftime({format}, {}, {modifiers}, {now}), '')",
                scalar_text(value, ty)
            );
            match &plan {
                Some(plan) if ty == Ty::Date => c.strftime_fast(plan, value, &slow),
                _ => slow,
            }
        };
        let (sql, ty) = match args.get(1) {
            None => (self.params.push("now"), Ty::Text),
            Some(Operand::Call { .. }) => {
                return fail("strftime() takes a date, a number or a field as its time value");
            }
            Some(arg) => match self.operand(arg)? {
                Val::Null => ("NULL::text".to_string(), Ty::Text),
                // booleans are 1 and 0 to SQLite
                Val::Lit(Lit::Bool(b)) => (self.params.push(if b { "1" } else { "0" }), Ty::Text),
                Val::Lit(lit) => (self.params.push(lit_text(&lit)), Ty::Text),
                Val::Scalar {
                    sql, ty: Ty::Bool, ..
                } => (
                    format!("(CASE WHEN {sql} THEN '1' WHEN NOT {sql} THEN '0' END)"),
                    Ty::Text,
                ),
                Val::Scalar { sql, ty, .. } => (sql, ty),
                // multi-valued: each value, as a multi-valued result
                Val::Set { rows, ty } => {
                    let v = call(self, "__s.v", ty);
                    return Ok(Val::Set {
                        rows: format!("SELECT {v} AS v FROM ({rows}) AS __s"),
                        ty: Ty::Text,
                    });
                }
                Val::Multi { source } => {
                    let v = call(self, "__s.v", Ty::Text);
                    return Ok(Val::Set {
                        rows: format!(
                            "SELECT {v} AS v FROM jsonb_array_elements_text({source}) AS __s(v)"
                        ),
                        ty: Ty::Text,
                    });
                }
                Val::List(json) => {
                    let json = self.params.push(json);
                    let v = call(self, "__s.v", Ty::Text);
                    return Ok(Val::Set {
                        rows: format!(
                            "SELECT {v} AS v FROM jsonb_array_elements_text(({json})::jsonb) AS __s(v)"
                        ),
                        ty: Ty::Text,
                    });
                }
            },
        };
        Ok(Val::Scalar {
            sql: call(self, &sql, ty),
            ty: Ty::Text,
            nullable: false,
        })
    }

    /// `strftime()` of a date column with `to_char()`. Every intermediate time must be within
    /// years 2-9999, where PostgreSQL's calendar and SQLite's agree; `slow` handles the rest.
    fn strftime_fast(&mut self, plan: &StrftimePlan, column: &str, slow: &str) -> String {
        let mut times = vec![format!(
            "date_trunc('milliseconds', ({column}) AT TIME ZONE 'UTC')"
        )];
        for shift in &plan.shifts {
            let x = times.last().unwrap();
            times.push(match shift {
                StrftimeShift::By(n, unit) => format!("({x} + interval '{n} {unit}')"),
                // SQLite's month arithmetic: days past the month's end overflow into the next
                StrftimeShift::Months(n) => format!(
                    "(date_trunc('month', {x}) + interval '{n} months' + ({x} - date_trunc('month', {x})))"
                ),
                StrftimeShift::StartOf(unit) => format!("date_trunc('{unit}', {x})"),
                StrftimeShift::Weekday(n) => format!(
                    "({x} + ((({n} - extract(dow FROM {x})::int) + 7) % 7) * interval '1 day')"
                ),
            });
        }
        let x = times.last().unwrap().clone();
        let mut out = Vec::new();
        let mut pattern = String::new();
        for piece in &plan.pieces {
            match piece {
                StrftimePiece::Pattern(p) => {
                    // keep adjacent patterns apart ("ID" "DDD" isn't "IDDD")
                    if !pattern.is_empty() && !pattern.ends_with('"') {
                        pattern.push_str("\"\"");
                    }
                    pattern.push_str(p);
                }
                StrftimePiece::Literal(text) => {
                    pattern.push('"');
                    for c in text.chars() {
                        if c == '"' || c == '\\' {
                            pattern.push('\\');
                        }
                        pattern.push(c);
                    }
                    pattern.push('"');
                }
                StrftimePiece::Expr(expr) => {
                    if !pattern.is_empty() {
                        let p = self.params.push(std::mem::take(&mut pattern));
                        out.push(format!("to_char({x}, {p})"));
                    }
                    out.push(expr(&x));
                }
            }
        }
        if !pattern.is_empty() {
            let p = self.params.push(pattern);
            out.push(format!("to_char({x}, {p})"));
        }
        let fast = if out.is_empty() {
            "''".to_string()
        } else {
            out.join(" || ")
        };
        // each time is checked before the next is computed, so none can overflow
        let mut result = fast;
        for t in times.iter().rev() {
            result = format!(
                "CASE WHEN {t} >= '0002-01-01' AND {t} < '10000-01-01' THEN {result} ELSE {slow} END"
            );
        }
        format!("CASE WHEN ({column}) IS NULL THEN '' ELSE {result} END")
    }

    fn number_arg(&mut self, val: Val, function: &str) -> Result<String, CompileError> {
        match val {
            Val::Lit(Lit::Num(n)) => Ok(format!(
                "({})::double precision",
                self.params.push(n.to_string())
            )),
            Val::Lit(Lit::Text(t)) if t.trim().parse::<f64>().is_ok_and(f64::is_finite) => {
                Ok(format!(
                    "({})::double precision",
                    self.params.push(t.trim().to_string())
                ))
            }
            Val::Scalar {
                sql,
                ty: Ty::Number,
                ..
            } => Ok(sql),
            Val::Scalar {
                sql, ty: Ty::Dyn, ..
            } => Ok(to_num(&sql)),
            _ => fail(format!("{function}() arguments must be numbers")),
        }
    }

    fn ident(&mut self, ident: &str) -> Result<Val, CompileError> {
        if let Some(rest) = ident.strip_prefix("@collection.") {
            return self.collection_value(rest);
        }
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
        if path.starts_with('@') {
            if modifier.is_some() {
                return fail(format!("modifiers can't be used with {path}"));
            }
            return self.macro_value(path);
        }
        self.field_value(path, modifier)
    }

    /// `@collection.name[:alias].path`: a field of another collection's rows.
    fn collection_value(&mut self, rest: &str) -> Result<Val, CompileError> {
        let (name, alias, path) = collection_ref(rest)?;
        let collection = self
            .scope
            .request
            .registry
            .as_ref()
            .and_then(|r| r.get(name).cloned())
            .or_else(|| {
                (self.scope.collection.name == name || self.scope.collection.id == name)
                    .then(|| std::sync::Arc::new(self.scope.collection.clone()))
            })
            .ok_or_else(|| CompileError(format!("unknown collection {name:?}")))?;
        let sql_alias = match self
            .lookups
            .iter()
            .find(|(key, _, _, _)| *key == format!("{}:{alias}", collection.id))
        {
            Some((_, sql_alias, _, _)) => sql_alias.clone(),
            None => {
                if self.lookups.len() >= MAX_LOOKUPS {
                    return fail(format!(
                        "at most {MAX_LOOKUPS} @collection lookups are allowed"
                    ));
                }
                self.aliases += 1;
                let sql_alias = format!("__c{}", self.aliases);
                let on = self
                    .client_rule(&collection, &sql_alias)?
                    .unwrap_or_else(|| "TRUE".into());
                self.lookups.push((
                    format!("{}:{alias}", collection.id),
                    sql_alias.clone(),
                    table_ref(&collection.name),
                    on,
                ));
                sql_alias
            }
        };
        let (path, modifier) = match path.split_once(':') {
            Some((path, modifier)) => (path, Some(modifier)),
            None => (path, None),
        };
        self.field_value_in(&collection, &sql_alias, path, modifier)
    }

    fn field_value(&mut self, path: &str, modifier: Option<&str>) -> Result<Val, CompileError> {
        let (collection, alias) = (self.scope.collection, self.scope.alias);
        self.field_value_in(collection, alias, path, modifier)
    }

    fn field_value_in(
        &mut self,
        collection: &Collection,
        base_alias: &str,
        path: &str,
        modifier: Option<&str>,
    ) -> Result<Val, CompileError> {
        let mut parts = path.split('.');
        let name = parts.next().unwrap_or_default();
        let rest: Vec<&str> = parts.collect();

        let field = match collection.field(name) {
            Some(f) if !f.is_secret() && (!f.hidden || self.scope.allow_hidden) => f,
            _ => return fail(format!("unknown field {name:?}")),
        };
        let column = format!("{base_alias}.{}", quote_ident(&field.name));
        let column = self.visible_column(collection, base_alias, field, column);

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
                FieldKind::Relation(_) => {
                    let column = format!("{base_alias}.{}", quote_ident(&field.name));
                    self.relation_path(field, column, &rest, modifier)
                }
                FieldKind::GeoPoint(_) => match rest.as_slice() {
                    [key @ ("lon" | "lat")] => self.apply_modifier(
                        Val::Scalar {
                            sql: format!("(({column} ->> '{key}')::double precision)"),
                            ty: Ty::Number,
                            nullable: false,
                        },
                        modifier,
                    ),
                    _ => fail(format!("{name:?} only has lon and lat")),
                },
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

    fn related_collection(&self, collection_id: &str) -> Option<std::sync::Arc<Collection>> {
        if collection_id == self.scope.collection.id {
            return Some(std::sync::Arc::new(self.scope.collection.clone()));
        }
        self.scope
            .request
            .registry
            .as_ref()
            .and_then(|r| r.by_id(collection_id).cloned())
    }

    /// `relation.field[.field...]`: a correlated subquery over the related tables. The
    /// result is a scalar when every hop is single-valued, a set otherwise. The related
    /// collections' API rules don't apply (as in PocketBase); hidden fields still do.
    ///
    /// `first_column` is the SQL of the first relation's value (a column, or the caller's
    /// value for `@request.auth` paths).
    fn relation_path(
        &mut self,
        first: &Field,
        first_column: String,
        rest: &[&str],
        modifier: Option<&str>,
    ) -> Result<Val, CompileError> {
        const MAX_HOPS: usize = 6;

        fn target_of(f: &Field) -> String {
            match &f.kind {
                FieldKind::Relation(o) => o.collection_id.clone(),
                _ => String::new(),
            }
        }
        let missing =
            |name: &str| CompileError(format!("the related collection of {name:?} doesn't exist"));

        let mut multi = first.is_multi();
        let mut collection = self
            .related_collection(&target_of(first))
            .ok_or_else(|| missing(&first.name))?;
        self.aliases += 1;
        let mut alias = format!("__r{}", self.aliases);
        let from = format!("{} {alias}", table_ref(&collection.name));
        let mut condition = link(&alias, &first_column, first.is_multi());
        if let Some(rule) = self.client_rule(&collection, &alias)? {
            condition = format!("({condition}) AND ({rule})");
        }
        let mut joins = Vec::new();
        let mut hops = 1;

        let mut i = 0;
        let (value, ty) = loop {
            let part = rest[i];
            let last = i == rest.len() - 1;
            let field = match collection.field(part) {
                Some(f) if !f.is_secret() && (!f.hidden || self.scope.allow_hidden) => f.clone(),
                _ => return fail(format!("unknown field {part:?} in {:?}", collection.name)),
            };
            let column = format!("{alias}.{}", quote_ident(&field.name));
            let column = self.visible_column(&collection, &alias, &field, column);

            if !last {
                match &field.kind {
                    FieldKind::Relation(_) => {
                        hops += 1;
                        if hops > MAX_HOPS {
                            return fail(format!(
                                "relation paths are limited to {MAX_HOPS} levels"
                            ));
                        }
                        multi |= field.is_multi();
                        collection = self
                            .related_collection(&target_of(&field))
                            .ok_or_else(|| missing(part))?;
                        self.aliases += 1;
                        let next = format!("__r{}", self.aliases);
                        let mut on = link(&next, &column, field.is_multi());
                        if let Some(rule) = self.client_rule(&collection, &next)? {
                            on = format!("({on}) AND ({rule})");
                        }
                        joins.push(format!(
                            "JOIN {} {next} ON {on}",
                            table_ref(&collection.name)
                        ));
                        alias = next;
                        i += 1;
                        continue;
                    }
                    FieldKind::Json(_) => {
                        let keys = &rest[i + 1..];
                        if keys.iter().any(|k| {
                            k.is_empty() || !k.chars().all(|c| c.is_alphanumeric() || c == '_')
                        }) {
                            return fail("invalid JSON path");
                        }
                        let path = self.params.push(format!("{{{}}}", keys.join(",")));
                        break (format!("({column} #>> ({path})::text[])"), Ty::Dyn);
                    }
                    _ => return fail(format!("{part:?} has no nested fields")),
                }
            }

            break match field.storage() {
                Storage::Multi => {
                    // the items of a multi-valued field of the related record
                    multi = true;
                    self.aliases += 1;
                    let items = format!("__i{}", self.aliases);
                    joins.push(format!(
                        "CROSS JOIN LATERAL jsonb_array_elements_text({column}) AS {items}(v)"
                    ));
                    (format!("{items}.v"), Ty::Text)
                }
                Storage::Text => (column, Ty::Text),
                Storage::Number => (column, Ty::Number),
                Storage::Bool => (column, Ty::Bool),
                Storage::Date => (column, Ty::Date),
                Storage::Json => (format!("({column} #>> '{{}}')"), Ty::Dyn),
            };
        };

        let rows = format!(
            "SELECT {value} AS v FROM {from} {} WHERE {condition}",
            joins.join(" ")
        );
        if multi {
            if modifier.is_some_and(|m| m != "each") {
                return fail("modifiers are not supported on multi-valued relation paths");
            }
            return Ok(Val::Set { rows, ty });
        }
        let val = Val::Scalar {
            sql: format!("({rows} LIMIT 1)"),
            ty,
            nullable: true,
        };
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
            ("auth", Some(key)) if key.contains('.') => return self.auth_path(key, modifier),
            ("auth", Some(key)) => {
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

    /// `@request.auth.relation.field[...]` (and `@request.auth.json.key`): the caller's
    /// relation values, joined like a record's relation path. A guest, or a caller whose
    /// collection doesn't have the field, gets null. Without a caller (a rule being saved),
    /// the path is checked against every auth collection that has the field.
    fn auth_path(&mut self, key: &str, modifier: Option<&str>) -> Result<Val, CompileError> {
        let (first, rest) = key.split_once('.').expect("a path");
        let rest: Vec<&str> = rest.split('.').collect();
        let usable = |c: &Collection| c.field(first).filter(|f| !f.is_secret()).cloned();
        let Some(auth) = self.scope.request.auth.clone() else {
            let registry = self.scope.request.registry.clone();
            let checked = self.params.0.len();
            for c in registry
                .iter()
                .flat_map(|r| r.all().iter())
                .filter(|c| c.is_auth())
            {
                if let Some(field) = usable(c) {
                    self.auth_field_path(&field, None, &rest, modifier)?;
                }
            }
            self.params.0.truncate(checked);
            return Ok(Val::Null);
        };
        let field = self
            .related_collection(&auth.collection_id)
            .and_then(|c| usable(&c));
        match field {
            Some(field) => self.auth_field_path(&field, auth.values.get(first), &rest, modifier),
            None => Ok(Val::Null),
        }
    }

    fn auth_field_path(
        &mut self,
        field: &Field,
        value: Option<&Value>,
        rest: &[&str],
        modifier: Option<&str>,
    ) -> Result<Val, CompileError> {
        match &field.kind {
            FieldKind::Relation(_) => {
                let first = if field.is_multi() {
                    let ids = match value {
                        Some(v @ Value::Array(_)) => v.to_string(),
                        _ => "[]".into(),
                    };
                    format!("({})::jsonb", self.params.push(ids))
                } else {
                    let id = value.and_then(Value::as_str).unwrap_or_default();
                    self.params.push(id.to_string())
                };
                self.relation_path(field, first, rest, modifier)
            }
            FieldKind::Json(_) => {
                let mut value = value;
                for key in rest {
                    value = value.and_then(|v| v.get(key));
                }
                let val = json_val(value);
                self.apply_modifier(val, modifier)
            }
            _ => fail(format!("{:?} has no nested fields", field.name)),
        }
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

/// Joins a related table (`alias`) to a relation column of the previous one.
fn link(alias: &str, relation_column: &str, multi: bool) -> String {
    if multi {
        format!("{alias}.\"id\" IN (SELECT jsonb_array_elements_text({relation_column}))")
    } else {
        format!("{alias}.\"id\" = {relation_column}")
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

/// A `strftime()` that compiles to `to_char()`: its modifiers, then its format.
struct StrftimePlan {
    shifts: Vec<StrftimeShift>,
    pieces: Vec<StrftimePiece>,
}

enum StrftimeShift {
    /// `+N days|hours|minutes|seconds`
    By(i64, &'static str),
    /// `+N months|years`
    Months(i64),
    /// `start of month|year|day`
    StartOf(&'static str),
    /// `weekday N`
    Weekday(i64),
}

enum StrftimePiece {
    /// A `to_char()` pattern.
    Pattern(&'static str),
    Literal(String),
    /// SQL of a time.
    Expr(fn(&str) -> String),
}

/// The `to_char()` form of a `strftime()`, `None` when it needs the full implementation (other
/// modifiers and formats, or shifts that could leave PostgreSQL's range).
fn strftime_plan(format: &str, modifiers: &[&str]) -> Option<StrftimePlan> {
    if modifiers.len() > 3 {
        return None;
    }
    let mut shifts = Vec::new();
    for m in modifiers {
        let m = m.to_ascii_lowercase();
        let shift = match m.as_str() {
            // the server's zone is UTC, and the value isn't a number
            "localtime" | "utc" | "ceiling" => continue,
            "start of month" => StrftimeShift::StartOf("month"),
            "start of year" => StrftimeShift::StartOf("year"),
            "start of day" => StrftimeShift::StartOf("day"),
            _ => {
                if let Some(n) = m.strip_prefix("weekday ") {
                    if !matches!(n.as_bytes(), [b'0'..=b'6']) {
                        return None;
                    }
                    StrftimeShift::Weekday(n.parse().ok()?)
                } else {
                    let (number, unit) = m.split_once(' ')?;
                    let digits = number.strip_prefix(['+', '-']).unwrap_or(number);
                    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                        return None;
                    }
                    let n: i64 = number.trim_start_matches('+').parse().ok()?;
                    let unit = unit.trim_start_matches(' ');
                    let unit = unit.strip_suffix('s').unwrap_or(unit);
                    // at most ~4,500 years, so no step leaves PostgreSQL's range
                    let (shift, limit) = match unit {
                        "second" => (StrftimeShift::By(n, "seconds"), 140_000_000_000),
                        "minute" => (StrftimeShift::By(n, "minutes"), 2_400_000_000),
                        "hour" => (StrftimeShift::By(n, "hours"), 40_000_000),
                        "day" => (StrftimeShift::By(n, "days"), 1_700_000),
                        "month" => (StrftimeShift::Months(n), 55_000),
                        "year" => (StrftimeShift::Months(n * 12), 4_600),
                        _ => return None,
                    };
                    if n.abs() >= limit {
                        return None;
                    }
                    shift
                }
            }
        };
        shifts.push(shift);
    }

    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            literal.push(c);
            continue;
        }
        let piece = match chars.next()? {
            '%' => {
                literal.push('%');
                continue;
            }
            'd' => StrftimePiece::Pattern("DD"),
            'f' => StrftimePiece::Pattern("SS.MS"),
            'F' => StrftimePiece::Pattern("YYYY-MM-DD"),
            'G' => StrftimePiece::Pattern("IYYY"),
            'H' => StrftimePiece::Pattern("HH24"),
            'I' => StrftimePiece::Pattern("HH12"),
            'j' => StrftimePiece::Pattern("DDD"),
            'm' => StrftimePiece::Pattern("MM"),
            'M' => StrftimePiece::Pattern("MI"),
            'p' => StrftimePiece::Pattern("AM"),
            'R' => StrftimePiece::Pattern("HH24:MI"),
            'S' => StrftimePiece::Pattern("SS"),
            'T' => StrftimePiece::Pattern("HH24:MI:SS"),
            'u' => StrftimePiece::Pattern("ID"),
            'V' => StrftimePiece::Pattern("IW"),
            'Y' => StrftimePiece::Pattern("YYYY"),
            'e' => StrftimePiece::Expr(|x| format!("lpad(to_char({x}, 'FMDD'), 2, ' ')")),
            'k' => StrftimePiece::Expr(|x| format!("lpad(to_char({x}, 'FMHH24'), 2, ' ')")),
            'l' => StrftimePiece::Expr(|x| format!("lpad(to_char({x}, 'FMHH12'), 2, ' ')")),
            'P' => StrftimePiece::Expr(|x| format!("lower(to_char({x}, 'AM'))")),
            'g' => StrftimePiece::Expr(|x| format!("right(to_char({x}, 'IYYY'), 2)")),
            's' => StrftimePiece::Expr(|x| format!("floor(extract(epoch FROM {x}))::bigint::text")),
            'w' => StrftimePiece::Expr(|x| format!("extract(dow FROM {x})::int::text")),
            'U' => StrftimePiece::Expr(|x| {
                format!(
                    "lpad(((extract(doy FROM {x})::int + 6 - extract(dow FROM {x})::int) / 7)::text, 2, '0')"
                )
            }),
            'W' => StrftimePiece::Expr(|x| {
                format!(
                    "lpad(((extract(doy FROM {x})::int + 6 - (extract(dow FROM {x})::int + 6) % 7) / 7)::text, 2, '0')"
                )
            }),
            _ => return None,
        };
        if !literal.is_empty() {
            pieces.push(StrftimePiece::Literal(std::mem::take(&mut literal)));
        }
        pieces.push(piece);
    }
    if !literal.is_empty() {
        pieces.push(StrftimePiece::Literal(literal));
    }
    Some(StrftimePlan { shifts, pieces })
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
        // empty lists match null (like '')
        assert!(sql("tags = null").contains("(__e1.v = '')"));
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
    fn geo_points() {
        let mut c = collection();
        c.fields.push(
            serde_json::from_value(
                serde_json::json!({ "id": "f8", "name": "loc", "type": "geoPoint" }),
            )
            .unwrap(),
        );
        let request = RequestInfo::new("GET");
        let scope = Scope {
            collection: &c,
            alias: "t",
            request: &request,
            allow_hidden: false,
        };
        let mut params = Params::default();
        let sql = crate::filter::to_sql(
            "geoDistance(loc.lon, loc.lat, 23.32, 42.69) < 25 && loc.lat > 40",
            &scope,
            &mut params,
        )
        .unwrap();
        assert!(sql.contains("asin"), "{sql}");
        assert!(
            sql.contains("((t.\"loc\" ->> 'lon')::double precision)"),
            "{sql}"
        );
        assert_eq!(params.0.len(), 4, "the literals are bound");
        for bad in [
            "geoDistance(1, 2, 3) < 1",
            "geoDistance(1, 2, 3, 'x') < 1",
            "nope(1) = 1",
            "loc.alt > 1",
        ] {
            assert!(
                crate::filter::to_sql(bad, &scope, &mut Params::default()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn strftime() {
        let request = RequestInfo::new("GET");
        let c = collection();
        let scope = Scope {
            collection: &c,
            alias: "t",
            request: &request,
            allow_hidden: false,
        };
        let mut params = Params::default();
        let sql = crate::filter::to_sql(
            "strftime('%Y-%m', published, 'start of month', '+1 month') = '2024-02'",
            &scope,
            &mut params,
        )
        .unwrap();
        assert!(
            sql.contains("coalesce(_slonbase.strftime($1::text, to_char("),
            "{sql}"
        );
        assert!(sql.contains("ARRAY[$2::text, $3::text]::text[]"), "{sql}");
        // a date field: to_char() first, the full implementation outside years 2-9999
        assert!(sql.contains("date_trunc('month', "), "{sql}");
        assert!(sql.contains("interval '1 months'"), "{sql}");
        assert!(sql.contains("to_char((date_trunc"), "{sql}");
        assert_eq!(params.0[4].as_deref(), Some("YYYY\"-\"MM"));
        assert_eq!(
            params.0.len(),
            6,
            "format, modifiers, now, pattern and the literal"
        );
        // other values and formats use the full implementation only
        let mut params = Params::default();
        let sql =
            crate::filter::to_sql("strftime('%J', published) = ''", &scope, &mut params).unwrap();
        assert!(!sql.contains("date_trunc"), "{sql}");
        let sql = crate::filter::to_sql("strftime('%Y', title) = ''", &scope, &mut params).unwrap();
        assert!(!sql.contains("date_trunc"), "{sql}");
        for ok in [
            "strftime('%Y') = '2024'",
            "strftime('%s', 0) > 1",
            "strftime('%Y', @now) = ''",
        ] {
            assert!(
                crate::filter::to_sql(ok, &scope, &mut Params::default()).is_ok(),
                "{ok}"
            );
        }
        for bad in [
            "strftime() = ''",
            "strftime(1) = ''",
            "strftime(title) = ''",
            "strftime('%Y', published, 1) = ''",
            "strftime('%Y', published, title) = ''",
            "strftime('%Y', strftime('%Y')) = ''",
        ] {
            assert!(
                crate::filter::to_sql(bad, &scope, &mut Params::default()).is_err(),
                "{bad}"
            );
        }
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
