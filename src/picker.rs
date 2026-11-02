//! The `fields` query parameter: a port of PocketBase's picker (`tools/picker`). It keeps the
//! listed keys of a JSON response, with nested paths (`expand.author.name`), `*` for every key
//! at a level (`*,expand.author.id`), and modifiers (`body:excerpt(200,true)`). Lists apply it
//! to each item, and paginated results (`page`, `perPage`, ..., `items`) to their items only.
//!
//! Applied to every successful JSON response (see [`layer`]) and to realtime records.

use std::collections::BTreeMap;
use std::ops::Bound;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value};

/// A parsed `fields` parameter.
#[derive(Debug, Clone, Default)]
pub struct Fields(BTreeMap<String, Option<Modifier>>);

#[derive(Debug, Clone, PartialEq)]
pub enum Modifier {
    Excerpt { max: i64, ellipsis: bool },
}

/// A modifier that couldn't be applied (HTML nested too deep): as in PocketBase, the answer is
/// then sent without picking.
#[derive(Debug)]
struct Failed;

impl Modifier {
    fn apply(&self, value: &mut Value) -> Result<(), Failed> {
        match self {
            Modifier::Excerpt { max, ellipsis } => {
                // other values are kept as they are (e.g. an expand that's missing)
                if let Value::String(html) = value {
                    *html = crate::excerpt::excerpt(html, *max, *ellipsis).map_err(|_| Failed)?;
                }
            }
        }
        Ok(())
    }
}

impl Fields {
    /// Parses `id,title,expand.author.name,body:excerpt(200,true)`.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let mut fields = BTreeMap::new();
        for token in tokenize(raw, &[','], false)? {
            let token = token.trim();
            match token.split_once(':') {
                Some((name, modifier)) => {
                    fields.insert(name.to_string(), Some(parse_modifier(modifier)?));
                }
                None => {
                    fields.insert(token.to_string(), None);
                }
            }
        }
        Ok(Self(fields))
    }

    /// Whether a modifier is used (the picking then takes real work, and can fail).
    pub fn has_modifiers(&self) -> bool {
        self.0.values().any(Option::is_some)
    }

    /// Picks the fields of a response. Paginated results are picked in their items only.
    pub fn pick_response(&self, data: &mut Value) {
        if is_search_result(data) {
            if let Some(items) = data.get_mut("items") {
                self.pick_or_keep(items);
            }
        } else {
            self.pick_or_keep(data);
        }
    }

    /// Picks the fields of a list's items.
    pub fn pick_items(&self, items: &mut Value) {
        self.pick_or_keep(items);
    }

    /// Picks the fields of one record (realtime).
    pub fn pick_record(&self, record: &mut Value) {
        self.pick_or_keep(record);
    }

    /// [`Self::pick_response`], on a blocking thread when there are excerpts to make (HTML
    /// parsing would hold up the async workers).
    pub async fn pick_response_off_thread(&self, mut data: Value) -> Value {
        if !self.has_modifiers() {
            self.pick_response(&mut data);
            return data;
        }
        let fields = self.clone();
        tokio::task::spawn_blocking(move || {
            fields.pick_response(&mut data);
            data
        })
        .await
        .unwrap_or(Value::Null)
    }

    /// [`Self::pick_items`], off the async workers when there are excerpts to make.
    pub async fn pick_items_off_thread(&self, mut items: Value) -> Value {
        if !self.has_modifiers() {
            self.pick_items(&mut items);
            return items;
        }
        let fields = self.clone();
        tokio::task::spawn_blocking(move || {
            fields.pick_items(&mut items);
            items
        })
        .await
        .unwrap_or(Value::Null)
    }

    /// Picks in place, or leaves the data whole when a modifier fails (as PocketBase).
    fn pick_or_keep(&self, data: &mut Value) {
        if !self.has_modifiers() {
            // nothing can fail
            let _ = pick(data, self.0.clone());
            return;
        }
        let mut picked = data.clone();
        if pick(&mut picked, self.0.clone()).is_ok() {
            *data = picked;
        }
    }
}

/// `{"page", "perPage", "totalItems", "totalPages", "items"}`, as the list endpoints answer.
fn is_search_result(data: &Value) -> bool {
    let Value::Object(map) = data else {
        return false;
    };
    map.len() == 5
        && ["page", "perPage", "totalItems", "totalPages"]
            .iter()
            .all(|k| map.contains_key(*k))
        && map.get("items").is_some_and(Value::is_array)
}

type Picks = BTreeMap<String, Option<Modifier>>;

fn pick(data: &mut Value, mut fields: Picks) -> Result<(), Failed> {
    match data {
        Value::Object(map) => pick_map(map, &mut fields)?,
        Value::Array(items) => {
            // lists of objects only, as PocketBase
            if !items.first().is_some_and(Value::is_object) {
                return Ok(());
            }
            for item in items {
                if let Value::Object(map) = item {
                    pick_map(map, &mut fields)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// PocketBase's `strings.HasPrefix(f+".", k+".")`: the field is the key or goes into it.
fn starts_path(field: &str, key: &str) -> bool {
    field.starts_with(key) && (field.len() == key.len() || field.as_bytes()[key.len()] == b'.')
}

/// What's left of the field's path after the key (PocketBase's
/// `TrimSuffix(TrimPrefix(f+".", k+"."), ".")`); empty for the key itself.
fn rest_of_path<'a>(field: &'a str, key: &str) -> &'a str {
    // (the "." appended to f is the one TrimSuffix takes off again)
    if field.len() == key.len() {
        ""
    } else {
        &field[key.len() + 1..]
    }
}

/// The fields that are the key or go into it: those starting with it, a range of the sorted
/// fields (so a long `fields` costs little per key).
fn matching<'a>(
    fields: &'a Picks,
    key: &'a str,
) -> impl Iterator<Item = (&'a String, &'a Option<Modifier>)> {
    fields
        .range::<str, _>((Bound::Included(key), Bound::Unbounded))
        .take_while(move |(f, _)| f.starts_with(key))
        .filter(move |(f, _)| starts_path(f, key))
}

fn pick_map(data: &mut Map<String, Value>, fields: &mut Picks) -> Result<(), Failed> {
    if fields.is_empty() {
        return Ok(());
    }
    // `*` stands for every key that no other field names
    if let Some(star) = fields.get("*").cloned() {
        for key in data.keys() {
            if matching(fields, key).next().is_none() {
                fields.insert(key.clone(), star.clone());
            }
        }
    }
    let mut failed = false;
    data.retain(|key, value| {
        if failed {
            return true;
        }
        let matching: Vec<(&String, &Option<Modifier>)> = matching(fields, key).collect();
        if matching.is_empty() {
            return false;
        }
        // the key itself: kept whole, with its modifier
        if let Some((_, modifier)) = matching
            .iter()
            .find(|(f, _)| rest_of_path(f, key).is_empty())
        {
            if let Some(m) = modifier {
                failed = m.apply(value).is_err();
            }
            return true;
        }
        // deeper paths: what follows `key.`
        let nested: Picks = matching
            .into_iter()
            .map(|(f, m)| (rest_of_path(f, key).to_string(), m.clone()))
            .collect();
        failed = pick(value, nested).is_err();
        true
    });
    if failed { Err(Failed) } else { Ok(()) }
}

fn parse_modifier(raw: &str) -> Result<Modifier, String> {
    let parts = tokenize(raw, &['(', ')', ',', ' '], true)?;
    let Some((name, args)) = parts.split_first() else {
        return Err(format!("invalid or empty modifier expression {raw:?}"));
    };
    match name.as_str() {
        "excerpt" => {
            if args.is_empty() {
                return Err("max argument is required - expected (max, withEllipsis?)".into());
            }
            if args.len() > 2 {
                return Err("too many arguments - expected (max, withEllipsis?)".into());
            }
            let max = go_to_int(&args[0]).unwrap_or(0);
            if max <= 0 {
                return Err("max argument must be > 0".into());
            }
            let ellipsis = args.get(1).is_some_and(|a| go_parse_bool(a));
            Ok(Modifier::Excerpt { max, ellipsis })
        }
        other => Err(format!("missing or invalid modifier {other:?}")),
    }
}

/// PocketBase's tokenizer: tokens split by `separators` outside of quotes (and parentheses,
/// unless ignored), trimmed, empty ones skipped. A NUL ends the input, as in Go.
fn tokenize(raw: &str, separators: &[char], ignore_parens: bool) -> Result<Vec<String>, String> {
    const WHITESPACE: &[char] = &['\t', '\n', '\x0B', '\x0C', '\r', ' ', '\u{85}', '\u{A0}'];
    let trim: Vec<char> = WHITESPACE
        .iter()
        .copied()
        .filter(|c| !separators.contains(c))
        .collect();
    let mut chars = raw.chars().peekable();
    let mut tokens = Vec::new();
    loop {
        // Scan: the end of the input (or a NUL) stops
        match chars.peek() {
            None | Some('\0') => break,
            _ => {}
        }
        let mut buf = String::new();
        let (mut parens, mut quote, mut prev) = (0u32, None::<char>, '\0');
        for ch in chars.by_ref() {
            if ch == '\0' {
                break;
            }
            if prev != '\\' {
                if !ignore_parens && ch == '(' && quote.is_none() {
                    parens += 1;
                } else if !ignore_parens && ch == ')' && parens > 0 && quote.is_none() {
                    parens -= 1;
                } else if matches!(ch, '\'' | '"' | '`') {
                    match quote {
                        Some(q) if q == ch => quote = None,
                        None => quote = Some(ch),
                        Some(_) => {}
                    }
                }
            }
            if separators.contains(&ch) && parens == 0 && quote.is_none() {
                break;
            }
            prev = ch;
            buf.push(ch);
        }
        if parens > 0 || quote.is_some() {
            return Err(format!(
                "unbalanced parenthesis or quoted expression: {buf:?}"
            ));
        }
        let token = buf.trim_matches(trim.as_slice());
        if !token.is_empty() {
            tokens.push(token.to_string());
        }
    }
    Ok(tokens)
}

/// spf13/cast's ToInt of a string, as PocketBase reads `max`: Go's `strconv.ParseInt(s, 0, 64)`
/// (`0x`, `0b`, `0o` and leading-zero octal, `_` between digits), or a plain decimal with
/// its fraction dropped (`12.7` is 12).
fn go_to_int(s: &str) -> Option<i64> {
    if let Some((int, frac)) = s.split_once('.') {
        let digits = int.strip_prefix(['+', '-']).unwrap_or(int);
        if digits.is_empty()
            || !digits.bytes().all(|b| b.is_ascii_digit())
            || !frac.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        return int.parse::<i64>().ok();
    }
    go_parse_int(s)
}

fn go_parse_int(s: &str) -> Option<i64> {
    if !underscore_ok(s) {
        return None;
    }
    let (negative, unsigned) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let lower = unsigned.to_ascii_lowercase();
    let (base, digits) = if let Some(d) = lower.strip_prefix("0x") {
        (16, d)
    } else if let Some(d) = lower.strip_prefix("0b") {
        (2, d)
    } else if let Some(d) = lower.strip_prefix("0o") {
        (8, d)
    } else if lower.len() > 1 && lower.starts_with('0') {
        (8, &lower[1..])
    } else {
        (10, lower.as_str())
    };
    let digits = digits.replace('_', "");
    if digits.is_empty() || digits.starts_with(['+', '-']) {
        return None;
    }
    let magnitude = u64::from_str_radix(&digits, base).ok()?;
    if negative {
        if magnitude > i64::MAX as u64 + 1 {
            return None;
        }
        Some((magnitude as i64).wrapping_neg())
    } else {
        i64::try_from(magnitude).ok()
    }
}

/// Go's `underscoreOK`: underscores only between digits (or after a base prefix).
fn underscore_ok(s: &str) -> bool {
    let s = s.strip_prefix(['+', '-']).unwrap_or(s).as_bytes();
    let mut saw = b'^';
    let mut i = 0;
    let mut hex = false;
    if s.len() >= 2 && s[0] == b'0' && matches!(s[1].to_ascii_lowercase(), b'b' | b'o' | b'x') {
        i = 2;
        saw = b'0';
        hex = s[1].eq_ignore_ascii_case(&b'x');
    }
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_digit() || (hex && c.to_ascii_lowercase().is_ascii_hexdigit()) {
            saw = b'0';
        } else if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
        } else {
            if saw == b'_' {
                return false;
            }
            saw = b'!';
        }
        i += 1;
    }
    saw != b'_'
}

/// Go's `strconv.ParseBool`, false for anything else.
fn go_parse_bool(s: &str) -> bool {
    matches!(s, "1" | "t" | "T" | "TRUE" | "true" | "True")
}

/// Marks a response whose `fields` were already applied by its handler.
#[derive(Clone, Copy, Debug)]
pub struct Picked;

/// PocketBase's answer to a `fields` parameter it can't parse.
fn invalid_fields() -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({
            "data": {},
            "message": "Something went wrong while processing your request.",
            "status": 400,
        })),
    )
        .into_response()
}

/// The `fields` parameter of a request (the first one, as Go's `Query().Get`).
pub fn from_query(query: Option<&str>) -> Option<String> {
    // (most requests have none: skip the parsing)
    let query = query.filter(|q| q.contains("fields"))?;
    form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "fields")
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

/// Applies `fields` to successful JSON responses. A request that would change something is
/// refused before it runs when its `fields` can't be parsed (PocketBase makes the change, then
/// answers 400); other requests get the 400 in place of their JSON answer, as in PocketBase.
pub async fn layer(request: Request, next: Next) -> Response {
    let Some(raw) = from_query(request.uri().query()) else {
        return next.run(request).await;
    };
    let fields = Fields::parse(&raw);
    let changes = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if fields.is_err() && changes {
        return invalid_fields();
    }
    let response = next.run(request).await;
    let json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !response.status().is_success() || !json || response.extensions().get::<Picked>().is_some() {
        return response;
    }
    let fields = match fields {
        Ok(fields) => fields,
        Err(_) => return invalid_fields(),
    };
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Ok(data) = serde_json::from_slice::<Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let data = fields.pick_response_off_thread(data).await;
    let body = serde_json::to_vec(&data).unwrap_or_default();
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn picked(data: Value, fields: &str) -> Value {
        let mut data = data;
        Fields::parse(fields).unwrap().pick_response(&mut data);
        data
    }

    #[test]
    fn nested_paths_and_star() {
        let record = json!({
            "id": "r1", "title": "T", "body": "<p>Hello world</p>",
            "expand": { "author": { "id": "a1", "name": "N", "bio": "B" } },
            "meta": { "nested": { "deep": 1, "list": [{ "k": 1, "v": 2 }, { "k": 3, "v": 4 }] } },
        });
        assert_eq!(
            picked(record.clone(), "id,title"),
            json!({ "id": "r1", "title": "T" })
        );
        assert_eq!(
            picked(record.clone(), "id,expand.author.name"),
            json!({ "id": "r1", "expand": { "author": { "name": "N" } } })
        );
        assert_eq!(
            picked(record.clone(), "id,meta.nested.list.k"),
            json!({ "id": "r1", "meta": { "nested": { "list": [{ "k": 1 }, { "k": 3 }] } } })
        );
        let all = picked(record.clone(), "*,expand.author.id");
        assert_eq!(all["expand"], json!({ "author": { "id": "a1" } }));
        assert_eq!(all["title"], "T");
        // the key itself wins over its deeper paths
        assert_eq!(
            picked(record.clone(), "expand,expand.author.id")["expand"],
            record["expand"]
        );
        // a path into a value that isn't an object keeps the value
        assert_eq!(picked(record.clone(), "id.x"), json!({ "id": "r1" }));
        assert_eq!(
            picked(record.clone(), "id,body:excerpt(5,true)"),
            json!({ "id": "r1", "body": "Hello..." })
        );
        assert_eq!(picked(record, "*:excerpt(1)")["title"], "T");
    }

    #[test]
    fn lists_and_paginated_results() {
        let list = json!({ "page": 1, "perPage": 2, "totalItems": 2, "totalPages": 1,
                           "items": [{ "id": "a", "x": 1 }, { "id": "b", "x": 2 }] });
        assert_eq!(
            picked(list.clone(), "id"),
            json!({ "page": 1, "perPage": 2, "totalItems": 2, "totalPages": 1,
                    "items": [{ "id": "a" }, { "id": "b" }] })
        );
        assert_eq!(picked(list, "page")["items"], json!([{}, {}]));
        assert_eq!(
            picked(json!([{ "a": 1, "b": 2 }]), "a"),
            json!([{ "a": 1 }])
        );
        assert_eq!(picked(json!(["x"]), "a"), json!(["x"]));
        assert_eq!(
            picked(
                json!({ "token": "t", "record": { "id": "1", "email": "e" } }),
                "record.email"
            ),
            json!({ "record": { "email": "e" } })
        );
    }

    #[test]
    fn parsing_like_pocketbase() {
        for ok in [
            "id",
            " id , title ",
            "id,,title",
            "body:excerpt(5 true)",
            "body:excerpt(5,,true)",
            "body:excerpt(5)x",
            "body: excerpt(5)",
            "body:excerpt (5)",
            "body:excerpt((5))",
            "body:excerpt(12.7)",
            "body:excerpt(0x10)",
            "body:excerpt(0_10)",
            "body:excerpt(+3.2)",
        ] {
            assert!(Fields::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "id,(title",
            "id,'title",
            "body:excerpt('5',true)",
            "body:excerpt(0)",
            "body:excerpt(-3)",
            "body:excerpt()",
            "body:excerpt",
            "body:excerpt(1,2,3)",
            "body:nope(1)",
            "body:excerpt(1e1)",
            "body:excerpt(08)",
            "body:excerpt(1__0)",
            "body:excerpt(.5)",
            "body:excerpt(0x1.8)",
            "body:excerpt(99999999999999999999)",
            "body:",
        ] {
            assert!(Fields::parse(bad).is_err(), "{bad}");
        }
        let max = |f: &str| match Fields::parse(f).unwrap().0["body"].clone() {
            Some(Modifier::Excerpt { max, ellipsis }) => (max, ellipsis),
            None => unreachable!(),
        };
        assert_eq!(max("body:excerpt(12.7)"), (12, false));
        assert_eq!(max("body:excerpt(0x10,1)"), (16, true));
        assert_eq!(max("body:excerpt(010,True)"), (8, true));
        assert_eq!(max("body:excerpt(0b11,yes)"), (3, false));
        assert_eq!(max("body:excerpt(0o7,tRuE)"), (7, false));
        // the name keeps its spaces, as in PocketBase
        assert!(
            Fields::parse("body :excerpt(5)")
                .unwrap()
                .0
                .contains_key("body ")
        );
        // a NUL ends the input
        assert_eq!(Fields::parse("a\0b").unwrap().0.len(), 2);
        assert_eq!(Fields::parse("a\0\0b").unwrap().0.len(), 1);
    }
}

#[cfg(test)]
mod cost {
    use super::*;

    #[test]
    #[ignore]
    fn worst_case_cost() {
        // a page of 1000 records with 30 keys, and as many fields as an 8 KiB URL holds
        let record: Map<String, Value> = (0..30)
            .map(|i| (format!("field{i}"), Value::String("x".repeat(50))))
            .collect();
        let page = Value::Array(vec![Value::Object(record); 1000]);
        let raw: String = (0..2700)
            .map(|i| format!("f{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let fields = Fields::parse(&format!("*,{raw}")).unwrap();
        let started = std::time::Instant::now();
        let mut data = page.clone();
        fields.pick_items(&mut data);
        eprintln!("{} fields over 1000x30: {:?}", raw.len(), started.elapsed());
    }
}
