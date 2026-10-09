use crate::support::fixture::{Kyuubi, PASSWORD, QUERY_TIMEOUT};
use crate::support::*;
use gpui_kit::{ClipboardItem, TestAppContext};
use std::{
    io::Write,
    process::{Command, Stdio},
};

pub(super) fn assert_reader_rows(text: &str, null_marker: &str, rows: serde_json::Value) {
    let mut child = Command::new("python")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scripts/e2e/csv_reader.py"
        ))
        .stdin(Stdio::piped())
        .spawn()
        .expect("Run through ./qtest for the pinned DuckDB reader");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            serde_json::to_string(
                &serde_json::json!({"csv":text, "null":null_marker, "rows":rows}),
            )
            .unwrap()
            .as_bytes(),
        )
        .unwrap();
    assert!(
        child.wait().unwrap().success(),
        "DuckDB CSV round trip failed"
    );
}

pub(super) fn copy_query(cx: &mut TestAppContext, app: &TestApp) -> String {
    cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
    app.click(cx, "export-copy");
    app.wait_until(cx, "CSV query text", QUERY_TIMEOUT, |_, cx| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|text| text != "before")
    });
    app.wait_gone(cx, "export-copy");
    cx.read_from_clipboard().unwrap().text().unwrap()
}

pub(super) fn assert_markdown_json(cx: &mut TestAppContext, app: &TestApp) {
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    for format in ["JSON array", "JSON Lines"] {
        app.click(cx, "export-results");
        app.select(cx, "export-format", format);
        let text = copy_query(cx, app);
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let row = if format == "JSON array" {
            &json[0]
        } else {
            &json
        };
        assert_eq!(row["text"], "a|b");
        assert!(row["absent"].is_null());
        assert_eq!(row["empty"], "");
        assert_eq!(row["amount"], "123.45");
        assert_eq!(row["id"].as_i64(), Some(i64::MAX));
        assert_eq!(row["ok"], true);
        assert_eq!(row["bytes"], "AFz/");
        assert_eq!(row["items"], serde_json::json!([1, 2]));
        assert_eq!(row["day"], "2026-10-09");
        assert!(
            row["moment"]
                .as_str()
                .unwrap()
                .starts_with("2026-10-09 01:02:03")
        );
    }
    app.click(cx, "export-results");
    app.select(cx, "export-format", "JSON array");
    app.click(cx, "export-json-decimals");
    let text = copy_query(cx, app);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json[0]["amount"].as_number().unwrap().to_string(), "123.45");
    app.click(cx, "export-results");
    app.select(cx, "export-format", "Markdown");
    let text = copy_query(cx, app);
    assert!(text.contains("a\\|b | NULL |  | 123.45 |"), "{text}");
    assert_eq!(text.lines().count(), 3);
    app.click(cx, "export-results");
    app.select(cx, "export-markdown-style", "Code block");
    let text = copy_query(cx, app);
    assert!(text.starts_with("```\n"));
    assert!(text.ends_with("```\n"));
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn markdown_json_export_normalizes_real_kyuubi_values(cx: &mut TestAppContext) {
    let (workspace, credentials)=Kyuubi::get().workspace(
        "SELECT 'a|b' AS text, CAST(NULL AS STRING) AS absent, '' AS empty, CAST(123.45 AS DECIMAL(10,2)) AS amount, CAST(9223372036854775807 AS BIGINT) AS id, TRUE AS ok, unhex('005cff') AS bytes, array(1,2) AS items, DATE '2026-10-09' AS day, TIMESTAMP '2026-10-09 01:02:03.123456' AS moment", PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    assert_markdown_json(cx, &app);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn csv_export_of_real_query_preserves_null_empty_quotes_and_unicode(cx: &mut TestAppContext) {
    let sql = "SELECT 'a,b' AS text, CAST(NULL AS STRING) AS absent, '' AS empty, '最初😀' AS unicode, CAST(123.45 AS DECIMAL(10,2)) AS amount UNION ALL SELECT 'say \"hi\"', 'NULL', '', 'line one\\nline two', CAST(-0.01 AS DECIMAL(10,2))";
    let (workspace, credentials) = Kyuubi::get().workspace(sql, PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "a,b");
    app.click(cx, "export-results");
    app.select(cx, "export-null", "NULL");
    cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
    app.click(cx, "export-copy");
    app.wait_until(cx, "exported query text", QUERY_TIMEOUT, |_, cx| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|text| text.starts_with("text,absent,empty,unicode,amount\r\n"))
    });
    let text = cx.read_from_clipboard().unwrap().text().unwrap();
    assert_eq!(
        text,
        "text,absent,empty,unicode,amount\r\n\"a,b\",NULL,\"\",最初😀,123.45\r\n\"say \"\"hi\"\"\",\"NULL\",\"\",\"line one\nline two\",-0.01\r\n"
    );
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    assert_eq!(reader.headers().unwrap().len(), 5);
    assert_eq!(reader.records().count(), 2);
    assert_reader_rows(
        &text,
        "NULL",
        serde_json::json!([
            ["a,b", null, "", "最初😀", "123.45"],
            ["say \"hi\"", "NULL", "", "line one\nline two", "-0.01"]
        ]),
    );
    app.wait_gone(cx, "export-copy");
    // Both quote modes must preserve nulls and empty strings for every marker.
    for marker in ["Empty", "NULL", "\\N", "Custom"] {
        for quote_all in [false, true] {
            app.click(cx, "export-results");
            app.select(cx, "export-preset", "Standard (RFC 4180)");
            app.select(cx, "export-null", marker);
            if marker == "Custom" {
                app.fill(cx, "export-null-text", "未知😀");
            }
            if quote_all {
                app.click(cx, "export-quotes");
            }
            let text = copy_query(cx, &app);
            assert_reader_rows(
                &text,
                match marker {
                    "Empty" => "",
                    "Custom" => "未知😀",
                    _ => marker,
                },
                serde_json::json!([
                    ["a,b", null, "", "最初😀", "123.45"],
                    ["say \"hi\"", "NULL", "", "line one\nline two", "-0.01"]
                ]),
            );
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("query.csv");
    app.click(cx, "export-results");
    app.select(cx, "export-preset", "Excel (semicolon)");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    app.wait_gone(cx, "export-save");
    assert!(
        std::fs::read(&path)
            .unwrap()
            .starts_with(b"\xef\xbb\xbftext;absent;empty;unicode;amount\r\n")
    );
    app.wait_until(cx, "persisted export settings", QUERY_TIMEOUT, |_, _| {
        let settings = app.saved().settings.export;
        settings.csv == qrow::export::csv::Preset::ExcelSemicolon.options()
            && settings.directory.as_deref() == Some(directory.path())
    });
    // Exporting the preview must not submit SQL again.
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn csv_copy_limit_keeps_the_clipboard_and_allows_save_after_an_error(cx: &mut TestAppContext) {
    let (workspace, credentials) =
        Kyuubi::get().workspace("SELECT repeat('x', 10485761) AS value", PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.click(cx, "export-results");
    cx.write_to_clipboard(ClipboardItem::new_string("preserve this".into()));
    app.click(cx, "export-copy");
    app.wait_label(
        cx,
        "The text exceeds the copy limit. Save it to a file instead.",
    );
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().as_deref(),
        Some("preserve this")
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large.csv");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    app.wait_gone(cx, "export-save");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), b"value\r\n".len() + 10485761 + 2);
    assert!(bytes.starts_with(b"value\r\nxxx"));
    assert!(bytes.ends_with(b"xxx\r\n"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn custom_csv_marker_round_trips_null_empty_and_literal_marker(cx: &mut TestAppContext) {
    let (workspace, credentials) = Kyuubi::get().workspace(
        "SELECT CAST(NULL AS STRING) AS value, '' AS empty UNION ALL SELECT '未知😀', ''",
        PASSWORD,
    );
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    for quote_all in [false, true] {
        app.click(cx, "export-results");
        app.select(cx, "export-preset", "Standard (RFC 4180)");
        app.select(cx, "export-null", "Custom");
        app.fill(cx, "export-null-text", "未知😀");
        if quote_all {
            app.click(cx, "export-quotes");
        }
        let text = copy_query(cx, &app);
        assert_reader_rows(
            &text,
            "未知😀",
            serde_json::json!([[null, ""], ["未知😀", ""]]),
        );
    }
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn drag_selection_exports_only_the_selected_real_query_cells(cx: &mut TestAppContext) {
    let (workspace, credentials) = Kyuubi::get().workspace(
        "SELECT id, concat('row-', id) AS label, id * 10 AS amount FROM range(5) ORDER BY id",
        PASSWORD,
    );
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 3, 1, "3");
    app.update(cx, |window, cx| {
        let from = find_in(window, ("row", 1usize), ("cell", 1usize)).unwrap();
        let to = find_in(window, ("row", 3usize), ("cell", 2usize)).unwrap();
        pointer_drag(window, &from, &to, cx);
    });
    app.settle(cx);
    app.click(cx, "export-results");
    let text = copy_query(cx, &app);
    assert_eq!(text, "id,label\r\n1,row-1\r\n2,row-2\r\n3,row-3\r\n");
    assert_reader_rows(
        &text,
        "",
        serde_json::json!([["1", "row-1"], ["2", "row-2"], ["3", "row-3"]]),
    );
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}
