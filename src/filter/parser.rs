//! Parser for the PocketBase filter syntax, e.g.
//! `title ~ 'abc' && (status = "published" || @request.auth.id != '')`.
//!
//! Grammar:
//! ```text
//! expr    := and ("||" and)*
//! and     := primary ("&&" primary)*
//! primary := "(" expr ")" | operand op operand
//! operand := 'text' | "text" | number | identifier
//! op      := = != ~ !~ < <= > >=   (each optionally prefixed with ? for "any of")
//! ```
//! `&&` binds tighter than `||`, as in SQL. `// comments` run to the end of the line.

use std::fmt;

/// Maximum number of comparisons in a single expression (same as PocketBase).
pub const MAX_EXPRESSIONS: usize = 200;
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Cmp(Cmp),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cmp {
    pub left: Operand,
    pub op: Op,
    /// `?` prefixed operator: matches if any item of a multi-valued operand matches.
    pub any: bool,
    pub right: Operand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Neq,
    Like,
    NotLike,
    Lt,
    Lte,
    Gt,
    Gte,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Text(String),
    /// Kept as written so that no precision is lost before it's validated.
    Number(String),
    /// A field name, a `@macro` or a `@request.*` / `@collection.*` path,
    /// possibly with `:modifiers`.
    Ident(String),
    /// A function call, e.g. `geoDistance(lon, lat, 23.3, 42.7)`.
    Call {
        name: String,
        args: Vec<Operand>,
    },
}

/// Most arguments a function takes.
const MAX_ARGS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(msg: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError(msg.into()))
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    LParen,
    RParen,
    And,
    Or,
    Op(Op, bool),
    Operand(Operand),
}

/// A `{:name}` placeholder's value as an operand.
fn placeholder(value: &serde_json::Value) -> Operand {
    use serde_json::Value;
    match value {
        Value::Null => Operand::Ident("null".into()),
        Value::Bool(b) => Operand::Ident(b.to_string()),
        Value::Number(n) => Operand::Number(n.to_string()),
        Value::String(s) => Operand::Text(s.clone()),
        other => Operand::Text(other.to_string()),
    }
}

fn tokenize(
    input: &str,
    params: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<Vec<Token>, ParseError> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            '{' if params.is_some() && chars.get(i + 1) == Some(&':') => {
                let start = i + 2;
                let Some(len) = chars[start..].iter().position(|c| *c == '}') else {
                    return err("unterminated placeholder");
                };
                let name: String = chars[start..start + len].iter().collect();
                let value = params
                    .and_then(|p| p.get(name.trim()))
                    .ok_or_else(|| ParseError(format!("missing placeholder value {name:?}")))?;
                tokens.push(Token::Operand(placeholder(value)));
                i = start + len + 1;
            }
            '&' | '|' => {
                if chars.get(i + 1) != Some(&c) {
                    return err(format!("invalid operator {c:?}, expected \"{c}{c}\""));
                }
                tokens.push(if c == '&' { Token::And } else { Token::Or });
                i += 2;
            }
            '\'' | '"' => {
                let quote = c;
                let mut text = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return err("unterminated text literal"),
                        Some('\\') if chars.get(i + 1) == Some(&quote) => {
                            text.push(quote);
                            i += 2;
                        }
                        Some(&ch) if ch == quote => {
                            i += 1;
                            break;
                        }
                        Some(&ch) => {
                            text.push(ch);
                            i += 1;
                        }
                    }
                }
                tokens.push(Token::Operand(Operand::Text(text)));
            }
            '=' | '!' | '~' | '<' | '>' | '?' => {
                let any = c == '?';
                let start = if any { i + 1 } else { i };
                let (op, len) = match (chars.get(start), chars.get(start + 1)) {
                    (Some('!'), Some('=')) => (Op::Neq, 2),
                    (Some('!'), Some('~')) => (Op::NotLike, 2),
                    (Some('<'), Some('=')) => (Op::Lte, 2),
                    (Some('>'), Some('=')) => (Op::Gte, 2),
                    (Some('='), _) => (Op::Eq, 1),
                    (Some('~'), _) => (Op::Like, 1),
                    (Some('<'), _) => (Op::Lt, 1),
                    (Some('>'), _) => (Op::Gt, 1),
                    _ => return err(format!("invalid operator at position {i}")),
                };
                tokens.push(Token::Op(op, any));
                i = start + len;
            }
            c if c.is_ascii_digit()
                || (c == '-' && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit())) =>
            {
                let start = i;
                i += 1;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let number: String = chars[start..i].iter().collect();
                if number.parse::<f64>().is_err() || number.ends_with('.') {
                    return err(format!("invalid number {number:?}"));
                }
                tokens.push(Token::Operand(Operand::Number(number)));
            }
            c if c == '@' || c == '_' || c.is_alphabetic() => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '.' | ':' | '@'))
                {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();
                if chars.get(i) == Some(&'(') {
                    let (args, end) = call_args(&chars, i, params)?;
                    tokens.push(Token::Operand(Operand::Call { name: ident, args }));
                    i = end;
                    continue;
                }
                if ident.ends_with('.') || ident.ends_with(':') || ident.contains("..") {
                    return err(format!("invalid identifier {ident:?}"));
                }
                tokens.push(Token::Operand(Operand::Ident(ident)));
            }
            _ => return err(format!("unexpected character {c:?}")),
        }
    }

    Ok(tokens)
}

/// The arguments of a call whose `(` is at `open`, and the position after its `)`.
fn call_args(
    chars: &[char],
    open: usize,
    params: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<(Vec<Operand>, usize), ParseError> {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    let mut parts = Vec::new();
    let mut start = open + 1;
    let mut i = open;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                if c == '\\' && chars.get(i + 1) == Some(&q) {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        parts.push(chars[start..i].iter().collect::<String>());
                        break;
                    }
                }
                ',' if depth == 1 => {
                    parts.push(chars[start..i].iter().collect::<String>());
                    start = i + 1;
                }
                _ => {}
            },
        }
        i += 1;
    }
    if i >= chars.len() {
        return err("missing closing parenthesis of a function call");
    }
    if parts.len() == 1 && parts[0].trim().is_empty() {
        parts.clear();
    }
    if parts.len() > MAX_ARGS {
        return err("too many function arguments");
    }
    let mut args = Vec::with_capacity(parts.len());
    for part in parts {
        match tokenize(&part, params)?.as_slice() {
            [Token::Operand(operand)] => args.push(operand.clone()),
            _ => return err(format!("invalid function argument {:?}", part.trim())),
        }
    }
    Ok((args, i + 1))
}

pub fn parse(input: &str) -> Result<Expr, ParseError> {
    parse_tokens(tokenize(input, None)?)
}

/// Parses an expression with `{:name}` placeholders, replaced by the values in `params`
/// (as literals, so they can't change the expression).
pub fn parse_with_params(
    input: &str,
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<Expr, ParseError> {
    parse_tokens(tokenize(input, Some(params))?)
}

fn parse_tokens(tokens: Vec<Token>) -> Result<Expr, ParseError> {
    if tokens.is_empty() {
        return err("empty expression");
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        comparisons: 0,
    };
    let expr = parser.expr(0)?;
    if parser.pos != parser.tokens.len() {
        return err("unexpected token after the end of the expression");
    }
    Ok(expr)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    comparisons: usize,
}

impl Parser {
    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn expr(&mut self, depth: usize) -> Result<Expr, ParseError> {
        let mut left = self.and(depth)?;
        while self.peek() == Some(&Token::Or) {
            self.pos += 1;
            let right = self.and(depth)?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self, depth: usize) -> Result<Expr, ParseError> {
        let mut left = self.primary(depth)?;
        while self.peek() == Some(&Token::And) {
            self.pos += 1;
            let right = self.primary(depth)?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn primary(&mut self, depth: usize) -> Result<Expr, ParseError> {
        match self.next() {
            Some(Token::LParen) => {
                if depth >= MAX_DEPTH {
                    return err("the expression is nested too deeply");
                }
                let inner = self.expr(depth + 1)?;
                match self.next() {
                    Some(Token::RParen) => Ok(inner),
                    _ => err("missing closing parenthesis"),
                }
            }
            Some(Token::Operand(left)) => {
                let (op, any) = match self.next() {
                    Some(Token::Op(op, any)) => (op, any),
                    _ => return err("expected an operator after the left operand"),
                };
                let right = match self.next() {
                    Some(Token::Operand(right)) => right,
                    _ => return err("expected a right operand"),
                };
                self.comparisons += 1;
                if self.comparisons > MAX_EXPRESSIONS {
                    return err(format!(
                        "too many expressions (the limit is {MAX_EXPRESSIONS})"
                    ));
                }
                Ok(Expr::Cmp(Cmp {
                    left,
                    op,
                    any,
                    right,
                }))
            }
            _ => err("expected an expression"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(s: &str) -> Operand {
        Operand::Ident(s.into())
    }
    fn text(s: &str) -> Operand {
        Operand::Text(s.into())
    }
    fn cmp(left: Operand, op: Op, any: bool, right: Operand) -> Expr {
        Expr::Cmp(Cmp {
            left,
            op,
            any,
            right,
        })
    }

    #[test]
    fn function_calls() {
        let expr = parse("geoDistance(loc.lon, loc.lat, 23.3, \"42,7\") < 25").unwrap();
        let Expr::Cmp(cmp) = expr else { panic!() };
        assert_eq!(
            cmp.left,
            Operand::Call {
                name: "geoDistance".into(),
                args: vec![
                    Operand::Ident("loc.lon".into()),
                    Operand::Ident("loc.lat".into()),
                    Operand::Number("23.3".into()),
                    Operand::Text("42,7".into()),
                ],
            }
        );
        assert_eq!(cmp.op, Op::Lt);
    }

    #[test]
    fn placeholders() {
        let params = serde_json::json!({ "title": "it's \\", "n": 5, "ok": true })
            .as_object()
            .cloned()
            .unwrap();
        let expr =
            parse_with_params("title = {:title} && n > {: n } && ok = {:ok}", &params).unwrap();
        let Expr::And(left, ok) = expr else { panic!() };
        let Expr::And(title, n) = *left else { panic!() };
        let Expr::Cmp(title) = *title else { panic!() };
        assert_eq!(title.right, Operand::Text("it's \\".into()));
        let Expr::Cmp(n) = *n else { panic!() };
        assert_eq!(n.right, Operand::Number("5".into()));
        let Expr::Cmp(ok) = *ok else { panic!() };
        assert_eq!(ok.right, Operand::Ident("true".into()));
        assert!(parse_with_params("a = {:missing}", &params).is_err());
        // without params, braces stay invalid
        assert!(parse("a = {:title}").is_err());
    }

    #[test]
    fn parses_operators() {
        let cases = [
            ("a = 'x'", Op::Eq, false),
            ("a != 'x'", Op::Neq, false),
            ("a ~ 'x'", Op::Like, false),
            ("a !~ 'x'", Op::NotLike, false),
            ("a < 'x'", Op::Lt, false),
            ("a <= 'x'", Op::Lte, false),
            ("a > 'x'", Op::Gt, false),
            ("a >= 'x'", Op::Gte, false),
            ("a ?= 'x'", Op::Eq, true),
            ("a ?!= 'x'", Op::Neq, true),
            ("a ?~ 'x'", Op::Like, true),
            ("a ?!~ 'x'", Op::NotLike, true),
            ("a ?<= 'x'", Op::Lte, true),
            ("a?>'x'", Op::Gt, true),
        ];
        for (input, op, any) in cases {
            assert_eq!(
                parse(input).unwrap(),
                cmp(ident("a"), op, any, text("x")),
                "{input}"
            );
        }
    }

    #[test]
    fn parses_operands() {
        assert_eq!(
            parse(r#"@request.auth.id != "" "#).unwrap(),
            cmp(ident("@request.auth.id"), Op::Neq, false, text(""))
        );
        assert_eq!(
            parse("total >= -1.5").unwrap(),
            cmp(
                ident("total"),
                Op::Gte,
                false,
                Operand::Number("-1.5".into())
            )
        );
        assert_eq!(
            parse(r"title = 'it\'s'").unwrap(),
            cmp(ident("title"), Op::Eq, false, text("it's"))
        );
        assert_eq!(
            parse("tags:length > 2").unwrap(),
            cmp(
                ident("tags:length"),
                Op::Gt,
                false,
                Operand::Number("2".into())
            )
        );
        assert_eq!(
            parse("'a' = 'b'").unwrap(),
            cmp(text("a"), Op::Eq, false, text("b"))
        );
    }

    #[test]
    fn precedence_and_grouping() {
        let a = || cmp(ident("a"), Op::Eq, false, Operand::Number("1".into()));
        let b = || cmp(ident("b"), Op::Eq, false, Operand::Number("2".into()));
        let c = || cmp(ident("c"), Op::Eq, false, Operand::Number("3".into()));

        // && binds tighter
        assert_eq!(
            parse("a = 1 || b = 2 && c = 3").unwrap(),
            Expr::Or(
                Box::new(a()),
                Box::new(Expr::And(Box::new(b()), Box::new(c())))
            )
        );
        assert_eq!(
            parse("(a = 1 || b = 2) && c = 3").unwrap(),
            Expr::And(
                Box::new(Expr::Or(Box::new(a()), Box::new(b()))),
                Box::new(c())
            )
        );
        assert_eq!(
            parse("a = 1 // the first\n && b = 2").unwrap(),
            Expr::And(Box::new(a()), Box::new(b()))
        );
    }

    #[test]
    fn rejects_invalid_input() {
        for input in [
            "",
            "   // only a comment",
            "a",
            "a =",
            "= 'x'",
            "a == 'x'",
            "a = 'x' &&",
            "a = 'x' & b = 1",
            "(a = 'x'",
            "a = 'x')",
            "a = 'unterminated",
            "a = 1.",
            "a. = 1",
            "geoDistance(1, 2 < 10",
            "f(a b) = 1",
            "a = 'x' b = 1",
            "a # 1",
        ] {
            assert!(parse(input).is_err(), "{input:?} should fail");
        }
    }

    #[test]
    fn limits() {
        let many = vec!["a = 1"; MAX_EXPRESSIONS + 1].join(" && ");
        assert!(parse(&many).is_err());
        let ok = vec!["a = 1"; MAX_EXPRESSIONS].join(" || ");
        assert!(parse(&ok).is_ok());

        let deep = format!("{}a = 1{}", "(".repeat(40), ")".repeat(40));
        assert!(parse(&deep).is_err());
    }
}
