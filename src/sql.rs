//! Small SQL lexer for highlighting, single-statement validation, and checked
//! formatting, not SQL parsing.
use sqlformat::{Dialect, FormatOptions, Indent, QueryParams};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    Identifier,
    Separator,
}

pub fn tokens(sql: &str) -> Vec<(Range<usize>, Kind)> {
    let bytes = sql.as_bytes();
    let mut tokens = vec![];
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let kind = if bytes[i..].starts_with(b"--") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            Kind::Comment
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            Kind::Comment
        } else if matches!(bytes[i], b'\'' | b'"' | b'`') {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && quote != b'`' {
                    i = (i + 2).min(bytes.len());
                } else if bytes[i] == quote {
                    i += 1;
                    if i < bytes.len() && bytes[i] == quote {
                        i += 1;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if quote == b'`' {
                Kind::Identifier
            } else {
                Kind::String
            }
        } else if bytes[i].is_ascii_digit() {
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_')
            {
                i += 1;
            }
            Kind::Number
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' || bytes[i] >= 128 {
            i += sql[i..].chars().next().unwrap().len_utf8();
            while i < bytes.len() {
                let c = sql[i..].chars().next().unwrap();
                if !c.is_alphanumeric() && c != '_' {
                    break;
                }
                i += c.len_utf8();
            }
            let word = sql[start..i].to_ascii_uppercase();
            if "SELECT FROM WHERE GROUP BY ORDER HAVING LIMIT OFFSET AS WITH UNION ALL DISTINCT JOIN LEFT RIGHT FULL OUTER INNER CROSS ON AND OR NOT NULL IS IN EXISTS BETWEEN LIKE RLIKE CASE WHEN THEN ELSE END ASC DESC TRUE FALSE USE SET SHOW DESCRIBE EXPLAIN CREATE ALTER DROP INSERT INTO OVERWRITE TABLE VIEW DATABASE IF CAST TRY_CAST OVER PARTITION ROWS RANGE CURRENT ROW UNBOUNDED PRECEDING FOLLOWING WINDOW LATERAL VALUES DATE TIMESTAMP INTERVAL FETCH FIRST ONLY SEMI ANTI REPLACE".split_whitespace().any(|s| s == word) {
                Kind::Keyword
            } else { Kind::Plain }
        } else {
            i += 1;
            if bytes[start] == b';' {
                Kind::Separator
            } else {
                Kind::Plain
            }
        };
        tokens.push((start..i, kind));
    }
    tokens
}

/// Byte ranges of the statements in `sql`, each with its separator. Comments
/// between statements stay outside the ranges.
pub fn statement_ranges(sql: &str) -> Vec<Range<usize>> {
    let mut start = None;
    let mut end = 0;
    let mut ranges = vec![];
    for (range, kind) in tokens(sql) {
        if kind == Kind::Comment || sql[range.clone()].trim().is_empty() {
            continue;
        }
        if kind == Kind::Separator {
            if let Some(start) = start.take() {
                ranges.push(start..range.end);
            }
        } else {
            start.get_or_insert(range.start);
            end = range.end;
        }
    }
    if let Some(start) = start {
        ranges.push(start..end);
    }
    ranges
}

pub fn last_statement_range(sql: &str) -> Option<Range<usize>> {
    statement_ranges(sql).pop()
}

pub fn validate_single(sql: &str) -> anyhow::Result<()> {
    let mut statements = 0;
    let mut content = false;
    for (range, kind) in tokens(sql) {
        if kind == Kind::Separator {
            if content {
                statements += 1;
                content = false;
            }
        } else if kind != Kind::Comment && !sql[range].trim().is_empty() {
            content = true;
        }
    }
    statements += usize::from(content);
    anyhow::ensure!(statements > 0, "Write or select a SQL statement first.");
    anyhow::ensure!(
        statements == 1,
        "Run one statement at a time. Select the statement you want to execute."
    );
    Ok(())
}

/// Formats a long one-line query statement, or returns `None` to keep it as
/// written. The result changes only whitespace, and only where Spark SQL
/// ignores it.
pub fn format_long_line(sql: &str) -> Option<String> {
    const MIN_CHARS: usize = 80;
    const FORMATTABLE: [&str; 10] = [
        "SELECT", "WITH", "FROM", "VALUES", "INSERT", "CREATE", "EXPLAIN", "MERGE", "UPDATE",
        "DELETE",
    ];
    if sql.contains('\n') || sql.chars().count() <= MIN_CHARS {
        return None;
    }
    let first = significant(sql)
        .into_iter()
        .find(|t| t.kind != Kind::Comment)?;
    if !FORMATTABLE.contains(&first.text.to_ascii_uppercase().as_str()) {
        return None;
    }
    let options = FormatOptions {
        indent: Indent::Spaces(2),
        // The PostgreSQL dialect keeps subscripts such as `items[0]` together.
        dialect: Dialect::PostgreSql,
        max_inline_top_level: Some(60),
        ..FormatOptions::default()
    };
    let formatted = sqlformat::format(sql, &QueryParams::None, &options);
    let formatted = formatted.trim();
    (formatted != sql && same_tokens(sql, formatted)).then(|| formatted.to_owned())
}

struct Significant<'a> {
    kind: Kind,
    text: &'a str,
    space_before: bool,
}

fn significant(sql: &str) -> Vec<Significant<'_>> {
    let mut result = vec![];
    let mut space_before = false;
    for (range, kind) in tokens(sql) {
        let text = &sql[range];
        if text.trim().is_empty() {
            space_before = true;
        } else {
            result.push(Significant {
                kind,
                text,
                space_before,
            });
            space_before = false;
        }
    }
    result
}

/// Whether `formatted` has the tokens of `original` and changes whitespace only
/// between tokens that stay separate tokens in Spark SQL. For example, `>=`,
/// `${var}`, and `X'1F'` must stay together. Delimiters such as `,` and `(`
/// never join with other symbols.
fn same_tokens(original: &str, formatted: &str) -> bool {
    let (original, formatted) = (significant(original), significant(formatted));
    let starts_word = |t: &Significant| {
        t.text
            .starts_with(|c: char| c.is_alphanumeric() || c == '_')
    };
    let is_symbol = |t: &Significant| {
        t.kind == Kind::Plain && !starts_word(t) && !t.text.starts_with(|c| ",()[]".contains(c))
    };
    let is_word = |t: &Significant| matches!(t.kind, Kind::Plain | Kind::Keyword) && starts_word(t);
    original.len() == formatted.len()
        && original
            .iter()
            .zip(&formatted)
            .enumerate()
            .all(|(i, (a, b))| {
                a.kind == b.kind
                    && a.text == b.text
                    && (i == 0
                        || a.space_before == b.space_before
                        || !(is_symbol(&original[i - 1]) && is_symbol(a)
                            || is_word(&original[i - 1]) && a.kind == Kind::String))
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn single_statement_respects_literals_comments_and_unicode() {
        for sql in [
            "select ';日本語'; -- rest",
            "/* outer /* inner ; */ */ select 1",
            "select 'it''s;fine'",
            "select `a;b`",
        ] {
            assert!(validate_single(sql).is_ok(), "{sql}");
        }
        assert!(validate_single("SET x=1; SELECT 1").is_err());
        assert!(validate_single("-- no query").is_err());
        let sql = "select '日\\本', 列 from 表";
        for (range, _) in tokens(sql) {
            let _ = &sql[range];
        }
    }

    #[test]
    fn formats_long_one_line_queries() {
        let sql = "SELECT COUNT(*) AS paid_bookings FROM integrations.bookings WHERE state = 'paid' AND CAST(booked_at AS DATE) = DATE '2026-09-26'";
        assert_eq!(
            format_long_line(sql).unwrap(),
            "SELECT COUNT(*) AS paid_bookings\nFROM integrations.bookings\nWHERE\n  state = 'paid'\n  AND CAST(booked_at AS DATE) = DATE '2026-09-26'"
        );
        let sql = "SELECT items[0], `order  id`, 'a  b', 'it\\'s', x::int, a >= b /*+ keep  this */ FROM t WHERE ok -- note";
        let formatted = format_long_line(sql).unwrap();
        assert!(formatted.contains("items[0]") && formatted.ends_with("WHERE ok -- note"));
        assert!(formatted.contains("`order  id`") && formatted.contains("'a  b'"));
        assert!(formatted.contains("'it\\'s'") && formatted.contains("/*+ keep  this */"));
    }

    #[test]
    fn keeps_short_multiline_raw_and_unsafe_statements() {
        let padding = "a, ".repeat(30);
        for sql in [
            "SELECT 1".to_owned(),
            format!("SELECT {padding}\nb FROM t"),
            format!("SET spark.sql.shuffle.partitions=10 -- {padding}"),
            format!("ADD JAR /opt/jars/udf-1.0.jar -- {padding}"),
            // Spark substitutes `${var}` before parsing.
            format!("SELECT {padding}b FROM t WHERE dt = ${{hivevar:dt}}"),
        ] {
            assert_eq!(format_long_line(&sql), None, "{sql}");
        }
    }

    #[test]
    fn token_check_rejects_joined_or_split_tokens() {
        assert!(same_tokens("SELECT a >= b", "SELECT\n  a >= b"));
        assert!(same_tokens("SELECT f(a)", "SELECT f (a)"));
        assert!(same_tokens(
            "SELECT -(1),(2) IN ('a','b')",
            "SELECT - (1), (2) IN ('a', 'b')"
        ));
        for formatted in [
            "SELECT a > = b",
            "SELECT x = -1",
            "SELECT $ {x}",
            "SELECT X '1F'",
            "SELECT 'a b'",
            "SELECT ab",
        ] {
            let original = formatted
                .replace("> =", ">=")
                .replace("x = -1", "x=-1")
                .replace("$ {", "${")
                .replace("X '", "X'")
                .replace("'a b'", "'a  b'")
                .replace("ab", "a b");
            assert!(!same_tokens(&original, formatted), "{formatted}");
        }
    }

    #[test]
    fn last_statement_skips_comments_and_preserves_utf8_offsets() {
        let sql = "SELECT '日本語'; -- earlier\n\nSELECT 2 -- latest";
        let range = last_statement_range(sql).unwrap();
        assert_eq!(&sql[range], "SELECT 2");
        assert_eq!(last_statement_range("-- only a comment"), None);
    }

    #[test]
    fn statement_ranges_cover_each_statement_with_its_separator() {
        let sql = "SELECT '日;本';\n-- note; not a query\n;SELECT 2;\n\nSELECT 3 -- open";
        let statements: Vec<_> = statement_ranges(sql)
            .into_iter()
            .map(|range| &sql[range])
            .collect();
        assert_eq!(statements, ["SELECT '日;本';", "SELECT 2;", "SELECT 3"]);
        assert!(statement_ranges(" -- only a comment\n").is_empty());
    }
}
