//! Small SQL lexer for statement ranges, single-statement validation, and
//! checked formatting, not SQL parsing.
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
    /// A keyword or a name without quotes, a run of ASCII whitespace, or
    /// one byte of another symbol. `>=` is two tokens, so that formatting
    /// can find symbols that join.
    Plain,
    String,
    Comment,
    Number,
    Identifier,
    Separator,
}

/// Splits `sql` into tokens that cover all of it, in order.
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
            Kind::Plain
        } else if bytes[i].is_ascii_whitespace() {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            Kind::Plain
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

/// Finds the statement at a UTF-8 byte offset. Same-line whitespace beside
/// a statement belongs to it; blank lines and comments between statements do not.
pub fn statement_range_at(sql: &str, offset: usize) -> Option<Range<usize>> {
    if !sql.is_char_boundary(offset) {
        return None;
    }
    let ranges = statement_ranges(sql);
    // Prefer the next statement's start over the previous statement's end.
    if let Some(range) = ranges.iter().find(|range| range.contains(&offset)) {
        return Some(range.clone());
    }
    if tokens(sql)
        .iter()
        .any(|(range, kind)| *kind == Kind::Comment && range.contains(&offset))
    {
        return None;
    }
    ranges.into_iter().find(|range| {
        let gap = if offset < range.start {
            offset..range.start
        } else {
            range.end..offset
        };
        sql[gap]
            .chars()
            .all(|c| c.is_whitespace() && !matches!(c, '\n' | '\r'))
    })
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
    let formatted = case_other_keywords(formatted.trim(), style.keyword_case);
    (formatted != sql && same_tokens(sql, &formatted)).then_some(formatted)
}

/// Spark built-in functions, and `OVER`, that Qrow writes in the keyword case
/// before `(`. Spark finds built-in functions without regard to case. The list
/// keeps a table or a CTE name before `(` as written.
const FUNCTIONS: &[&str] = &[
    "abs",
    "acos",
    "add_months",
    "aggregate",
    "any",
    "any_value",
    "approx_count_distinct",
    "approx_percentile",
    "array",
    "array_contains",
    "array_distinct",
    "array_except",
    "array_join",
    "array_max",
    "array_min",
    "array_position",
    "array_remove",
    "array_sort",
    "array_union",
    "arrays_zip",
    "ascii",
    "asin",
    "atan",
    "avg",
    "base64",
    "bit_and",
    "bit_or",
    "bool_and",
    "bool_or",
    "bround",
    "cardinality",
    "cast",
    "cbrt",
    "ceil",
    "ceiling",
    "char_length",
    "coalesce",
    "collect_list",
    "collect_set",
    "concat",
    "concat_ws",
    "contains",
    "corr",
    "cos",
    "count",
    "count_if",
    "covar_pop",
    "covar_samp",
    "cume_dist",
    "current_date",
    "current_timestamp",
    "date",
    "date_add",
    "date_diff",
    "date_format",
    "date_part",
    "date_sub",
    "date_trunc",
    "dateadd",
    "datediff",
    "day",
    "dayofmonth",
    "dayofweek",
    "dayofyear",
    "decode",
    "degrees",
    "dense_rank",
    "element_at",
    "endswith",
    "every",
    "exists",
    "exp",
    "explode",
    "explode_outer",
    "extract",
    "filter",
    "first",
    "first_value",
    "flatten",
    "floor",
    "forall",
    "format_number",
    "format_string",
    "from_json",
    "from_unixtime",
    "from_utc_timestamp",
    "get_json_object",
    "greatest",
    "hash",
    "hex",
    "hour",
    "if",
    "ifnull",
    "initcap",
    "inline",
    "instr",
    "isnan",
    "isnotnull",
    "isnull",
    "json_tuple",
    "lag",
    "last",
    "last_day",
    "last_value",
    "lead",
    "least",
    "left",
    "len",
    "length",
    "ln",
    "locate",
    "log",
    "log10",
    "log2",
    "lower",
    "lpad",
    "ltrim",
    "make_date",
    "make_timestamp",
    "map",
    "map_from_arrays",
    "map_keys",
    "map_values",
    "max",
    "max_by",
    "md5",
    "mean",
    "median",
    "min",
    "min_by",
    "minute",
    "mod",
    "month",
    "months_between",
    "named_struct",
    "next_day",
    "now",
    "nth_value",
    "ntile",
    "nullif",
    "nvl",
    "nvl2",
    "over",
    "percent_rank",
    "percentile",
    "percentile_approx",
    "posexplode",
    "position",
    "pow",
    "power",
    "quarter",
    "radians",
    "rand",
    "rank",
    "regexp_extract",
    "regexp_extract_all",
    "regexp_like",
    "regexp_replace",
    "repeat",
    "replace",
    "reverse",
    "right",
    "round",
    "row_number",
    "rpad",
    "rtrim",
    "second",
    "sequence",
    "sha1",
    "sha2",
    "shuffle",
    "sign",
    "sin",
    "size",
    "slice",
    "sort_array",
    "split",
    "split_part",
    "sqrt",
    "stack",
    "startswith",
    "std",
    "stddev",
    "stddev_pop",
    "stddev_samp",
    "struct",
    "substr",
    "substring",
    "substring_index",
    "sum",
    "tan",
    "timestamp",
    "to_date",
    "to_json",
    "to_timestamp",
    "to_unix_timestamp",
    "to_utc_timestamp",
    "transform",
    "translate",
    "trim",
    "trunc",
    "try_add",
    "try_avg",
    "try_cast",
    "try_divide",
    "try_element_at",
    "try_sum",
    "try_to_number",
    "ucase",
    "unbase64",
    "unhex",
    "unix_timestamp",
    "upper",
    "uuid",
    "var_pop",
    "var_samp",
    "variance",
    "weekday",
    "weekofyear",
    "xxhash64",
    "year",
    "zip_with",
];

/// Type names that Qrow writes in the keyword case in `CAST(... AS type)`.
const TYPES: &[&str] = &[
    "array",
    "bigint",
    "binary",
    "boolean",
    "byte",
    "char",
    "date",
    "dec",
    "decimal",
    "double",
    "float",
    "int",
    "integer",
    "interval",
    "long",
    "map",
    "numeric",
    "real",
    "short",
    "smallint",
    "string",
    "struct",
    "timestamp",
    "timestamp_ltz",
    "timestamp_ntz",
    "tinyint",
    "varchar",
];

/// Prefixes of typed literals, such as `DATE '2026-09-26'`.
const LITERAL_PREFIXES: &[&str] = &[
    "date",
    "interval",
    "timestamp",
    "timestamp_ltz",
    "timestamp_ntz",
];

/// Writes in `case` the keywords that sqlformat keeps as written: built-in
/// function names, cast types, typed literal prefixes, and `NULLS FIRST` or
/// `NULLS LAST`. Words next to `.` keep their case.
fn case_other_keywords(sql: &str, case: KeywordCase) -> String {
    let words: Vec<_> = tokens(sql)
        .into_iter()
        .filter(|(range, _)| !sql[range.clone()].trim().is_empty())
        .collect();
    let text = |i: usize| words.get(i).map_or("", |(range, _)| &sql[range.clone()]);
    let is_word = |i: usize| {
        words.get(i).is_some_and(|(_, kind)| *kind == Kind::Plain)
            && text(i).starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
    };
    let mut result = sql.to_owned();
    // For each open parenthesis: whether it belongs to a cast, and whether
    // the cast has reached its type.
    let mut parens: Vec<(bool, bool)> = vec![];
    for i in 0..words.len() {
        let word = text(i).to_ascii_lowercase();
        match word.as_str() {
            "(" => {
                let cast = i > 0
                    && matches!(
                        text(i - 1).to_ascii_lowercase().as_str(),
                        "cast" | "try_cast"
                    );
                parens.push((cast, false));
                continue;
            }
            ")" => {
                parens.pop();
                continue;
            }
            _ => {}
        }
        if !is_word(i) || text(i + 1) == "." || i > 0 && text(i - 1) == "." {
            continue;
        }
        let in_cast_type = parens.last().is_some_and(|&(cast, typed)| cast && typed);
        let keyword = text(i + 1) == "(" && FUNCTIONS.contains(&word.as_str())
            || words
                .get(i + 1)
                .is_some_and(|(_, kind)| *kind == Kind::String)
                && LITERAL_PREFIXES.contains(&word.as_str())
            || in_cast_type && TYPES.contains(&word.as_str())
            || word == "nulls"
                && matches!(text(i + 1).to_ascii_lowercase().as_str(), "first" | "last")
            || matches!(word.as_str(), "first" | "last")
                && text(i.wrapping_sub(1)).eq_ignore_ascii_case("nulls");
        if word == "as"
            && let Some((true, typed)) = parens.last_mut()
        {
            *typed = true;
        }
        if keyword {
            let range = words[i].0.clone();
            let cased = match case {
                KeywordCase::Uppercase => word.to_ascii_uppercase(),
                KeywordCase::Lowercase => word,
            };
            result.replace_range(range, &cased);
        }
    }
    result
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
    let is_word = |t: &Significant| t.kind == Kind::Plain && starts_word(t);
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
SELECT t.Select, COUNT(*) AS n
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
select T.SELECT, count(*) as N
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
    fn functions_types_and_literals_follow_keyword_case() {
        let lower = SqlStyle {
            keyword_case: KeywordCase::Lowercase,
            indent_spaces: 2,
        };
        let sql = "select COUNT(distinct search_id) as searches from avia.searches where pdate >= DATE '2026-09-26' and pdate < DATE '2026-09-28';";
        assert_eq!(
            format_statement(sql, lower).unwrap(),
            "\
select count(distinct search_id) as searches
from avia.searches
where pdate >= date '2026-09-26' and pdate < date '2026-09-28';"
        );
        let sql = "select max(x), CAST(y AS Decimal(10, 2)), row_number() over (order by b nulls last) as rn, interval '1' day from t order by 1 nulls first";
        let formatted = format_statement(sql, SqlStyle::default()).unwrap();
        for expected in [
            "MAX(x)",
            "CAST(y AS DECIMAL(10, 2))",
            "ROW_NUMBER() OVER",
            "NULLS LAST",
            "INTERVAL '1' DAY",
            "NULLS FIRST",
        ] {
            assert!(formatted.contains(expected), "{expected} in {formatted}");
        }
    }

    #[test]
    fn names_keep_their_case() {
        // A qualified name, a table before `(`, an alias named like a type,
        // and a column named like a function without `(`.
        let sql = "INSERT INTO bookings(Count, Date) SELECT t.Count(x), (SELECT d AS Date) AS Date, Sum FROM db.Max t WHERE t.Date = 1 AND flag";
        let formatted = format_statement(sql, SqlStyle::default()).unwrap();
        for expected in [
            "bookings(Count, Date)",
            "t.Count(x)",
            "d AS Date",
            "AS Date,",
            "Sum",
            "db.Max t",
            "t.Date = 1",
        ] {
            assert!(formatted.contains(expected), "{expected} in {formatted}");
        }
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
    fn tokens_merge_whitespace_runs_and_keep_one_byte_symbols() {
        let sql = "SELECT  \n a>=1;";
        let texts: Vec<_> = tokens(sql)
            .into_iter()
            .map(|(range, kind)| (&sql[range], kind))
            .collect();
        assert_eq!(
            texts,
            [
                ("SELECT", Kind::Plain),
                ("  \n ", Kind::Plain),
                ("a", Kind::Plain),
                (">", Kind::Plain),
                ("=", Kind::Plain),
                ("1", Kind::Number),
                (";", Kind::Separator),
            ]
        );
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

    #[test]
    fn cursor_statement_covers_multiline_sql_and_unicode() {
        let sql = "SELECT 1;\n\n  SELECT '日本😀;' AS `a;b`\n  /* inner ; */\n  FROM t\n  LIMIT 10;  \n\nSELECT 3";
        let expected = "SELECT '日本😀;' AS `a;b`\n  /* inner ; */\n  FROM t\n  LIMIT 10;";
        for marker in [
            "  SELECT", "日本", "😀", "a;b", "/* inner", "  FROM", "LIMIT 10",
        ] {
            let offset = sql.find(marker).unwrap();
            assert_eq!(&sql[statement_range_at(sql, offset).unwrap()], expected);
        }
        let end = sql.find("10;").unwrap() + 3;
        for offset in end..=end + 2 {
            assert_eq!(&sql[statement_range_at(sql, offset).unwrap()], expected);
        }
        assert_eq!(&sql[statement_range_at(sql, 0).unwrap()], "SELECT 1;");
        assert_eq!(
            &sql[statement_range_at(sql, sql.len()).unwrap()],
            "SELECT 3"
        );
        assert_eq!(statement_range_at(sql, sql.find('😀').unwrap() + 1), None);
        assert_eq!(statement_range_at(sql, usize::MAX), None);
    }

    #[test]
    fn cursor_statement_rejects_gaps_and_standalone_comments() {
        let sql = "\n-- leading;\nSELECT 1;\n\n-- between;\n/* nested /* ; */ comment */\n\nSELECT 2;\n\n-- trailing;\n";
        for marker in [
            "-- leading",
            "-- between",
            "/* nested",
            "comment */",
            "-- trailing",
        ] {
            let offset = sql.find(marker).unwrap();
            assert_eq!(statement_range_at(sql, offset), None, "{marker}");
        }
        for offset in [0, sql.find("\n\n").unwrap() + 1, sql.len()] {
            assert_eq!(statement_range_at(sql, offset), None);
        }
        for sql in ["", " \n ", "-- comment", "/* comment */", "; ;"] {
            assert_eq!(statement_range_at(sql, sql.len()), None);
        }
    }

    #[test]
    fn cursor_statement_distinguishes_adjacent_statements_and_comments() {
        let sql = "SELECT 1;SELECT 2;-- between\nSELECT 3;/* between */SELECT 4";
        for marker in ["SELECT 2", "SELECT 3", "SELECT 4"] {
            let offset = sql.find(marker).unwrap();
            let range = statement_range_at(sql, offset).unwrap();
            assert!(sql[range].starts_with(marker));
        }
        for marker in ["-- between", "/* between */"] {
            assert_eq!(statement_range_at(sql, sql.find(marker).unwrap()), None);
        }
        let sql = "SELECT 1;";
        assert_eq!(statement_range_at(sql, sql.len()), Some(0..sql.len()));
    }
}
