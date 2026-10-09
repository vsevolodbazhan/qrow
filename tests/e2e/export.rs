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
        if format == "JSON Lines" {
            assert_eq!(text.lines().count(), 1, "{text}");
        }
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
    assert_parquet(cx, app);
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn kyuubi_all_rows_retries_from_the_completed_spool_after_a_new_query(cx: &mut TestAppContext) {
    let (mut workspace, credentials) = Kyuubi::get().workspace(
        "SELECT id AS value FROM range(100123) ORDER BY id",
        PASSWORD,
    );
    workspace.settings.export_replay_limit_mib = 0;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Preview: More rows available");
    let directory = tempfile::tempdir().unwrap();
    let blocked = directory.path().join("blocked.csv");
    std::fs::create_dir(&blocked).unwrap();
    app.click(cx, "export-results");
    app.select(cx, "export-rows", "All rows");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(blocked.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_label(cx, "Retry…");
    assert!(blocked.is_dir());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    // With no tab replay, a fresh dialog can still use the failed job's spool.
    let shared = directory.path().join("shared.csv");
    app.click(cx, "export-results");
    app.select(cx, "export-rows", "All rows");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(shared.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the shared spool file", QUERY_TIMEOUT, |_, _| {
        shared.exists()
    });
    assert_eq!(
        csv::Reader::from_path(shared).unwrap().records().count(),
        100123
    );
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
    app.run_complete(cx, "SELECT 42 AS value");
    app.wait_cell(cx, 0, 1, "42");
    app.click_labelled(cx, "Retry…");
    app.select(cx, "export-format", "JSON Lines");
    let path = directory.path().join("retried.jsonl");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the retried file", QUERY_TIMEOUT, |_, _| path.exists());
    let text = std::fs::read_to_string(path).unwrap();
    for (expected, line) in text.lines().enumerate() {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(row["value"].as_u64(), Some(expected as u64));
    }
    assert_eq!(text.lines().count(), 100123);
    app.wait_cell(cx, 0, 1, "42");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 2);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn kyuubi_all_rows_and_export_again_use_one_cursor_in_order(cx: &mut TestAppContext) {
    let (workspace, credentials) =
        Kyuubi::get().workspace("SELECT id AS value FROM range(2123) ORDER BY id", PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Preview: More rows available");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("all.csv");
    app.click(cx, "export-results");
    app.select(cx, "export-rows", "All rows");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the All rows file", QUERY_TIMEOUT, |_, _| path.exists());
    let bytes = std::fs::read(path).unwrap();
    let records = csv::Reader::from_reader(bytes.as_slice())
        .records()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(records.len(), 2123);
    for (expected, row) in records.iter().enumerate() {
        assert_eq!(&row[0], expected.to_string());
    }
    app.wait_status(cx, "Preview: Export download complete");
    app.click(cx, "export-results");
    app.select(cx, "export-rows", "Export again as… (2123 rows, 0.0 MiB)");
    app.select(cx, "export-format", "JSON Lines");
    let path = directory.path().join("replay.jsonl");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the replay file", QUERY_TIMEOUT, |_, _| path.exists());
    let text = std::fs::read_to_string(path).unwrap();
    assert_eq!(text.lines().count(), 2123);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text.lines().last().unwrap()).unwrap()["value"],
        2122
    );
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn kyuubi_all_rows_cancel_removes_the_output_and_releases_the_session(cx: &mut TestAppContext) {
    let (mut workspace, credentials) = Kyuubi::get().workspace(
        "SELECT id AS value FROM range(1000123) ORDER BY id",
        PASSWORD,
    );
    let gate = response_gate::ResponseGate::new(workspace.profiles[0].port);
    workspace.profiles[0].port = gate.port;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Preview: More rows available");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cancelled.csv");
    std::fs::write(&path, "original").unwrap();
    gate.pause();
    app.click(cx, "export-results");
    app.select(cx, "export-rows", "All rows");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the blocked export response", QUERY_TIMEOUT, |_, _| {
        gate.blocked()
    });
    app.click_labelled(cx, "Cancel export");
    gate.resume();
    app.wait_until(cx, "cancelled export cleanup", QUERY_TIMEOUT, |_, cx| {
        cx.global::<qrow::export::Jobs>().active_count() == 0
            && app.status(cx).starts_with("Export download stopped")
    });
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    app.run_complete(cx, "SELECT 42 AS value");
    app.wait_cell(cx, 0, 1, "42");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 2);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn kyuubi_all_rows_preserves_the_batch_that_crossed_the_preview_byte_limit(
    cx: &mut TestAppContext,
) {
    let (workspace, credentials) = Kyuubi::get().workspace(
        "SELECT id AS value, repeat('x', 35000) AS wide FROM range(2200) ORDER BY id",
        PASSWORD,
    );
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Preview: More rows available");
    app.click(cx, "next-page");
    app.wait_status(cx, "Preview: Limit reached");
    app.wait_cell(cx, 0, 1, "0");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("wide.csv");
    app.click(cx, "export-results");
    app.select(cx, "export-rows", "All rows");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the wide All rows file", QUERY_TIMEOUT, |_, _| {
        path.exists()
    });
    let mut count = 0;
    for row in csv::Reader::from_path(path).unwrap().records() {
        let row = row.unwrap();
        assert_eq!(&row[0], count.to_string());
        assert_eq!(row[1].len(), 35000);
        assert!(row[1].bytes().all(|byte| byte == b'x'));
        count += 1;
    }
    assert_eq!(count, 2200);
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn kyuubi_run_export_keeps_session_state_and_only_the_first_page(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt;
    let (workspace, credentials) = Kyuubi::get().workspace(
        "CREATE TEMPORARY VIEW qrow_direct_export AS SELECT id FROM range(2123)",
        PASSWORD,
    );
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
    app.type_sql(cx, "SELECT id AS value FROM qrow_direct_export ORDER BY id");
    app.run_export(cx);
    app.wait_for(cx, "export-save");
    app.click(cx, "export-copy");
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("direct.csv");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the direct export", QUERY_TIMEOUT, |_, _| {
        output.exists()
    });
    let records = csv::Reader::from_path(output)
        .unwrap()
        .records()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(records.len(), 2123);
    for (expected, row) in records.iter().enumerate() {
        assert_eq!(&row[0], expected.to_string());
    }
    app.update(cx, |window, _| {
        assert_eq!(window.find("result-loaded").label(), Some("1000 loaded"))
    });
    app.wait_cell(cx, 0, 1, "0");
    app.wait_status(cx, "Preview: Export download complete");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 2);
    assert_eq!(app.logs(cx).matches("Connected to").count(), 1);
    app.run_complete(cx, "SELECT COUNT(*) AS value FROM qrow_direct_export");
    app.wait_cell(cx, 0, 1, "2123");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn kyuubi_run_export_again_uses_result_sql_and_warns_after_disconnect(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt;
    let sql = "SELECT id AS value FROM range(2123) ORDER BY id";
    let (workspace, credentials) = Kyuubi::get().workspace(sql, PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Preview: More rows available");
    app.type_sql(cx, "SELECT 987654 AS must_not_run");
    app.click(cx, "disconnect");
    app.wait_status(cx, "Disconnected");
    app.click(cx, "export-results");
    app.select(cx, "export-rows", "Run again and export (all rows)");
    app.update(cx, |window, _| {
        assert_eq!(window.find("export-captured-sql").label(), Some(sql));
        assert!(
            window
                .find("export-session-warning")
                .label()
                .unwrap()
                .contains("temporary tables")
        );
    });
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("again.jsonl");
    app.select(cx, "export-format", "JSON Lines");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the rerun export", QUERY_TIMEOUT, |_, _| {
        output.exists()
    });
    let text = std::fs::read_to_string(output).unwrap();
    assert_eq!(text.lines().count(), 2123);
    for (expected, line) in text.lines().enumerate() {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(line).unwrap()["value"],
            expected
        );
    }
    app.update(cx, |window, _| {
        assert_eq!(window.find("result-loaded").label(), Some("1000 loaded"))
    });
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 2);
    assert!(!app.logs(cx).contains("987654"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn kyuubi_run_export_rechecks_idle_disconnect_after_the_save_panel(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt;
    let (mut workspace, credentials) =
        Kyuubi::get().workspace("SELECT id AS value FROM range(2123) ORDER BY id", PASSWORD);
    workspace.profiles[0].lifecycle.idle_seconds = 5;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Preview: More rows available");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
    app.click(cx, "export-results");
    app.select(cx, "export-rows", "Run again and export (all rows)");
    app.update(cx, |window, _| {
        assert!(window.try_find("export-session-warning").is_none())
    });
    app.click(cx, "export-save");
    app.wait_status(cx, "Disconnected: Idle timeout");
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("reviewed.csv");
    std::fs::write(&output, "original").unwrap();
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    app.wait_for(cx, "export-session-warning");
    assert_eq!(
        app.update(cx, |_, cx| app.status(cx)),
        "Disconnected: Idle timeout"
    );
    assert_eq!(std::fs::read_to_string(&output).unwrap(), "original");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the reviewed rerun", QUERY_TIMEOUT, |_, _| {
        std::fs::metadata(&output).is_ok_and(|metadata| metadata.len() > 8)
    });
    assert_eq!(
        csv::Reader::from_path(output).unwrap().records().count(),
        2123
    );
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 2);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn kyuubi_run_export_explicit_rerun_refreshes_preview_and_retry_survives_tab_close(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestWindowExt;
    let (workspace, credentials) =
        Kyuubi::get().workspace("SELECT rand() AS value FROM range(1001)", PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.csv");
    app.run_export(cx);
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(first.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the first random export", QUERY_TIMEOUT, |_, _| {
        first.exists()
    });
    let old = csv::Reader::from_path(first)
        .unwrap()
        .records()
        .next()
        .unwrap()
        .unwrap()[0]
        .to_owned();
    app.wait_status(cx, "Preview: Export download complete");
    app.click(cx, "export-results");
    app.select(cx, "export-rows", "Run again and export (all rows)");
    let blocked = directory.path().join("blocked.csv");
    std::fs::create_dir(&blocked).unwrap();
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(blocked.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_label(cx, "Retry…");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 2);
    let fresh = app.update(cx, |window, _| {
        find_in(window, ("row", 0usize), ("cell", 1usize))
            .unwrap()
            .label()
            .unwrap()
            .to_owned()
    });
    assert_ne!(fresh, old);
    app.click_labelled(cx, "Retry…");
    app.wait_until(
        cx,
        "the refreshed format preview",
        QUERY_TIMEOUT,
        |window, _| {
            window
                .find("export-preview")
                .label()
                .is_some_and(|text| text.contains(&fresh) && !text.contains(&old))
        },
    );
    app.press(cx, "escape");
    app.wait_gone(cx, "export-save");
    let closed_tab = app.saved().tabs[0].id;
    app.click(cx, format!("close-tab-{closed_tab}"));
    app.wait_until(cx, "the originating tab to close", QUERY_TIMEOUT, |_, _| {
        !app.saved().tabs.iter().any(|tab| tab.id == closed_tab)
    });
    let reads = app.credentials.reads();
    app.click_labelled(cx, "Retry…");
    app.select(cx, "export-format", "JSON Lines");
    let retry = directory.path().join("retry.jsonl");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(retry.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the tab-independent retry", QUERY_TIMEOUT, |_, _| {
        retry.exists()
    });
    let text = std::fs::read_to_string(retry).unwrap();
    assert_eq!(text.lines().count(), 1001);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text.lines().next().unwrap()).unwrap()["value"]
            .as_f64()
            .unwrap(),
        fresh.parse::<f64>().unwrap()
    );
    assert_eq!(app.credentials.reads(), reads);
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(String::new()));
    assert!(!app.logs(cx).contains("Submitted query:"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn kyuubi_run_export_empty_result_retry_has_the_new_column_preview(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt;
    let (workspace, credentials) =
        Kyuubi::get().workspace("SELECT 1 AS empty_value WHERE false", PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    let directory = tempfile::tempdir().unwrap();
    let blocked = directory.path().join("blocked.csv");
    std::fs::create_dir(&blocked).unwrap();
    app.run_export(cx);
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(blocked.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_label(cx, "Retry…");
    app.click_labelled(cx, "Retry…");
    app.wait_until(
        cx,
        "the empty result headers",
        QUERY_TIMEOUT,
        |window, _| {
            window
                .find("export-preview")
                .label()
                .is_some_and(|text| text == "empty_value\r\n")
        },
    );
    let output = directory.path().join("empty.csv");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_until(cx, "the empty export", QUERY_TIMEOUT, |_, _| {
        output.exists()
    });
    assert_eq!(std::fs::read_to_string(output).unwrap(), "empty_value\r\n");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}

fn assert_parquet(cx: &mut TestAppContext, app: &TestApp) {
    for compression in ["Snappy", "Gzip", "None"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("result.parquet");
        app.click(cx, "export-results");
        app.select(cx, "export-format", "Parquet");
        app.select(cx, "export-parquet-compression", compression);
        app.click(cx, "export-save");
        cx.simulate_new_path_selection(|_| Some(path.clone()));
        app.wait_gone(cx, "export-save");
        let status = Command::new("python")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/scripts/e2e/parquet_reader.py"
            ))
            .arg(&path)
            .status()
            .expect("Run through ./qtest for the pinned DuckDB reader");
        assert!(
            status.success(),
            "DuckDB Parquet round trip failed with {compression}"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
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
    let (workspace, credentials) = Kyuubi::get().workspace(
        "SELECT repeat('x', 5242881) AS value FROM range(2)",
        PASSWORD,
    );
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
    assert_eq!(bytes.len(), b"value\r\n".len() + 2 * (5242881 + 2));
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
