//! Small SQL lexer for highlighting, single-statement validation, and checked
//! formatting, not SQL parsing.
use serde::{Deserialize, Serialize};
use sqlformat::{Dialect, FormatOptions, Indent, QueryParams};
use std::ops::Range;

/// The letter case of SQL keywords in formatted SQL.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeywordCase {
    #[default]
    Uppercase,
    Lowercase,
}

/// How Qrow lays out SQL that it formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct SqlStyle {
    pub keyword_case: KeywordCase,
    /// Spaces for each indent level.
    pub indent_spaces: u8,
}

impl Default for SqlStyle {
    fn default() -> Self {
        Self {
            keyword_case: KeywordCase::default(),
            indent_spaces: 2,
        }
    }
}

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

/// Formats a query statement that is longer than 80 characters with single
/// spaces, or returns `None` to keep it as written. The result changes only
/// whitespace where Spark SQL ignores it and the letter case of keywords.
pub fn format_statement(sql: &str, style: SqlStyle) -> Option<String> {
    const MIN_CHARS: usize = 80;
    const FORMATTABLE: [&str; 10] = [
        "SELECT", "WITH", "FROM", "VALUES", "INSERT", "CREATE", "EXPLAIN", "MERGE", "UPDATE",
        "DELETE",
    ];
    let tokens = significant(sql);
    let chars: usize = tokens
        .iter()
        .map(|t| t.text.chars().count() + usize::from(t.space_before))
        .sum();
    if chars <= MIN_CHARS {
        return None;
    }
    let first = tokens.into_iter().find(|t| t.kind != Kind::Comment)?;
    if !FORMATTABLE.contains(&first.text.to_ascii_uppercase().as_str()) {
        return None;
    }
    let options = FormatOptions {
        indent: Indent::Spaces(style.indent_spaces),
        // sqlformat changes only reserved keywords, not identifiers such as
        // `t.Date` or a column named `user`.
        uppercase: Some(style.keyword_case == KeywordCase::Uppercase),
        // The PostgreSQL dialect keeps subscripts such as `items[0]` together.
        dialect: Dialect::PostgreSql,
        max_inline_top_level: Some(60),
        // Without this limit, sqlformat 0.5 puts each item of an inline clause,
        // such as `SELECT a, b`, on a new line without indent.
        max_inline_arguments: Some(60),
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
/// never join with other symbols. Words can change letter case, except next to
/// `.`, where they name a field, a column, or a table.
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
                let qualified = [i.checked_sub(1), Some(i + 1)]
                    .into_iter()
                    .flatten()
                    .any(|j| original.get(j).is_some_and(|t| t.text == "."));
                a.kind == b.kind
                    && (a.text == b.text
                        || is_word(a) && !qualified && a.text.eq_ignore_ascii_case(b.text))
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
            format_statement(sql, SqlStyle::default()).unwrap(),
            "SELECT COUNT(*) AS paid_bookings\nFROM integrations.bookings\nWHERE\n  state = 'paid'\n  AND CAST(booked_at AS DATE) = DATE '2026-09-26'"
        );
        let sql = "SELECT items[0], `order  id`, 'a  b', 'it\\'s', x::int, a >= b /*+ keep  this */ FROM t WHERE ok -- note";
        let formatted = format_statement(sql, SqlStyle::default()).unwrap();
        assert!(formatted.contains("items[0]") && formatted.ends_with("WHERE ok -- note"));
        assert!(formatted.contains("`order  id`") && formatted.contains("'a  b'"));
        assert!(formatted.contains("'it\\'s'") && formatted.contains("/*+ keep  this */"));
    }

    #[test]
    fn inline_clauses_keep_their_items_on_one_line() {
        let sql = "SELECT gate_name, gate_id, COUNT(*) AS paid_bookings FROM integrations.bookings WHERE state = 'paid' AND CAST(booked_at AS DATE) >= DATE '2026-09-20' AND CAST(booked_at AS DATE) < DATE '2026-09-27' AND (gate_name IS NOT NULL OR gate_id IS NOT NULL) GROUP BY gate_name, gate_id ORDER BY paid_bookings DESC LIMIT 10;";
        assert_eq!(
            format_statement(sql, SqlStyle::default()).unwrap(),
            "\
SELECT gate_name, gate_id, COUNT(*) AS paid_bookings
FROM integrations.bookings
WHERE
  state = 'paid'
  AND CAST(booked_at AS DATE) >= DATE '2026-09-20'
  AND CAST(booked_at AS DATE) < DATE '2026-09-27'
  AND (gate_name IS NOT NULL OR gate_id IS NOT NULL)
GROUP BY gate_name, gate_id
ORDER BY paid_bookings DESC
LIMIT 10;"
        );
    }

    #[test]
    fn formatting_uses_keyword_case_and_indent_from_style() {
        let sql = "select t.Select, count(*) as n from db.Events t where t.state = 'paid' and t.Date is not null and t.market in ('de', 'fr') group by t.Select";
        let upper = SqlStyle {
            keyword_case: KeywordCase::Uppercase,
            indent_spaces: 4,
        };
        assert_eq!(
            format_statement(sql, upper).unwrap(),
            "\
SELECT t.Select, count(*) AS n
FROM db.Events t
WHERE
    t.state = 'paid'
    AND t.Date IS NOT NULL
    AND t.market IN ('de', 'fr')
GROUP BY t.Select"
        );
        let lower = SqlStyle {
            keyword_case: KeywordCase::Lowercase,
            indent_spaces: 2,
        };
        let sql = "SELECT T.SELECT, COUNT(*) AS N FROM DB.EVENTS T WHERE T.STATE = 'PAID' AND T.DATE IS NOT NULL AND T.MARKET IN ('DE', 'FR') GROUP BY T.SELECT";
        assert_eq!(
            format_statement(sql, lower).unwrap(),
            "\
select T.SELECT, COUNT(*) as N
from DB.EVENTS T
where
  T.STATE = 'PAID'
  and T.DATE is not null
  and T.MARKET in ('DE', 'FR')
group by T.SELECT"
        );
    }

    #[test]
    fn formats_statements_written_on_several_lines() {
        let sql = "SELECT gate_id, COUNT(*) AS clicks\nFROM avia.clicks -- all gates\nWHERE CAST(created_at AS DATE) >= DATE '2026-09-26'\n  AND CAST(created_at AS DATE) < DATE '2026-09-28'\n  AND gate_id IS NOT NULL\nGROUP BY gate_id\nORDER BY clicks DESC\nLIMIT 10;";
        assert_eq!(
            format_statement(sql, SqlStyle::default()).unwrap(),
            "\
SELECT gate_id, COUNT(*) AS clicks
FROM avia.clicks -- all gates
WHERE
  CAST(created_at AS DATE) >= DATE '2026-09-26'
  AND CAST(created_at AS DATE) < DATE '2026-09-28'
  AND gate_id IS NOT NULL
GROUP BY gate_id
ORDER BY clicks DESC
LIMIT 10;"
        );
        // A short statement keeps the layout that the assistant wrote.
        assert_eq!(
            format_statement("SELECT\n  a\nFROM\n  t", SqlStyle::default()),
            None
        );
        // A line comment between conditions must not hide the next condition.
        let sql = "SELECT a FROM integrations.bookings WHERE state = 'paid' -- only paid\nAND booked_at >= DATE '2026-09-20' AND gate_id IS NOT NULL";
        let formatted = format_statement(sql, SqlStyle::default()).unwrap();
        for line in formatted.lines().filter(|line| line.contains("--")) {
            assert!(line.trim_end().ends_with("-- only paid"), "{formatted}");
        }
        assert!(formatted.contains("AND booked_at >="), "{formatted}");
    }

    #[test]
    fn keeps_short_raw_and_unsafe_statements() {
        let padding = "a, ".repeat(30);
        for sql in [
            "SELECT 1".to_owned(),
            format!("SET spark.sql.shuffle.partitions=10 -- {padding}"),
            format!("ADD JAR /opt/jars/udf-1.0.jar -- {padding}"),
            // Spark substitutes `${var}` before parsing.
            format!("SELECT {padding}b FROM t WHERE dt = ${{hivevar:dt}}"),
        ] {
            assert_eq!(format_statement(&sql, SqlStyle::default()), None, "{sql}");
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
        assert!(same_tokens("select x from t", "SELECT X FROM T"));
        for (original, formatted) in [
            ("SELECT a >= b", "SELECT a > = b"),
            ("SELECT x=-1", "SELECT x = -1"),
            ("SELECT ${x}", "SELECT $ {x}"),
            ("SELECT X'1F'", "SELECT X '1F'"),
            ("SELECT 'a  b'", "SELECT 'a b'"),
            ("SELECT a b", "SELECT ab"),
            ("SELECT 'a'", "SELECT 'A'"),
            ("SELECT t.select", "SELECT t.SELECT"),
            ("SELECT select.t", "SELECT SELECT.t"),
        ] {
            assert!(!same_tokens(original, formatted), "{formatted}");
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
