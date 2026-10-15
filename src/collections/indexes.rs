//! Collection indexes. PocketBase stores them as `CREATE INDEX` statements; slonbase accepts
//! the same strings but only a safe subset: plain column lists, and partial indexes whose
//! `WHERE` compares columns with literals (`=`, `!=`, `<`, ..., `IS [NOT] NULL`, combined with
//! `AND`, `OR`, `NOT` and parentheses), like the ones PocketBase makes for auth collections.
//! The SQL that runs is always regenerated from the parsed form with quoted identifiers.

use crate::filter::quote_ident;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    pub unique: bool,
    /// Column names and whether they're sorted descending.
    pub columns: Vec<(String, bool)>,
    /// The condition of a partial index.
    pub predicate: Option<Vec<Pred>>,
}

/// A token of a partial index's condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pred {
    Column(String),
    /// A string literal (unquoted).
    Text(String),
    /// A number, `TRUE`, `FALSE` or `NULL`, as written (validated).
    Literal(String),
    /// `=`, `!=`, `<`, `<=`, `>`, `>=`.
    Op(&'static str),
    /// `AND`, `OR`, `NOT`, `IS`, `(`, `)`.
    Word(&'static str),
}

impl Index {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let tokens = tokenize(raw)?;

        // simple recursive-descent over the token list
        let mut pos = 0;
        let next = |pos: &mut usize| -> Option<&str> {
            let w = tokens.get(*pos).map(String::as_str);
            *pos += 1;
            w
        };
        let is = |w: Option<&str>, word: &str| w.is_some_and(|w| w.eq_ignore_ascii_case(word));

        if !is(next(&mut pos), "CREATE") {
            return Err("expected CREATE".into());
        }
        let mut unique = false;
        let mut word = next(&mut pos);
        if is(word, "UNIQUE") {
            unique = true;
            word = next(&mut pos);
        }
        if !is(word, "INDEX") {
            return Err("expected INDEX".into());
        }
        let mut word = next(&mut pos);
        if is(word, "IF") {
            if !is(next(&mut pos), "NOT") || !is(next(&mut pos), "EXISTS") {
                return Err("expected IF NOT EXISTS".into());
            }
            word = next(&mut pos);
        }
        let name = identifier(word).ok_or("invalid index name")?;
        if !is(next(&mut pos), "ON") {
            return Err("expected ON".into());
        }
        // the table name is validated by the caller against the collection name
        identifier(next(&mut pos)).ok_or("invalid table name")?;
        if next(&mut pos) != Some("(") {
            return Err("expected a column list".into());
        }

        let mut columns = Vec::new();
        loop {
            let column = identifier(next(&mut pos)).ok_or("invalid column name")?;
            let mut desc = false;
            let mut word = next(&mut pos);
            if is(word, "ASC") {
                word = next(&mut pos);
            } else if is(word, "DESC") {
                desc = true;
                word = next(&mut pos);
            }
            columns.push((column, desc));
            match word {
                Some(",") => continue,
                Some(")") => break,
                Some(w) if w.eq_ignore_ascii_case("COLLATE") => {
                    return Err("COLLATE is not supported in indexes".into());
                }
                _ => return Err("only plain column lists are supported in indexes".into()),
            }
        }
        let predicate = match next(&mut pos) {
            None | Some(";") if pos >= tokens.len() => None,
            Some(w) if w.eq_ignore_ascii_case("WHERE") => {
                Some(parse_predicate(where_clause(raw)?)?)
            }
            _ => return Err("unexpected text after the column list".into()),
        };

        Ok(Self {
            name,
            unique,
            columns,
            predicate,
        })
    }

    /// The table name used in the raw statement.
    pub fn table_of(raw: &str) -> Option<String> {
        let tokens = tokenize(raw).ok()?;
        let on = tokens.iter().position(|t| t.eq_ignore_ascii_case("ON"))?;
        identifier(tokens.get(on + 1).map(String::as_str))
    }

    /// The canonical PocketBase-style statement stored in the collection.
    pub fn to_pocketbase_sql(&self, table: &str) -> String {
        let columns: Vec<String> = self
            .columns
            .iter()
            .map(|(c, desc)| format!("`{c}`{}", if *desc { " DESC" } else { "" }))
            .collect();
        format!(
            "CREATE {}INDEX `{}` ON `{}` ({}){}",
            if self.unique { "UNIQUE " } else { "" },
            self.name,
            table,
            columns.join(", "),
            self.where_sql(|c| format!("`{c}`"))
        )
    }

    /// ` WHERE ...` for a partial index (empty otherwise), with columns quoted by `quote`.
    fn where_sql(&self, quote: impl Fn(&str) -> String) -> String {
        let Some(predicate) = &self.predicate else {
            return String::new();
        };
        let parts: Vec<String> = predicate
            .iter()
            .map(|p| match p {
                Pred::Column(c) => quote(c),
                Pred::Text(t) => format!("'{}'", t.replace('\'', "''")),
                Pred::Literal(l) => l.clone(),
                Pred::Op(o) => (*o).to_string(),
                Pred::Word(w) => (*w).to_string(),
            })
            .collect();
        format!(" WHERE {}", parts.join(" "))
    }

    /// The PostgreSQL statement.
    pub fn to_postgres_sql(&self, table_ref: &str) -> String {
        let columns: Vec<String> = self
            .columns
            .iter()
            .map(|(c, desc)| format!("{}{}", quote_ident(c), if *desc { " DESC" } else { "" }))
            .collect();
        format!(
            "CREATE {}INDEX {} ON {} ({}){}",
            if self.unique { "UNIQUE " } else { "" },
            quote_ident(&self.name),
            table_ref,
            columns.join(", "),
            self.where_sql(quote_ident)
        )
    }
}

/// The text after the `WHERE` of an index statement (outside quoted identifiers).
fn where_clause(raw: &str) -> Result<&str, String> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'`' | b'[' | b'\'' => {
                let close = match bytes[i] {
                    b'[' => b']',
                    c => c,
                };
                i += 1;
                while i < bytes.len() && bytes[i] != close {
                    i += 1;
                }
                i += 1;
            }
            _ => {
                let boundary = |j: usize| {
                    j >= bytes.len() || !(bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
                };
                if raw[i..].len() >= 5
                    && raw[i..i + 5].eq_ignore_ascii_case("WHERE")
                    && (i == 0 || boundary(i - 1))
                    && boundary(i + 5)
                {
                    return Ok(raw[i + 5..].trim().trim_end_matches(';').trim());
                }
                i += 1;
            }
        }
    }
    Err("expected WHERE".into())
}

/// Longest condition of a partial index (in tokens).
const MAX_PREDICATE_TOKENS: usize = 64;

/// Parses and checks a partial index's condition: comparisons of columns with literals.
fn parse_predicate(text: &str) -> Result<Vec<Pred>, String> {
    let invalid = || "only comparisons of columns with literals are supported in WHERE".to_string();
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let token = match c {
            '(' => {
                i += 1;
                Pred::Word("(")
            }
            ')' => {
                i += 1;
                Pred::Word(")")
            }
            '\'' => {
                let mut value = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err("unterminated string in WHERE".into()),
                        Some('\'') if chars.get(i + 1) == Some(&'\'') => {
                            value.push('\'');
                            i += 2;
                        }
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some(ch) => {
                            value.push(*ch);
                            i += 1;
                        }
                    }
                }
                Pred::Text(value)
            }
            '"' | '`' | '[' => {
                let close = if c == '[' { ']' } else { c };
                let start = i;
                i += 1;
                while i < chars.len() && chars[i] != close {
                    i += 1;
                }
                i += 1;
                let quoted: String = chars[start..i.min(chars.len())].iter().collect();
                Pred::Column(identifier(Some(&quoted)).ok_or_else(invalid)?)
            }
            '=' | '!' | '<' | '>' => {
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                let (op, len) = match two.as_str() {
                    "==" => ("=", 2),
                    "!=" | "<>" => ("<>", 2),
                    "<=" => ("<=", 2),
                    ">=" => (">=", 2),
                    _ => match c {
                        '=' => ("=", 1),
                        '<' => ("<", 1),
                        '>' => (">", 1),
                        _ => return Err(invalid()),
                    },
                };
                i += len;
                Pred::Op(op)
            }
            _ => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '_' | '.' | '-'))
                {
                    i += 1;
                }
                if start == i {
                    return Err(invalid());
                }
                let word: String = chars[start..i].iter().collect();
                let upper = word.to_ascii_uppercase();
                match upper.as_str() {
                    "AND" => Pred::Word("AND"),
                    "OR" => Pred::Word("OR"),
                    "NOT" => Pred::Word("NOT"),
                    "IS" => Pred::Word("IS"),
                    "NULL" | "TRUE" | "FALSE" => Pred::Literal(upper),
                    _ if word.parse::<f64>().is_ok_and(f64::is_finite) => Pred::Literal(word),
                    _ => Pred::Column(identifier(Some(&word)).ok_or_else(invalid)?),
                }
            }
        };
        out.push(token);
        if out.len() > MAX_PREDICATE_TOKENS {
            return Err("the WHERE condition is too long".into());
        }
    }
    check_predicate(&out).then_some(out).ok_or_else(invalid)
}

/// Whether the tokens form `cond ((AND|OR) cond)*`, where cond is `NOT cond`, `( ... )`,
/// `operand op operand` or `operand IS [NOT] NULL`.
fn check_predicate(tokens: &[Pred]) -> bool {
    fn operand(t: Option<&Pred>) -> bool {
        matches!(t, Some(Pred::Column(_) | Pred::Text(_) | Pred::Literal(_)))
    }
    fn cond(t: &[Pred], pos: &mut usize, depth: usize) -> bool {
        if depth > 8 {
            return false;
        }
        match t.get(*pos) {
            Some(Pred::Word("NOT")) => {
                *pos += 1;
                cond(t, pos, depth + 1)
            }
            Some(Pred::Word("(")) => {
                *pos += 1;
                if !expr(t, pos, depth + 1) || t.get(*pos) != Some(&Pred::Word(")")) {
                    return false;
                }
                *pos += 1;
                true
            }
            _ => {
                if !operand(t.get(*pos)) {
                    return false;
                }
                *pos += 1;
                match t.get(*pos) {
                    Some(Pred::Op(_)) => {
                        *pos += 1;
                        let ok = operand(t.get(*pos));
                        *pos += 1;
                        ok
                    }
                    Some(Pred::Word("IS")) => {
                        *pos += 1;
                        if t.get(*pos) == Some(&Pred::Word("NOT")) {
                            *pos += 1;
                        }
                        let ok = t.get(*pos) == Some(&Pred::Literal("NULL".into()));
                        *pos += 1;
                        ok
                    }
                    _ => false,
                }
            }
        }
    }
    fn expr(t: &[Pred], pos: &mut usize, depth: usize) -> bool {
        if !cond(t, pos, depth) {
            return false;
        }
        while matches!(t.get(*pos), Some(Pred::Word("AND" | "OR"))) {
            *pos += 1;
            if !cond(t, pos, depth) {
                return false;
            }
        }
        true
    }
    let mut pos = 0;
    !tokens.is_empty() && expr(tokens, &mut pos, 0) && pos == tokens.len()
}

/// An identifier token, unquoted from `"x"`, `` `x` `` or `[x]`. Only `[A-Za-z0-9_]` allowed.
fn identifier(token: Option<&str>) -> Option<String> {
    let token = token?;
    let inner = ["\"", "`", "["]
        .iter()
        .zip(["\"", "`", "]"])
        .find_map(|(open, close)| token.strip_prefix(open).and_then(|t| t.strip_suffix(close)))
        .unwrap_or(token);
    let valid = !inner.is_empty()
        && inner.len() <= 63
        && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then(|| inner.to_string())
}

fn tokenize(raw: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '(' | ')' | ',' | ';' => {
                tokens.push(c.to_string());
                i += 1;
            }
            '"' | '`' | '[' => {
                let close = if c == '[' { ']' } else { c };
                let start = i;
                i += 1;
                while i < chars.len() && chars[i] != close {
                    i += 1;
                }
                if i >= chars.len() {
                    return Err("unterminated quoted identifier".into());
                }
                i += 1;
                tokens.push(chars[start..i].iter().collect());
            }
            _ => {
                let start = i;
                while i < chars.len()
                    && !chars[i].is_whitespace()
                    && !matches!(chars[i], '(' | ')' | ',' | ';' | '"' | '`' | '[')
                {
                    i += 1;
                }
                tokens.push(chars[start..i].iter().collect());
            }
        }
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pocketbase_indexes() {
        let idx = Index::parse("CREATE UNIQUE INDEX `idx_title` ON `posts` (`title`)").unwrap();
        assert_eq!(
            idx,
            Index {
                name: "idx_title".into(),
                unique: true,
                columns: vec![("title".into(), false)],
                predicate: None,
            }
        );

        let idx = Index::parse(
            "create index if not exists \"idx_multi\" on posts (status, \"created\" DESC);",
        )
        .unwrap();
        assert!(!idx.unique);
        assert_eq!(
            idx.columns,
            vec![("status".into(), false), ("created".into(), true)]
        );
        assert_eq!(
            Index::table_of("CREATE INDEX x ON [posts] (a)").as_deref(),
            Some("posts")
        );

        assert_eq!(
            idx.to_pocketbase_sql("posts"),
            "CREATE INDEX `idx_multi` ON `posts` (`status`, `created` DESC)"
        );
        assert_eq!(
            idx.to_postgres_sql("slonbase.\"posts\""),
            "CREATE INDEX \"idx_multi\" ON slonbase.\"posts\" (\"status\", \"created\" DESC)"
        );
    }

    #[test]
    fn rejects_unsupported_or_unsafe_indexes() {
        for raw in [
            "",
            "DROP TABLE posts",
            "CREATE INDEX idx ON posts",
            "CREATE INDEX idx ON posts ()",
            "CREATE INDEX idx ON posts (lower(title))",
            "CREATE INDEX idx ON posts (title COLLATE NOCASE)",
            "CREATE INDEX idx ON posts (title) WHERE lower(title) != ''",
            "CREATE INDEX idx ON posts (title) WHERE title != ''; DROP TABLE posts",
            "CREATE INDEX idx ON posts (title) WHERE title IN (SELECT 1)",
            "CREATE INDEX idx ON posts (title) WHERE title",
            "CREATE INDEX idx ON posts (title) WHERE title = 'x",
            "CREATE INDEX idx ON posts (title) WHERE (title = 'x'",
            "CREATE INDEX idx ON posts (title) WHERE",
            "CREATE INDEX idx ON posts (title); DROP TABLE posts",
            "CREATE INDEX \"bad name\" ON posts (title)",
            "CREATE INDEX idx ON posts (\"a\"\"b\")",
            "CREATE INDEX `unterminated ON posts (title)",
        ] {
            assert!(Index::parse(raw).is_err(), "{raw:?} should be rejected");
        }
    }

    #[test]
    fn partial_indexes() {
        // what PocketBase makes for auth collections
        let idx = Index::parse(
            "CREATE UNIQUE INDEX `idx_email_pbc_1` ON `users` (`email`) WHERE `email` != ''",
        )
        .unwrap();
        assert!(idx.unique);
        assert_eq!(
            idx.to_postgres_sql("slonbase.\"users\""),
            "CREATE UNIQUE INDEX \"idx_email_pbc_1\" ON slonbase.\"users\" (\"email\") WHERE \"email\" <> ''"
        );
        assert_eq!(
            idx.to_pocketbase_sql("users"),
            "CREATE UNIQUE INDEX `idx_email_pbc_1` ON `users` (`email`) WHERE `email` <> ''"
        );
        // round trip
        assert_eq!(Index::parse(&idx.to_pocketbase_sql("users")).unwrap(), idx);

        let idx = Index::parse(
            "CREATE INDEX idx ON posts (title) WHERE (published = TRUE OR views >= 10.5) AND NOT \"deleted\" IS NOT NULL AND note = 'it''s; fine'",
        )
        .unwrap();
        assert_eq!(
            idx.to_postgres_sql("p"),
            "CREATE INDEX \"idx\" ON p (\"title\") WHERE ( \"published\" = TRUE OR \"views\" >= 10.5 ) AND NOT \"deleted\" IS NOT NULL AND \"note\" = 'it''s; fine'"
        );
    }
}
