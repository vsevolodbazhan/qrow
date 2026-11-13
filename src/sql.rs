//! Small SQL lexer for statement ranges, single-statement validation, and
//! checked formatting, not SQL parsing.
use crate::model::DatabaseType;
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
    tokens_for(sql, crate::model::DatabaseType::Kyuubi)
}

pub fn tokens_for(
    sql: &str,
    database_type: crate::model::DatabaseType,
) -> Vec<(Range<usize>, Kind)> {
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
        } else if database_type != crate::model::DatabaseType::Trino
            && bytes[i] == b'$'
            && dollar_quote_end(sql, i).is_some()
        {
            let (delimiter_end, string_end) = dollar_quote_end(sql, i).unwrap();
            i = string_end.unwrap_or(bytes.len()).max(delimiter_end);
            Kind::String
        } else if matches!(bytes[i], b'\'' | b'"' | b'`')
            || (database_type != DatabaseType::Kyuubi
                && matches!(bytes[i], b'U' | b'u')
                && bytes.get(i + 1) == Some(&b'&')
                && matches!(bytes.get(i + 2), Some(b'\'' | b'"')))
        {
            if matches!(bytes[i], b'U' | b'u') {
                i += 2;
            }
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\'
                    && quote != b'`'
                    && (database_type == crate::model::DatabaseType::Kyuubi
                        || (database_type == crate::model::DatabaseType::Postgres
                            && quote == b'\''
                            && start > 0
                            && matches!(bytes[start - 1], b'E' | b'e')))
                {
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
            if quote == b'`'
                || (quote == b'"' && database_type != crate::model::DatabaseType::Kyuubi)
            {
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

/// The opening delimiter and end of a Postgres dollar-quoted string.
fn dollar_quote_end(sql: &str, start: usize) -> Option<(usize, Option<usize>)> {
    let suffix = &sql[start + 1..];
    let length = suffix.find('$')?;
    let tag = &suffix[..length];
    if !tag.is_empty()
        && (!tag.starts_with(|c: char| c.is_alphabetic() || c == '_')
            || !tag.chars().all(|c| c.is_alphanumeric() || c == '_'))
    {
        return None;
    }
    let delimiter_end = start + length + 2;
    let delimiter = &sql[start..delimiter_end];
    Some((
        delimiter_end,
        sql[delimiter_end..]
            .find(delimiter)
            .map(|offset| delimiter_end + offset + delimiter.len()),
    ))
}

/// Byte ranges of the statements in `sql`, each with its separator. Comments
/// between statements stay outside the ranges.
pub fn statement_ranges(sql: &str) -> Vec<Range<usize>> {
    statement_ranges_for(sql, crate::model::DatabaseType::Kyuubi)
}

pub fn statement_ranges_for(
    sql: &str,
    database_type: crate::model::DatabaseType,
) -> Vec<Range<usize>> {
    statement_ranges_in(sql, tokens_for(sql, database_type))
}

fn statement_ranges_in(sql: &str, tokens: Vec<(Range<usize>, Kind)>) -> Vec<Range<usize>> {
    let mut start = None;
    let mut end = 0;
    let mut ranges = vec![];
    for (range, kind) in tokens {
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
    statement_range_at_for(sql, offset, DatabaseType::Kyuubi)
}

/// Finds the cursor statement using the connection's quoting rules.
pub fn statement_range_at_for(
    sql: &str,
    offset: usize,
    database_type: DatabaseType,
) -> Option<Range<usize>> {
    StatementIndex::new_for(sql, database_type).range_at(sql, offset)
}

/// The statements and comments of one SQL text, from one pass of the tokenizer.
/// Keep it while the text does not change, to find statements at many offsets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatementIndex {
    statements: Vec<Range<usize>>,
    comments: Vec<Range<usize>>,
}

impl StatementIndex {
    pub fn new(sql: &str) -> Self {
        Self::new_for(sql, DatabaseType::Kyuubi)
    }

    /// Indexes `sql` using the connection's quoting rules.
    pub fn new_for(sql: &str, database_type: DatabaseType) -> Self {
        let tokens = tokens_for(sql, database_type);
        let comments = tokens
            .iter()
            .filter(|(_, kind)| *kind == Kind::Comment)
            .map(|(range, _)| range.clone())
            .collect();
        Self {
            statements: statement_ranges_in(sql, tokens),
            comments,
        }
    }

    pub fn statements(&self) -> &[Range<usize>] {
        &self.statements
    }

    /// The statement that a run without a selection uses, as
    /// [`statement_range_at`] describes. `sql` is the text of the index.
    pub fn range_at(&self, sql: &str, offset: usize) -> Option<Range<usize>> {
        if !sql.is_char_boundary(offset) {
            return None;
        }
        // Prefer the next statement's start over the previous statement's end.
        if let Some(range) = self.statements.iter().find(|range| range.contains(&offset)) {
            return Some(range.clone());
        }
        if self.comments.iter().any(|range| range.contains(&offset)) {
            return None;
        }
        self.statements
            .iter()
            .find(|range| {
                let gap = if offset < range.start {
                    offset..range.start
                } else {
                    range.end..offset
                };
                same_line_space(&sql[gap])
            })
            .cloned()
    }

    /// The range to highlight for a run at `offset`: the statement, extended
    /// over the whitespace before it and after it on its first and last line.
    /// A line that only the statement uses then shows from edge to edge.
    pub fn run_highlight(&self, sql: &str, offset: usize) -> Option<Range<usize>> {
        let range = self.range_at(sql, offset)?;
        let line_start = sql[..range.start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = sql[range.end..]
            .find(['\r', '\n'])
            .map_or(sql.len(), |i| range.end + i);
        let start = if same_line_space(&sql[line_start..range.start]) {
            line_start
        } else {
            range.start
        };
        let end = if same_line_space(&sql[range.end..line_end]) {
            line_end
        } else {
            range.end
        };
        Some(start..end)
    }
}

fn same_line_space(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_whitespace() && !matches!(c, '\n' | '\r'))
}

pub fn validate_single(sql: &str) -> anyhow::Result<()> {
    validate_single_for(sql, crate::model::DatabaseType::Kyuubi)
}

pub fn validate_single_for(
    sql: &str,
    database_type: crate::model::DatabaseType,
) -> anyhow::Result<()> {
    let mut statements = 0;
    let mut content = false;
    for (range, kind) in tokens_for(sql, database_type) {
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
    match format_statement_with_mode(sql, style, FormatMode::Assistant) {
        FormatOutcome::Formatted(text) => Some(text),
        _ => None,
    }
}

/// Formatting initiated by the user has no minimum statement length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormatMode {
    Assistant,
    Manual,
}

/// Why formatting changed or kept the input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FormatOutcome {
    Formatted(String),
    TooShort,
    Unsupported,
    Unchanged,
    Rejected,
}

pub fn format_statement_with_mode(sql: &str, style: SqlStyle, mode: FormatMode) -> FormatOutcome {
    format_statement_for(sql, style, mode, DatabaseType::Kyuubi)
}

/// Formats SQL with the connection's token and quoting rules.
pub fn format_statement_for(
    sql: &str,
    style: SqlStyle,
    mode: FormatMode,
    database_type: DatabaseType,
) -> FormatOutcome {
    const FORMATTABLE: [&str; 10] = [
        "SELECT", "WITH", "FROM", "VALUES", "INSERT", "CREATE", "EXPLAIN", "MERGE", "UPDATE",
        "DELETE",
    ];
    let tokens = significant(sql, database_type);
    let chars: usize = tokens
        .iter()
        .map(|t| t.text.chars().count() + usize::from(t.space_before))
        .sum();
    if mode == FormatMode::Assistant && chars <= 80 {
        return FormatOutcome::TooShort;
    }
    let Some(first) = tokens.into_iter().find(|t| t.kind != Kind::Comment) else {
        return FormatOutcome::Unsupported;
    };
    if !FORMATTABLE.contains(&first.text.to_ascii_uppercase().as_str()) {
        return FormatOutcome::Unsupported;
    }
    let options = FormatOptions {
        indent: Indent::Spaces(style.indent_spaces),
        // sqlformat changes only reserved keywords, not identifiers such as
        // `t.Date` or a column named `user`.
        uppercase: Some(style.keyword_case == KeywordCase::Uppercase),
        // PostgreSQL layout keeps Spark, Postgres, and Trino subscripts together.
        dialect: Dialect::PostgreSql,
        max_inline_top_level: Some(60),
        // Without this limit, sqlformat 0.5 puts each item of an inline clause,
        // such as `SELECT a, b`, on a new line without indent.
        max_inline_arguments: Some(60),
        ..FormatOptions::default()
    };
    let formatted = format_layout(sql, &options, database_type);
    let formatted = case_other_keywords(formatted.trim(), style.keyword_case, database_type);
    if !same_tokens_for(sql, &formatted, database_type) {
        FormatOutcome::Rejected
    } else if formatted == sql {
        FormatOutcome::Unchanged
    } else {
        FormatOutcome::Formatted(formatted)
    }
}

// sqlformat treats backslashes as escapes in every dialect and does not
// recognize dollar quotes. Hide quoted tokens during layout, then restore
// their exact text before the dialect-aware token check.
fn format_layout(sql: &str, options: &FormatOptions<'_>, database_type: DatabaseType) -> String {
    let normalized = normalize_layout_whitespace(sql, database_type);
    let sql = normalized.as_str();
    let mut prefix = "__qrow_format_token_".to_string();
    while sql.contains(&prefix) {
        prefix.push('_');
    }
    let mut protected = Vec::new();
    let mut masked = String::new();
    let mut end = 0;
    let tokens = tokens_for(sql, database_type);
    for (i, (range, kind)) in tokens.iter().enumerate() {
        let mut range = range.clone();
        if matches!(kind, Kind::String | Kind::Identifier) {
            // Keep adjacent literal prefixes with their string. Layout must not
            // split DATE'…', X'…', or Postgres E'…' before the token check.
            let adjacent_prefix = *kind == Kind::String
                && sql[range.clone()].starts_with('\'')
                && i.checked_sub(1).is_some_and(|i| {
                    let (prefix_range, kind) = &tokens[i];
                    let prefix = sql[prefix_range.clone()].to_ascii_lowercase();
                    *kind == Kind::Plain
                        && prefix_range.end == range.start
                        && (LITERAL_PREFIXES.contains(&prefix.as_str())
                            || matches!(prefix.as_str(), "e" | "x" | "b" | "n"))
                });
            if adjacent_prefix {
                let prefix_range = &tokens[i - 1].0;
                range.start = prefix_range.start;
            }
            if database_type == DatabaseType::Kyuubi && !adjacent_prefix {
                continue;
            }
            let marker = format!("'{prefix}{}'", protected.len());
            masked.push_str(&sql[end..range.start]);
            masked.push_str(&marker);
            protected.push((marker, &sql[range.clone()]));
            end = range.end;
        }
    }
    masked.push_str(&sql[end..]);
    // Canonicalize keyword case before the final layout. sqlformat's BETWEEN
    // state check is case-sensitive, so a single pass can change on the next run.
    let canonical = sqlformat::format(&masked, &QueryParams::None, options);
    let canonical = normalize_layout_whitespace(&canonical, database_type);
    let mut formatted = sqlformat::format(&canonical, &QueryParams::None, options);
    for (marker, original) in protected {
        formatted = formatted.replace(&marker, original);
    }
    formatted
}

fn normalize_layout_whitespace(sql: &str, database_type: DatabaseType) -> String {
    // sqlformat counts source whitespace when choosing inline layouts.
    let mut normalized = String::new();
    let mut after_line_comment = false;
    for (range, kind) in tokens_for(sql, database_type) {
        let token = &sql[range];
        if token.trim().is_empty() {
            normalized.push(if after_line_comment && token.contains(['\n', '\r']) {
                '\n'
            } else {
                ' '
            });
        } else {
            normalized.push_str(token);
            after_line_comment = kind == Kind::Comment && token.starts_with("--");
        }
    }
    normalized
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
    "time",
    "interval",
    "timestamp",
    "timestamp_ltz",
    "timestamp_ntz",
];

/// Writes in `case` the keywords that sqlformat keeps as written: built-in
/// function names, cast types, typed literal prefixes, and `NULLS FIRST` or
/// `NULLS LAST`. Words next to `.` keep their case.
fn case_other_keywords(sql: &str, case: KeywordCase, database_type: DatabaseType) -> String {
    let words: Vec<_> = tokens_for(sql, database_type)
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
    line_break_before: bool,
}

fn significant(sql: &str, database_type: DatabaseType) -> Vec<Significant<'_>> {
    let mut result = vec![];
    let mut space_before = false;
    let mut line_break_before = false;
    for (range, kind) in tokens_for(sql, database_type) {
        let text = &sql[range];
        if text.trim().is_empty() {
            space_before = true;
            line_break_before |= text.contains(['\n', '\r']);
        } else {
            result.push(Significant {
                kind,
                text,
                space_before,
                line_break_before,
            });
            space_before = false;
            if kind == Kind::Comment {
                line_break_before |= text.contains(['\n', '\r']);
            } else {
                line_break_before = false;
            }
        }
    }
    result
}

/// Whether `formatted` has the tokens of `original` and changes whitespace only
/// between tokens that stay separate tokens in Spark SQL. For example, `>=`,
/// `${var}`, and `X'1F'` must stay together. Delimiters such as `,` and `(`
/// never join with other symbols. Words can change letter case, except next to
/// `.`, where they name a field, a column, or a table.
#[cfg(test)]
fn same_tokens(original: &str, formatted: &str) -> bool {
    same_tokens_for(original, formatted, DatabaseType::Kyuubi)
}

pub(crate) fn same_tokens_for(
    original: &str,
    formatted: &str,
    database_type: DatabaseType,
) -> bool {
    let (original, formatted) = (
        significant(original, database_type),
        significant(formatted, database_type),
    );
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
                let continued_string = database_type == DatabaseType::Postgres
                    && a.kind == Kind::String
                    && original[..i]
                        .iter()
                        .rev()
                        .find(|t| t.kind != Kind::Comment)
                        .is_some_and(|t| t.kind == Kind::String);
                a.kind == b.kind
                    && (!continued_string || a.line_break_before == b.line_break_before)
                    && (a.text == b.text
                        || is_word(a) && !qualified && a.text.eq_ignore_ascii_case(b.text))
                    && (i == 0
                        || a.space_before == b.space_before
                        || !(is_symbol(&original[i - 1]) && is_symbol(a)
                            || is_word(&original[i - 1]) && a.kind == Kind::String))
            })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod postgres_tests;

#[cfg(test)]
mod dialect_tests;

#[cfg(test)]
mod trino_tests;
