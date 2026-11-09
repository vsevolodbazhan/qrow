use super::*;
#[test]
fn formats_real_trino_cost_queries() {
    let sql = include_str!("../../tests/fixtures/trino-format.sql");
    let ranges = statement_ranges_for(sql, DatabaseType::Trino);
    assert_eq!(ranges.len(), 3);
    for range in ranges {
        let original = &sql[range];
        for keyword_case in [KeywordCase::Uppercase, KeywordCase::Lowercase] {
            let style = SqlStyle {
                keyword_case,
                ..SqlStyle::default()
            };
            for mode in [FormatMode::Manual, FormatMode::Assistant] {
                let outcome = format_statement_for(original, style, mode, DatabaseType::Trino);
                let FormatOutcome::Formatted(formatted) = outcome else {
                    panic!("Expected formatting, got {outcome:?}: {original}");
                };
                assert!(same_tokens_for(original, &formatted, DatabaseType::Trino));
                assert!(formatted.starts_with(match keyword_case {
                    KeywordCase::Uppercase => "SELECT",
                    KeywordCase::Lowercase => "select",
                }));
                assert_eq!(
                    format_statement_for(&formatted, style, mode, DatabaseType::Trino),
                    FormatOutcome::Unchanged,
                );
            }
        }
    }
}

#[test]
fn formatting_preserves_adjacent_typed_literal_prefixes() {
    for database_type in [
        DatabaseType::Kyuubi,
        DatabaseType::Postgres,
        DatabaseType::Trino,
    ] {
        for sql in [
            "select date'2026-10-21'",
            "select timestamp'2026-10-21 10:00:00'",
            "select interval'2' day",
            "select X'1F'",
        ] {
            let outcome =
                format_statement_for(sql, SqlStyle::default(), FormatMode::Manual, database_type);
            let FormatOutcome::Formatted(formatted) = outcome else {
                panic!("{database_type:?}: {sql}: {outcome:?}");
            };
            assert!(same_tokens_for(sql, &formatted, database_type));
            assert!(!formatted.contains(" '"), "{formatted}");
        }
    }
}

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
fn manual_format_outcomes_and_assistant_minimum() {
    let style = SqlStyle::default();
    assert_eq!(
        format_statement_with_mode("select 1", style, FormatMode::Assistant),
        FormatOutcome::TooShort
    );
    assert_eq!(
        format_statement_with_mode("select 1", style, FormatMode::Manual),
        FormatOutcome::Formatted("SELECT 1".into())
    );
    assert_eq!(
        format_statement_with_mode("SELECT 1", style, FormatMode::Manual),
        FormatOutcome::Unchanged
    );
    for sql in ["SET x=1", "-- only a comment", ""] {
        assert_eq!(
            format_statement_with_mode(sql, style, FormatMode::Manual),
            FormatOutcome::Unsupported
        );
    }
    assert_eq!(
        format_statement_with_mode("select ${hivevar:dt}", style, FormatMode::Manual),
        FormatOutcome::Rejected
    );
    for sql in [
        "select /*+ BROADCAST(t) */ `日 本`, '😀; x' from t where a>=1",
        "select 'a  b', 'it\\'s' from t -- keep  this",
        "select `a``b` from t where a!=b",
    ] {
        match format_statement_with_mode(sql, style, FormatMode::Manual) {
            FormatOutcome::Formatted(text) => assert!(same_tokens(sql, &text)),
            FormatOutcome::Rejected | FormatOutcome::Unchanged => {}
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
}

#[test]
fn formats_each_dialect_without_changing_quoted_tokens() {
    for (database_type, sql) in [
        (
            DatabaseType::Postgres,
            r#"select $tag$select; 'Mixed' 日本語😀$tag$ as "Case Sensitive", e'it\'s;fine', E'escaped\\path', ARRAY[1,2][1]"#,
        ),
        (
            DatabaseType::Postgres,
            r#"select 'path\' as "name\", $$a; b$$::text, $1 as value"#,
        ),
        (
            DatabaseType::Trino,
            r#"select 'path\' as "name\", ARRAY[1,2][1] as "Mixed Case", 'it''s;fine'"#,
        ),
        (
            DatabaseType::Postgres,
            r#"select '__qrow_format_token_0', '日本語😀' as "a""b""#,
        ),
    ] {
        let outcome =
            format_statement_for(sql, SqlStyle::default(), FormatMode::Manual, database_type);
        let FormatOutcome::Formatted(formatted) = outcome else {
            panic!("Expected formatting for {database_type:?}: {sql}, got {outcome:?}");
        };
        assert!(formatted.starts_with("SELECT"), "{formatted}");
        assert!(
            same_tokens_for(sql, &formatted, database_type),
            "{formatted}"
        );
        let quoted = |text: &str| {
            tokens_for(text, database_type)
                .into_iter()
                .filter(|(_, kind)| matches!(kind, Kind::String | Kind::Identifier))
                .map(|(range, _)| text[range].to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(quoted(sql), quoted(&formatted));
    }
}

#[test]
fn dialect_formatting_keeps_unicode_prefixes_and_required_newlines() {
    for database_type in [DatabaseType::Postgres, DatabaseType::Trino] {
        for sql in [r#"select U&"d\0061t" from t"#, r"select U&'d\0061t'"] {
            let FormatOutcome::Formatted(text) =
                format_statement_for(sql, SqlStyle::default(), FormatMode::Manual, database_type)
            else {
                panic!("{sql}")
            };
            assert!(text.contains("U&"), "{text}");
            assert!(!text.contains("U &"), "{text}");
            assert!(same_tokens_for(sql, &text, database_type));
        }
    }
    for sql in [
        "select 'foo'\n'bar'",
        "select E'foo'\n'bar'",
        "select 'foo'\n/*keep*/ 'bar'",
    ] {
        let outcome = format_statement_for(
            sql,
            SqlStyle::default(),
            FormatMode::Manual,
            DatabaseType::Postgres,
        );
        if let FormatOutcome::Formatted(text) = outcome {
            assert!(same_tokens_for(sql, &text, DatabaseType::Postgres));
            assert!(text.contains('\n'), "{text}");
        } else {
            assert_eq!(outcome, FormatOutcome::Rejected);
        }
    }
    assert!(!same_tokens_for(
        "select 'foo'\n'bar'",
        "SELECT 'foo' 'bar'",
        DatabaseType::Postgres
    ));
}

#[test]
fn dialect_cursor_ranges_keep_semicolons_inside_quoted_tokens() {
    for (database_type, sql) in [
        (
            DatabaseType::Postgres,
            "select $tag$one;two$tag$;\nselect 2;",
        ),
        (DatabaseType::Trino, r#"select 'path\', "a;b"; select 2;"#),
    ] {
        let ranges = statement_ranges_for(sql, database_type);
        assert_eq!(ranges.len(), 2);
        assert_eq!(
            statement_range_at_for(sql, sql.find(';').unwrap(), database_type),
            Some(ranges[0].clone())
        );
        assert_eq!(
            statement_range_at_for(sql, sql.len(), database_type),
            Some(ranges[1].clone())
        );
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

#[test]
fn run_highlight_fills_lines_that_only_the_statement_uses() {
    let sql = "  SELECT 1;  \n\nSELECT 2; SELECT 3; -- note\nSELECT\n  '日本😀'\r\nFROM t;\n";
    let index = StatementIndex::new(sql);
    let highlight = |marker: &str| {
        let range = index.run_highlight(sql, sql.find(marker).unwrap()).unwrap();
        &sql[range]
    };
    assert_eq!(highlight("SELECT 1"), "  SELECT 1;  ");
    // Statements that share a line keep their own extent.
    assert_eq!(highlight("SELECT 2"), "SELECT 2;");
    assert_eq!(highlight("SELECT 3"), "SELECT 3;");
    assert_eq!(highlight("SELECT\n"), "SELECT\n  '日本😀'\r\nFROM t;");
    assert_eq!(
        index.run_highlight(sql, sql.find("\n\n").unwrap() + 1),
        None
    );
    assert_eq!(index.run_highlight(sql, sql.find("-- note").unwrap()), None);
    assert_eq!(index.run_highlight(sql, sql.find('😀').unwrap() + 1), None);
}

#[test]
fn statement_index_finds_the_same_statements_as_each_lookup() {
    let sql = "SELECT 1;SELECT 2;-- between\nSELECT 3;/* between */SELECT 4;\n\n  SELECT 5  ";
    let index = StatementIndex::new(sql);
    assert_eq!(index.statements(), statement_ranges(sql));
    for offset in 0..=sql.len() {
        assert_eq!(index.range_at(sql, offset), statement_range_at(sql, offset));
    }
}
