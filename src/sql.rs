//! Small SQL lexer for highlighting and single-statement validation, not SQL parsing.
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
}
