//! Collection indexes. PocketBase stores them as `CREATE INDEX` statements; slonbase accepts
//! the same strings but only a safe subset: plain column lists, no expressions or `WHERE`.
//! The SQL that runs is always regenerated from the parsed form with quoted identifiers.

use crate::filter::quote_ident;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    pub unique: bool,
    /// Column names and whether they're sorted descending.
    pub columns: Vec<(String, bool)>,
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
        match next(&mut pos) {
            None | Some(";") if pos >= tokens.len() => {}
            Some(w) if w.eq_ignore_ascii_case("WHERE") => {
                return Err("partial indexes (WHERE) are not supported".into());
            }
            _ => return Err("unexpected text after the column list".into()),
        }

        Ok(Self {
            name,
            unique,
            columns,
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
            "CREATE {}INDEX `{}` ON `{}` ({})",
            if self.unique { "UNIQUE " } else { "" },
            self.name,
            table,
            columns.join(", ")
        )
    }

    /// The PostgreSQL statement.
    pub fn to_postgres_sql(&self, table_ref: &str) -> String {
        let columns: Vec<String> = self
            .columns
            .iter()
            .map(|(c, desc)| format!("{}{}", quote_ident(c), if *desc { " DESC" } else { "" }))
            .collect();
        format!(
            "CREATE {}INDEX {} ON {} ({})",
            if self.unique { "UNIQUE " } else { "" },
            quote_ident(&self.name),
            table_ref,
            columns.join(", ")
        )
    }
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
            "CREATE INDEX idx ON posts (title) WHERE title != ''",
            "CREATE INDEX idx ON posts (title); DROP TABLE posts",
            "CREATE INDEX \"bad name\" ON posts (title)",
            "CREATE INDEX idx ON posts (\"a\"\"b\")",
            "CREATE INDEX `unterminated ON posts (title)",
        ] {
            assert!(Index::parse(raw).is_err(), "{raw:?} should be rejected");
        }
    }
}
