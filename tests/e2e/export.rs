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
    for marker in ["Empty", "NULL", "\\N"] {
        for quote_all in [false, true] {
            app.click(cx, "export-results");
            app.select(cx, "export-preset", "Standard (RFC 4180)");
            app.select(cx, "export-null", marker);
            if quote_all {
                app.click(cx, "export-quotes");
            }
            let text = copy_query(cx, &app);
            assert_reader_rows(
                &text,
                if marker == "Empty" { "" } else { marker },
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
