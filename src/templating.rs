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
    // (a value that isn't there, or anything under it, is nothing: not an error)
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Chainable);
    env.set_auto_escape_callback(move |_| match mode {
        Mode::Html => AutoEscape::Html,
        Mode::Text => AutoEscape::None,
    });
    env.add_filter("date", date);
    env
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
    parsed.format_with_items(items.into_iter()).to_string()
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
    environment(Mode::Text)
        .template_from_str(source)
        .map(|_| ())
        .map_err(describe)
}

/// Checks the syntax of an expression (`steps.orders.count > 0`).
pub fn check_expression(expression: &str) -> Result<(), String> {
    environment(Mode::Text)
        .compile_expression(expression)
        .map(|_| ())
        .map_err(describe)
}

/// Renders `source` with `data`.
pub fn render(source: &str, data: &Value, mode: Mode) -> Result<String, String> {
    let out = environment(mode)
        .render_str(source, Jinja::from_serialize(data))
        .map_err(describe)?;
    if out.len() > MAX_OUTPUT {
        return Err(format!(
            "The result is larger than {} KB.",
            MAX_OUTPUT / 1024
        ));
    }
    Ok(out)
}

/// The value of an expression with `data`.
pub fn eval(expression: &str, data: &Value) -> Result<Value, String> {
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
        assert_eq!(eval("steps.q.count + 1", &data).unwrap(), json!(2));
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
        // a format that can't be used leaves the value alone
        assert_eq!(
            render("{{ at | date('%Q') }}", &data, Mode::Text).unwrap(),
            "2026-11-10 12:30:00.000Z"
        );
    }
}
