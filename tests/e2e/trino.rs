use crate::support::{MemoryCredentials, SignIns, TestApp, cell, connection_row, header};
use gpui_kit::{TestAppContext, test::TestWindowExt};
use qrow::{
    model::{DatabaseType, Profile, SavedTab, Workspace},
    storage::Credentials,
    tls::Trust,
};

fn launch(cx: &mut TestAppContext, sql: &str) -> TestApp {
    launch_with_preset(cx, sql, qrow::model::transfer::TransferPreset::Balanced)
}
fn launch_with_preset(
    cx: &mut TestAppContext,
    sql: &str,
    preset: qrow::model::transfer::TransferPreset,
) -> TestApp {
    let fixture = std::env::var("QROW_TRINO_FIXTURE").expect("Run with ./qtest run trino");
    assert!(fixture.starts_with("qrow-e2e-trino-"));
    let mut profile = Profile {
        database_type: DatabaseType::Trino,
        name: "Trino".into(),
        host: "localhost".into(),
        port: std::env::var("QROW_TRINO_PORT").unwrap().parse().unwrap(),
        username: "qrow".into(),
        database: "tpch".into(),
        trino_schema: "tiny".into(),
        tls: true,
        ..Profile::default()
    };
    profile.transfer.preset = preset;
    let credentials = MemoryCredentials::default();
    credentials
        .set_password(profile.id, "qrow-test-password")
        .unwrap();
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = sql.into();
    let workspace = Workspace {
        profiles: vec![profile],
        tabs: vec![tab],
        ..Workspace::default()
    };
    let trust =
        Trust::from_pem(&std::fs::read(std::env::var("QROW_TRINO_CA").unwrap()).unwrap()).unwrap();
    TestApp::launch_with_sign_ins(cx, workspace, credentials, SignIns::new(trust, None))
}

fn save_export(app: &TestApp, cx: &mut TestAppContext, path: &std::path::Path) {
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.to_owned()));
    app.wait_gone(cx, "export-save");
    app.wait_until(
        cx,
        "the published export",
        std::time::Duration::from_secs(40),
        |_, _| path.is_file(),
    );
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn cancelled_export_preserves_output_and_releases_the_tab_after_cleanup(cx: &mut TestAppContext) {
    let app = launch(cx, "SELECT count(*) AS value FROM tpch.sf1000.lineitem");
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("preserved.csv");
    std::fs::write(&output, "previous output").unwrap();
    app.run_export(cx);
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_status(cx, "Executing");
    app.click(cx, "cancel");
    app.wait_status(cx, "Export download stopped");
    assert_eq!(std::fs::read_to_string(output).unwrap(), "previous output");
    app.run_complete(cx, "SELECT 42 AS value");
    app.wait_cell(cx, 0, 1, "42");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 2);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn run_export_keeps_settings_prepared_statements_and_the_transaction(cx: &mut TestAppContext) {
    let app = launch(cx, "SELECT 1");
    app.run_complete(cx, "USE tpch.sf1");
    app.run_complete(cx, "SET SESSION query_max_run_time = '17m'");
    app.run_complete(cx, "PREPARE qrow_export FROM SELECT x*1000+y AS value, DECIMAL '12345678901234567890.12345' AS amount, '未知😀' AS text, current_schema AS schema FROM UNNEST(sequence(0,100)) t(x) CROSS JOIN UNNEST(sequence(1,1000)) u(y) WHERE x*1000+y <= 100001 ORDER BY value");
    app.run_complete(cx, "START TRANSACTION");
    app.type_sql(cx, "EXECUTE qrow_export");
    app.run_export(cx);
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("all.csv");
    save_export(&app, cx, &output);
    let mut count = 0;
    for (index, row) in csv::Reader::from_path(output)
        .unwrap()
        .records()
        .enumerate()
    {
        let row = row.unwrap();
        assert_eq!(&row[0], (index + 1).to_string());
        assert_eq!(&row[1], "12345678901234567890.12345");
        assert_eq!(&row[2], "未知😀");
        assert_eq!(&row[3], "sf1");
        count += 1;
    }
    assert_eq!(count, 100001);
    app.wait_status(cx, "Preview: Export download complete");
    app.wait_until(
        cx,
        "the first-page preview",
        std::time::Duration::from_secs(5),
        |window, _| window.find("result-loaded").label() == Some("1000 loaded"),
    );
    app.wait_cell(cx, 0, 1, "1");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 5);
    assert_eq!(app.logs(cx).matches("Connected to").count(), 1);
    app.click(cx, "export-results");
    app.select(cx, "export-rows", "All rows");
    app.select(cx, "export-format", "Parquet");
    let replay = directory.path().join("replay.parquet");
    save_export(&app, cx, &replay);
    use parquet::file::reader::{FileReader, SerializedFileReader};
    let reader = SerializedFileReader::new(std::fs::File::open(replay).unwrap()).unwrap();
    assert_eq!(reader.metadata().file_metadata().num_rows(), 100001);
    assert_eq!(
        reader.metadata().file_metadata().schema().get_fields()[1].get_precision(),
        25
    );
    assert_eq!(
        reader.metadata().file_metadata().schema().get_fields()[1].get_scale(),
        5
    );
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 5);
    app.run_complete(cx, "ROLLBACK");
    app.run_complete(cx, "SELECT current_schema AS value");
    app.wait_cell(cx, 0, 1, "sf1");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn run_export_exceeds_the_preview_byte_limit(cx: &mut TestAppContext) {
    let app = launch(
        cx,
        "SELECT i AS value, rpad('α', 40000, 'α') AS payload FROM UNNEST(sequence(1,2000)) t(i) ORDER BY i",
    );
    app.run_export(cx);
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("wide.csv");
    save_export(&app, cx, &output);
    let mut count = 0;
    for (index, row) in csv::Reader::from_path(output)
        .unwrap()
        .records()
        .enumerate()
    {
        let row = row.unwrap();
        assert_eq!(&row[0], (index + 1).to_string());
        assert_eq!(row[1].len(), 80000);
        count += 1;
    }
    assert_eq!(count, 2000);
    app.wait_status(cx, "Preview: Export download complete");
    app.update(cx, |window, _| {
        let loaded = window.find("result-loaded").label().unwrap().to_owned();
        let rows: usize = loaded.strip_suffix(" loaded").unwrap().parse().unwrap();
        assert!(rows > 0 && rows < 1000, "{loaded}");
    });
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn run_again_uses_result_sql_and_warns_after_disconnect(cx: &mut TestAppContext) {
    let sql = "SELECT i AS value FROM UNNEST(sequence(1,2001)) t(i) ORDER BY i";
    let app = launch(cx, sql);
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
    app.select(cx, "export-format", "JSON Lines");
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("again.jsonl");
    save_export(&app, cx, &output);
    let text = std::fs::read_to_string(output).unwrap();
    assert_eq!(text.lines().count(), 2001);
    for (index, line) in text.lines().enumerate() {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(line).unwrap()["value"],
            index + 1
        );
    }
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 2);
    assert!(!app.logs(cx).contains("987654"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn completed_spool_retry_survives_new_sql_and_tab_close(cx: &mut TestAppContext) {
    let app = launch(
        cx,
        "SELECT i AS value, '未知😀' AS text FROM UNNEST(sequence(1,2001)) t(i) ORDER BY i",
    );
    let directory = tempfile::tempdir().unwrap();
    let blocked = directory.path().join("blocked.csv");
    std::fs::create_dir(&blocked).unwrap();
    app.run_export(cx);
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(blocked.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_label(cx, "Retry…");
    app.wait_status(cx, "Preview: Export download complete");
    app.run_complete(cx, "SELECT 43 AS value");
    app.wait_cell(cx, 0, 1, "43");
    let closed = app.saved().tabs[0].id;
    app.click(cx, format!("close-tab-{closed}"));
    app.wait_until(
        cx,
        "the originating tab to close",
        std::time::Duration::from_secs(5),
        |_, _| !app.saved().tabs.iter().any(|tab| tab.id == closed),
    );
    let reads = app.credentials.reads();
    app.click_labelled(cx, "Retry…");
    app.select(cx, "export-format", "JSON Lines");
    let retry = directory.path().join("retry.jsonl");
    save_export(&app, cx, &retry);
    let text = std::fs::read_to_string(retry).unwrap();
    assert_eq!(text.lines().count(), 2001);
    for (index, line) in text.lines().enumerate() {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(row["value"], index + 1);
        assert_eq!(row["text"], "未知😀");
    }
    assert_eq!(app.credentials.reads(), reads);
    assert!(!app.logs(cx).contains("Submitted query:"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn empty_export_writes_a_header_and_no_result_preserves_output(cx: &mut TestAppContext) {
    let app = launch(cx, "SELECT 1 AS empty WHERE false");
    let directory = tempfile::tempdir().unwrap();
    let empty = directory.path().join("empty.csv");
    app.run_export(cx);
    save_export(&app, cx, &empty);
    assert_eq!(std::fs::read_to_string(empty).unwrap(), "empty\r\n");
    app.wait_status(cx, "Preview: Export download complete");
    app.type_sql(cx, "USE tpch.sf1");
    app.run_export(cx);
    let output = directory.path().join("preserved.csv");
    std::fs::write(&output, "previous output").unwrap();
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    app.wait_gone(cx, "export-save");
    app.wait_label(
        cx,
        "preserved.csv: The statement completed without a result set. No export file was written.",
    );
    assert_eq!(std::fs::read_to_string(output).unwrap(), "previous output");
    app.run_complete(cx, "SELECT current_schema AS value");
    app.wait_cell(cx, 0, 1, "sf1");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 3);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn csv_export_preserves_trino_decimal_text_and_nulls(cx: &mut TestAppContext) {
    let app = launch(
        cx,
        "SELECT 'a,b' AS value, CAST(NULL AS VARCHAR) AS absent, '' AS empty, DECIMAL '12345678901234567890.12345' AS amount UNION ALL SELECT 'NULL', 'NULL', '\\N', DECIMAL '-0.01000' ORDER BY amount DESC",
    );
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.click(cx, "export-results");
    app.select(cx, "export-null", "\\N");
    let text = super::export::copy_query(cx, &app);
    super::export::assert_reader_rows(
        &text,
        "\\N",
        serde_json::json!([
            ["a,b", null, "", "12345678901234567890.12345"],
            ["NULL", "NULL", "\\N", "-0.01000"]
        ]),
    );
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn markdown_json_export_normalizes_real_trino_values(cx: &mut TestAppContext) {
    let app = launch(
        cx,
        "SELECT 'a|b' AS text, CAST(NULL AS VARCHAR) AS absent, '' AS empty, DECIMAL '123.45' AS amount, BIGINT '9223372036854775807' AS id, TRUE AS ok, from_hex('005cff') AS bytes, ARRAY[1,2] AS items, DATE '2026-10-09' AS day, TIMESTAMP '2026-10-09 01:02:03.123456' AS moment",
    );
    super::export::assert_markdown_json(cx, &app);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn query_results_page_and_recover_after_a_sql_error(cx: &mut TestAppContext) {
    let app = launch(
        cx,
        "SELECT i AS value FROM UNNEST(sequence(1,1001)) AS t(i)",
    );
    app.wait_label(cx, "Trino database");
    app.context_menu(cx, connection_row(app.saved().profiles[0].id));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_label(cx, "Initial Catalog");
    app.wait_label(cx, "Initial Schema");
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "connection-name");
    app.click(cx, "run");
    app.wait_status(cx, "Preview: More rows available");
    app.update(cx, |window, _| {
        assert_eq!(header(window, 1).as_deref(), Some("value"));
        assert_eq!(cell(window, 0, 1).as_deref(), Some("1"));
    });
    app.click(cx, "next-page");
    app.wait_cell(cx, 0, 1, "1001");
    app.run_sql(cx, "SELECT missing_column");
    app.wait_status(cx, "Error: Query failed");
    app.run_complete(cx, "SELECT 42 AS value;");
    app.wait_cell(cx, 0, 1, "42");
    app.run_complete(cx, "USE tpch.sf1");
    app.run_complete(cx, "SELECT current_schema AS value");
    app.wait_cell(cx, 0, 1, "sf1");
    app.click(cx, "disconnect");
    app.wait_status(cx, "Disconnected");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn cancel_a_query_and_reuse_its_tab(cx: &mut TestAppContext) {
    let app = launch(cx, "SELECT count(*) FROM tpch.sf1000.lineitem");
    app.click(cx, "run");
    app.wait_status(cx, "Executing");
    app.click(cx, "cancel");
    app.wait_status(cx, "Cancelled");
    app.run_complete(cx, "SELECT 42 AS value");
    app.wait_cell(cx, 0, 1, "42");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn schema_browser_reads_the_initial_catalog_and_copies_three_part_names(cx: &mut TestAppContext) {
    use crate::support::labelled;
    use std::time::Duration;
    let app = launch(cx, "SELECT 1");
    let id = app.saved().profiles[0].id;
    app.context_menu(cx, connection_row(id));
    app.choose(cx, "popup-menu", "Edit");
    app.connection_page(cx, "Catalog");
    app.scroll_to(cx, "connection-schema-refresh");
    app.select(cx, "connection-schema-refresh", "Manual");
    app.scroll_to(cx, "connection-show-schemas");
    app.fill(cx, "connection-show-schemas", "tiny");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.context_menu(cx, connection_row(id));
    app.choose(cx, "popup-menu", "Refresh");
    app.toggle_connection(cx, id);
    app.wait_until(
        cx,
        "the Trino schema",
        Duration::from_secs(30),
        |window, _| labelled(window, "tiny").is_some(),
    );
    app.click_labelled(cx, "tiny");
    app.wait_until(
        cx,
        "the nation table",
        Duration::from_secs(30),
        |window, _| labelled(window, "nation").is_some(),
    );
    app.context_menu_labelled(cx, "nation");
    app.choose(cx, "popup-menu", "Copy qualified name");
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().unwrap(),
        "\"tpch\".\"tiny\".\"nation\""
    );
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn byte_limit_keeps_the_last_retained_batch_in_the_results_table(cx: &mut TestAppContext) {
    // Short values reach the byte budget through row storage with less transfer.
    let columns = 4000;
    let projection = (0..columns)
        .map(|index| format!("'x' AS c{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let app = launch(
        cx,
        &format!("SELECT {projection} FROM UNNEST(sequence(1,680)) t(i)"),
    );
    app.click(cx, "run");
    app.wait_until(
        cx,
        "the 64 MiB preview",
        std::time::Duration::from_secs(120),
        |_, cx| app.status(cx) == "Preview: Limit reached",
    );
    app.update(cx, |window, _| {
        let expected =
            qrow::model::MAX_RESULT_BYTES / (columns * (1 + std::mem::size_of::<Option<String>>()));
        assert_eq!(
            crate::support::label(window, "result-loaded").as_deref(),
            Some(format!("{expected} loaded").as_str())
        );
        assert_eq!(cell(window, 0, 1).as_deref(), Some("x"));
    });
}

#[path = "../support/trino_oidc.rs"]
mod trino_oidc;
#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn external_browser_sign_in_progress_query_and_memory_reuse(cx: &mut TestAppContext) {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };
    let (profile, trust) = trino_oidc::configuration();
    let opens = Arc::new(AtomicUsize::new(0));
    let browser = trino_oidc::browser(trust.clone(), opens.clone());
    let release = Arc::new(AtomicBool::new(false));
    let browser_release = release.clone();
    let entered = Arc::new(AtomicBool::new(false));
    let browser_entered = entered.clone();
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = "SELECT current_user AS value".into();
    let other = crate::support::offline_profile("Other connection");
    let app = TestApp::launch_with_sign_ins(
        cx,
        Workspace {
            tabs: vec![tab, SavedTab::new(1, Some(other.id))],
            profiles: vec![profile, other],
            active_tab: 1,
            ..Workspace::default()
        },
        MemoryCredentials::default(),
        SignIns::new(
            trust,
            Some(Arc::new(move |url| {
                browser_entered.store(true, Ordering::SeqCst);
                let deadline = Instant::now() + Duration::from_secs(15);
                while !browser_release.load(Ordering::SeqCst) {
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "Fixture browser was not released"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                browser(url)
            })),
        ),
    );
    let connection = app.saved().profiles[0].id;
    app.click(cx, format!("profile-{connection}"));
    let idle_until = Instant::now() + Duration::from_millis(250);
    app.wait_until(
        cx,
        "idle Trino selection",
        Duration::from_secs(5),
        |window, _| {
            assert!(!entered.load(Ordering::SeqCst));
            assert!(crate::support::present(window, &"run".into()));
            assert!(!crate::support::present(
                window,
                &"connection-authentication-progress".into()
            ));
            Instant::now() >= idle_until
        },
    );
    app.click(cx, "run");
    app.wait_status(cx, "Waiting for browser sign-in");
    app.wait_for(cx, "connection-authentication-progress");
    app.wait_for(cx, "cancel");
    release.store(true, Ordering::SeqCst);
    app.wait_gone(cx, "connection-authentication-progress");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "alice");
    app.click(cx, "disconnect");
    app.wait_status(cx, "Disconnected");
    app.run_complete(cx, "SELECT 42 AS value");
    app.wait_cell(cx, 0, 1, "42");
    assert_eq!(opens.load(Ordering::SeqCst), 1);
    assert!(app.saved().sign_ins.is_empty());
    assert!(
        !serde_json::to_string(&app.saved())
            .unwrap()
            .contains("Bearer")
    );
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn parquet_picosecond_error_preserves_the_file_and_text_retry_uses_the_result(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestWindowExt;
    use parquet::{
        file::reader::{FileReader, SerializedFileReader},
        record::RowAccessor,
    };
    let text = "2026-10-09 01:02:03.123456789123";
    let app = launch(
        cx,
        "SELECT TIMESTAMP '2026-10-09 01:02:03.123456789123' AS moment",
    );
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, text);
    app.click(cx, "export-results");
    app.select(cx, "export-format", "Parquet");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("picoseconds.parquet");
    std::fs::write(&path, "original").unwrap();
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    app.wait_label(
        cx,
        "Timestamp precision above 9 is not supported. Choose Text column types.",
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    app.update(cx, |window, _| {
        assert!(window.try_find("export-save").is_some())
    });
    app.select(cx, "export-parquet-types", "Text");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    app.wait_gone(cx, "export-save");
    let reader = SerializedFileReader::new(std::fs::File::open(&path).unwrap()).unwrap();
    assert_eq!(
        reader
            .get_row_iter(None)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .get_string(0)
            .unwrap(),
        text
    );
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn parallel_spooling_saves_all_rows_and_keeps_normal_queries_usable(cx: &mut TestAppContext) {
    let app = launch_with_preset(
        cx,
        "SELECT orderkey AS id, lpad(CAST(orderkey AS varchar),10,'0') || rpad('',1014,'x') AS payload FROM tpch.sf1.orders WHERE orderkey <=200000 ORDER BY orderkey LIMIT 50000",
        qrow::model::transfer::TransferPreset::Fast,
    );
    app.run_export(cx);
    app.wait_for(cx, "export-incremental-notice");
    app.update(cx, |window, _| {
        assert!(
            window
                .find("export-incremental-notice")
                .label()
                .unwrap()
                .contains("Trino spooling: Parallel")
        )
    });
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("parallel.csv");
    save_export(&app, cx, &output);
    let mut count = 0;
    let mut previous = 0;
    for row in csv::Reader::from_path(output).unwrap().records() {
        let row = row.unwrap();
        assert_eq!(row.len(), 2);
        let id = row[0].parse::<u64>().unwrap();
        assert!(id > previous);
        previous = id;
        assert_eq!(&row[1], format!("{id:010}{}", "x".repeat(1014)));
        count += 1;
    }
    assert_eq!(count, 50000);
    app.wait_status(cx, "Preview: Export download complete");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
    app.run_complete(cx, "SELECT 42 AS value");
    app.wait_cell(cx, 0, 1, "42");
}
