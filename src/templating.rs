//! Templates and expressions (Jinja syntax): `{{ record.name }}`, `{% if %}`, `{% for %}` and
//! filters, for the text of emails and for passing data between the steps of an automation.
//!
//! One engine with fixed rules, so whatever is typed into a template can only produce text:
//!
//! - **No reach.** A template sees the data it is given and nothing else: there is no include,
//!   no import, no file or network access and no way to call code.
//! - **Bounded.** Rendering stops after a fixed amount of work ([`FUEL`]) and a fixed amount of
//!   output ([`MAX_OUTPUT`]), so a loop typed wrongly can't hold a worker.
//! - **Escaped where it matters.** HTML output ([`Mode::Html`]) escapes every value unless the
//!   template says `| safe`; text output ([`Mode::Text`]) is for subjects, addresses and the
//!   values of a step, which are checked for what they are where they are used.
//! - **Forgiving.** A value that isn't there renders as nothing, as in Jinja.

use minijinja::{AutoEscape, Environment, Value as Jinja};
use serde_json::Value;

/// How much work one rendering or expression may do (instructions of the engine).
const FUEL: u64 = 200_000;
/// The most a rendering may produce, in bytes.
pub const MAX_OUTPUT: usize = 512 * 1024;

/// What the output is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// HTML: values are escaped.
    Html,
    /// Plain text: values are written as they are.
    Text,
}

fn environment(mode: Mode) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_fuel(Some(FUEL));
    // what can make a text far larger than what it was given is not there, or is bounded:
    // fuel counts steps, not bytes, so a single step must not be able to fill memory
    for name in [
        "batch",
        "slice",
        "indent",
        "pprint",
        "format",
        "chain",
        "zip",
        "urlencode",
    ] {
        env.remove_filter(name);
    }
    env.remove_global("namespace");
    env.remove_global("debug");
    env.add_filter("replace", bounded_replace);
    env.add_filter("join", bounded_join);
    // (a value that isn't there, or anything under it, is nothing: not an error)
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Chainable);
    env.set_auto_escape_callback(move |_| match mode {
        Mode::Html => AutoEscape::Html,
        Mode::Text => AutoEscape::None,
    });
    env.add_filter("date", date);
    env
}

fn too_large() -> minijinja::Error {
    minijinja::Error::new(
        minijinja::ErrorKind::InvalidOperation,
        format!("the result would be larger than {} KB", MAX_OUTPUT / 1024),
    )
}

/// `replace`, refused when what it would make is larger than [`MAX_OUTPUT`].
fn bounded_replace(
    state: &minijinja::State,
    value: minijinja::value::StringInput<'_>,
    from: minijinja::value::StringInput<'_>,
    to: minijinja::value::StringInput<'_>,
) -> Result<Jinja, minijinja::Error> {
    let (text, pattern, with) = (value.as_str(), from.as_str(), to.as_str());
    let found = match pattern.is_empty() {
        true => text.chars().count() + 1,
        false => text.matches(pattern).count(),
    };
    let size = text.len() + found.saturating_mul(with.len());
    if size > MAX_OUTPUT {
        return Err(too_large());
    }
    minijinja::filters::replace(state, value, from, to)
}

/// `join`, refused when what it would make is larger than [`MAX_OUTPUT`].
fn bounded_join(
    state: &minijinja::State,
    value: &Jinja,
    joiner: Option<minijinja::value::StringInput<'_>>,
) -> Result<Jinja, minijinja::Error> {
    let between = joiner.as_ref().map_or(0, |j| j.as_str().len());
    let mut size = 0usize;
    if let Ok(items) = value.try_iter() {
        for item in items {
            size = size
                .saturating_add(
                    item.as_str()
                        .map_or_else(|| item.to_string().len(), str::len),
                )
                .saturating_add(between);
            if size > MAX_OUTPUT {
                return Err(too_large());
            }
        }
    }
    minijinja::filters::join(state, value, joiner)
}

/// Operators that can make a text larger than its parts (`*`, `**`, `~`, `+`), and blocks that
/// keep text out of the output (`set` ... `endset`, `macro`, `call`, `filter`), are not part of
/// the language here: what a template or an expression makes goes to the bounded output, or
/// stays the size of what it was given. `code` is the inside of `{{ }}` or `{% %}`, or an
/// expression.
fn allowed_code(code: &str, block: bool) -> Result<(), String> {
    let code = code.trim_matches(|c: char| c == '-' || c == '+' || c.is_whitespace());
    if block {
        let keyword: String = code
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if [
            "set",
            "macro",
            "call",
            "filter",
            "endset",
            "endmacro",
            "endcall",
            "endfilter",
        ]
        .contains(&keyword.as_str())
        {
            return Err(format!(
                "{{% {keyword} %}} isn't available in templates here."
            ));
        }
    }
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in code.chars() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '*' | '~' | '+' => {
                    return Err(format!(
                        "The {c} operator isn't available in templates here: compute values in a step instead."
                    ));
                }
                _ => {}
            },
        }
    }
    Ok(())
}

/// [`allowed_code`] for every `{{ }}` and `{% %}` of a template (a `{% raw %}` block is text).
fn allowed_template(source: &str) -> Result<(), String> {
    let mut rest = source;
    while let Some(start) = rest.find('{') {
        let after = &rest[start..];
        let (close, block) = if after.starts_with("{{") {
            ("}}", false)
        } else if after.starts_with("{%") {
            ("%}", true)
        } else if after.starts_with("{#") {
            ("#}", false)
        } else {
            rest = &rest[start + 1..];
            continue;
        };
        let inner_start = start + 2;
        let Some(len) = find_close(&rest[inner_start..], close) else {
            // (unclosed: the engine says so)
            return Ok(());
        };
        let inner = &rest[inner_start..inner_start + len];
        rest = &rest[inner_start + len + 2..];
        if close == "#}" {
            continue;
        }
        let keyword = inner.trim_start_matches(['-', '+']).trim_start();
        if block && keyword.starts_with("raw") {
            // up to its end, as text
            match rest.find("endraw") {
                Some(end) => match rest[end..].find("%}") {
                    Some(c) => rest = &rest[end + c + 2..],
                    None => return Ok(()),
                },
                None => return Ok(()),
            }
            continue;
        }
        allowed_code(inner, block)?;
    }
    Ok(())
}

/// Where `close` ends the code that starts at `code`, outside its quoted texts.
fn find_close(code: &str, close: &str) -> Option<usize> {
    let bytes = code.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) => {
                if b == b'\\' {
                    i += 1;
                } else if b == q {
                    quote = None;
                }
            }
            None => {
                if b == b'"' || b == b'\'' {
                    quote = Some(b);
                } else if code[i..].starts_with(close) {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// `{{ value | date }}` or `{{ value | date("%d %B %Y") }}`: a date and time as slonbase
/// stores them (`2026-11-10 12:00:00.000Z`) or in RFC 3339, written in a `strftime` format
/// (`%Y-%m-%d` when none is given). Anything that isn't a date is left as it is.
fn date(value: String, format: Option<String>) -> String {
    let parsed = crate::datetime::parse(&value).or_else(|| {
        chrono::DateTime::parse_from_rfc3339(&value)
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc))
    });
    let Some(parsed) = parsed else {
        return value;
    };
    let format = format.unwrap_or_else(|| "%Y-%m-%d".to_string());
    // (a format chrono can't use would panic when written: checked first)
    let items: Vec<_> = chrono::format::StrftimeItems::new(&format).collect();
    if items
        .iter()
        .any(|item| matches!(item, chrono::format::Item::Error))
    {
        return value;
    }
    // (and some items only parse: writing them fails, which `to_string` would panic on)
    use std::fmt::Write;
    let mut out = String::new();
    match write!(out, "{}", parsed.format_with_items(items.into_iter())) {
        Ok(()) => out,
        Err(_) => value,
    }
}

/// An engine error in one line, without the engine's name for the template.
fn describe(err: minijinja::Error) -> String {
    let mut message = match err.detail() {
        Some(detail) => format!("{}: {detail}", err.kind()),
        None => err.kind().to_string(),
    };
    if let Some(line) = err.line() {
        message.push_str(&format!(" (line {line})"));
    }
    message
}

/// Whether `source` contains anything to render.
pub fn is_template(source: &str) -> bool {
    source.contains("{{") || source.contains("{%")
}

/// Checks the syntax of a template.
pub fn check(source: &str) -> Result<(), String> {
    allowed_template(source)?;
    environment(Mode::Text)
        .template_from_str(source)
        .map(|_| ())
        .map_err(describe)
}

/// Checks the syntax of an expression (`steps.orders.count > 0`).
pub fn check_expression(expression: &str) -> Result<(), String> {
    allowed_code(expression, false)?;
    environment(Mode::Text)
        .compile_expression(expression)
        .map(|_| ())
        .map_err(describe)
}

/// What a rendering writes into: it refuses to grow past [`MAX_OUTPUT`], so a template that
/// repeats a large text stops there instead of filling memory first.
#[derive(Default)]
struct Limited {
    out: Vec<u8>,
    full: bool,
}

impl std::io::Write for Limited {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.out.len() + buf.len() > MAX_OUTPUT {
            self.full = true;
            return Err(std::io::Error::other("too large"));
        }
        self.out.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Renders `source` with `data`.
pub fn render(source: &str, data: &Value, mode: Mode) -> Result<String, String> {
    allowed_template(source)?;
    let env = environment(mode);
    let template = env.template_from_str(source).map_err(describe)?;
    let mut out = Limited::default();
    let rendered = template
        .render_captured_to(Jinja::from_serialize(data), &mut out)
        .map(|_| ());
    if out.full {
        return Err(format!(
            "The result is larger than {} KB.",
            MAX_OUTPUT / 1024
        ));
    }
    rendered.map_err(describe)?;
    String::from_utf8(out.out).map_err(|e| e.to_string())
}

/// The value of an expression with `data`.
pub fn eval(expression: &str, data: &Value) -> Result<Value, String> {
    allowed_code(expression, false)?;
    let env = environment(Mode::Text);
    let compiled = env.compile_expression(expression).map_err(describe)?;
    let value = compiled
        .eval(Jinja::from_serialize(data))
        .map_err(describe)?;
    if value.is_undefined() {
        return Ok(Value::Null);
    }
    serde_json::to_value(&value).map_err(|e| e.to_string())
}

/// Whether an expression holds with `data` (as `{% if %}` reads it: nothing, zero, an empty
/// text or list are false).
pub fn holds(expression: &str, data: &Value) -> Result<bool, String> {
    allowed_code(expression, false)?;
    let env = environment(Mode::Text);
    let compiled = env.compile_expression(expression).map_err(describe)?;
    compiled
        .eval(Jinja::from_serialize(data))
        .map(|v| v.is_true())
        .map_err(describe)
}

/// Renders every text in `value` that is a template (the texts of objects and lists too); the
/// rest is kept as it is. A text that is exactly one `{{ expression }}` becomes the value of
/// the expression (a list stays a list, a number a number) rather than its text.
pub fn render_value(value: &Value, data: &Value) -> Result<Value, String> {
    Ok(match value {
        Value::String(s) if is_template(s) => match single_expression(s) {
            Some(expression) => eval(expression, data)?,
            None => Value::String(render(s, data, Mode::Text)?),
        },
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| render_value(v, data))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), render_value(v, data)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

/// The expression of a text that is nothing but `{{ expression }}`.
fn single_expression(source: &str) -> Option<&str> {
    let inner = source.trim().strip_prefix("{{")?.strip_suffix("}}")?;
    (!inner.contains("{{") && !inner.contains("}}") && !inner.contains("{%")).then(|| inner.trim())
}

/// Checks every template in `value` (see [`render_value`]); the error names the first that is
/// wrong.
pub fn check_value(value: &Value) -> Result<(), String> {
    match value {
        Value::String(s) if is_template(s) => match single_expression(s) {
            Some(expression) => check_expression(expression),
            None => check(s),
        },
        Value::Array(items) => items.iter().try_for_each(check_value),
        Value::Object(map) => map.values().try_for_each(check_value),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn html_escapes_values_and_text_does_not() {
        let data = json!({ "name": "<b>Ann</b> & co" });
        assert_eq!(
            render("Hi {{ name }}!", &data, Mode::Html).unwrap(),
            "Hi &lt;b&gt;Ann&lt;&#x2f;b&gt; &amp; co!"
        );
        assert_eq!(
            render("Hi {{ name }}!", &data, Mode::Text).unwrap(),
            "Hi <b>Ann</b> & co!"
        );
        assert_eq!(
            render("{{ name | safe }}", &data, Mode::Html).unwrap(),
            "<b>Ann</b> & co"
        );
    }

    #[test]
    fn loops_conditions_and_missing_values() {
        let data = json!({ "rows": [{ "n": 1 }, { "n": 2 }], "paid": false });
        let out = render(
            "{% for r in rows %}{{ r.n }}{% if not loop.last %},{% endif %}{% endfor %}{% if paid %} paid{% endif %}{{ nothing.here }}",
            &data,
            Mode::Text,
        )
        .unwrap();
        assert_eq!(out, "1,2");
    }

    #[test]
    fn a_runaway_template_is_stopped() {
        let err = render(
            "{% for a in range(100000) %}{% for b in range(100000) %}x{% endfor %}{% endfor %}",
            &json!({}),
            Mode::Text,
        )
        .unwrap_err();
        assert!(err.contains("fuel"), "{err}");
    }

    #[test]
    fn output_stops_at_the_limit_while_it_is_written() {
        // a large text repeated: within the work limit, far over the output limit
        let source = format!(
            "{{% for i in range(10000) %}}{}{{% endfor %}}",
            "x".repeat(90_000)
        );
        let started = std::time::Instant::now();
        let err = render(&source, &json!({}), Mode::Html).unwrap_err();
        assert!(err.contains("larger than"), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        // and a text can't be made large where the output doesn't see it: in an expression,
        // a filter or a block that keeps its text
        let big = "x".repeat(90_000);
        for source in [
            "{{ 'x' * 100000000 }}".to_string(),
            "{{ ('x' * 100000) * 100000 }}".to_string(),
            "{{ 'a' ~ 'b' }}".to_string(),
            "{{ 'a' + 'b' }}".to_string(),
            "{{ 2 ** 3 }}".to_string(),
            format!("{{{{ '{big}' | replace('x', '{big}') | length }}}}"),
            format!("{{{{ range(10000) | join('{big}') | length }}}}"),
            format!(
                "{{% set kept %}}{{% for i in range(10000) %}}{big}{{% endfor %}}{{% endset %}}"
            ),
            "{% set x = 1 %}".to_string(),
            "{% macro m() %}x{% endmacro %}".to_string(),
            "{% filter upper %}x{% endfilter %}".to_string(),
            "{{ [1] | batch(100000000, 'x') | length }}".to_string(),
            "{{ 'x' | indent(100000000) }}".to_string(),
            "{{ namespace() }}".to_string(),
        ] {
            let started = std::time::Instant::now();
            assert!(
                render(&source, &json!({}), Mode::Text).is_err(),
                "{}",
                &source[..source.len().min(80)]
            );
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
        }
        for expression in ["a * 2", "a ~ b", "a + 1", "a ** 2"] {
            assert!(check_expression(expression).is_err(), "{expression}");
            assert!(
                eval(expression, &json!({ "a": "x", "b": "y" })).is_err(),
                "{expression}"
            );
        }
        // operators inside quoted text, comments and raw blocks are text
        assert_eq!(
            render(
                "{{ '1 + 2 * 3 ~ 4' }}{# a * b #}{% raw %}{{ a * b }}{% endraw %}",
                &json!({}),
                Mode::Text
            )
            .unwrap(),
            "1 + 2 * 3 ~ 4{{ a * b }}"
        );
        assert_eq!(
            render(
                "{{- 'x' -}} {%- if true +%}y{% endif %}",
                &json!({}),
                Mode::Text
            )
            .unwrap(),
            "xy"
        );
        assert_eq!(
            render(
                "{{ 'a-b' | replace('-', '+') }} {{ [1, 2] | join(', ') }} {{ 5 - 2 }}",
                &json!({}),
                Mode::Text
            )
            .unwrap(),
            "a+b 1, 2 3"
        );
    }

    #[test]
    fn a_template_cant_reach_outside_its_data() {
        // no include, import or extends: there is nothing to load from
        for source in [
            "{% include 'x' %}",
            "{% import 'x' as y %}",
            "{% extends 'x' %}",
        ] {
            assert!(
                render(source, &json!({}), Mode::Text).is_err(),
                "{source} rendered"
            );
        }
    }

    #[test]
    fn syntax_is_checked_with_the_line() {
        assert!(check("Hello {{ name }}").is_ok());
        let err = check("line one\n{% if x %}never closed").unwrap_err();
        assert!(err.contains("line"), "{err}");
        assert!(check_expression("steps.orders.count > 0").is_ok());
        assert!(check_expression("steps.orders.count >").is_err());
    }

    #[test]
    fn expressions_keep_their_type() {
        let data = json!({ "steps": { "q": { "rows": [{ "email": "a@b.co" }], "count": 1 } } });
        assert_eq!(
            eval("steps.q.rows", &data).unwrap(),
            json!([{ "email": "a@b.co" }])
        );
        assert_eq!(eval("steps.q.count - 1", &data).unwrap(), json!(0));
        assert_eq!(eval("steps.nope.rows", &data).unwrap(), Value::Null);
        assert!(holds("steps.q.count > 0", &data).unwrap());
        assert!(!holds("steps.q.rows | length > 3", &data).unwrap());
        assert!(!holds("steps.nope", &data).unwrap());
    }

    #[test]
    fn values_are_rendered_in_place() {
        let data = json!({ "item": { "id": "abc", "total": 12.5, "tags": ["x"] } });
        let config = json!({
            "id": "{{ item.id }}",
            "data": { "note": "Total {{ item.total }}", "tags": "{{ item.tags }}", "n": 3 },
            "list": ["{{ item.total }}", "plain"],
        });
        assert_eq!(
            render_value(&config, &data).unwrap(),
            json!({
                "id": "abc",
                "data": { "note": "Total 12.5", "tags": ["x"], "n": 3 },
                "list": [12.5, "plain"],
            })
        );
        assert!(check_value(&config).is_ok());
        assert!(check_value(&json!({ "a": ["{{ nope +"] })).is_err());
    }

    #[test]
    fn dates_are_formatted() {
        let data = json!({ "at": "2026-11-10 12:30:00.000Z", "word": "soon" });
        assert_eq!(
            render("{{ at | date }}", &data, Mode::Text).unwrap(),
            "2026-11-10"
        );
        assert_eq!(
            render("{{ at | date('%d %B %Y, %H:%M') }}", &data, Mode::Text).unwrap(),
            "10 November 2026, 12:30"
        );
        assert_eq!(
            render("{{ word | date }}", &data, Mode::Text).unwrap(),
            "soon"
        );
        // (a format item that only parses must not bring the server down)
        assert_eq!(
            render("{{ at | date('%#z') }}", &data, Mode::Text).unwrap(),
            "2026-11-10 12:30:00.000Z"
        );
        // a format that can't be used leaves the value alone
        assert_eq!(
            render("{{ at | date('%Q') }}", &data, Mode::Text).unwrap(),
            "2026-11-10 12:30:00.000Z"
        );
    }
}
