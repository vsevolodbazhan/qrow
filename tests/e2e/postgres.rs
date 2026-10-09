use crate::support::{MemoryCredentials, TestApp, cell, connection_row, header};
use gpui_kit::{TestAppContext, test::TestWindowExt};
use qrow::{
    model::{DatabaseType, PostgresSslMode, Profile, SavedTab, Workspace},
    storage::Credentials,
};

fn workspace(sql: &str) -> (Workspace, MemoryCredentials) {
    let fixture = std::env::var("QROW_POSTGRES_FIXTURE").expect("Run with ./qtest run postgres");
    assert!(fixture.starts_with("qrow-e2e-postgres-"));
    let profile = Profile {
        database_type: DatabaseType::Postgres,
        postgres_ssl_mode: Some(PostgresSslMode::Require),
        name: "Postgres".into(),
        host: "127.0.0.1".into(),
        port: std::env::var("QROW_POSTGRES_PORT")
            .unwrap()
            .parse()
            .unwrap(),
        username: "qrow".into(),
        database: "qrow".into(),
        ..Profile::default()
    };
    let credentials = MemoryCredentials::default();
    credentials
        .set_password(profile.id, "qrow-test-password")
        .unwrap();
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = sql.into();
    (
        Workspace {
            profiles: vec![profile],
            tabs: vec![tab],
            ..Workspace::default()
        },
        credentials,
    )
}

fn save_export(app: &TestApp, cx: &mut TestAppContext, path: &std::path::Path) {
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.to_owned()));
    app.wait_gone(cx, "export-save");
    app.wait_until(
        cx,
        "the published export",
        std::time::Duration::from_secs(30),
        |_, _| path.is_file(),
    );
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn run_export_keeps_temp_tables_uncommitted_rows_and_exact_values(cx: &mut TestAppContext) {
    let (mut workspace, credentials) = workspace("SELECT 1");
    workspace.profiles[0]
        .parameters
        .insert("DateStyle".into(), "SQL, DMY".into());
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.run_complete(cx, "CREATE TEMP TABLE qrow_export_state AS SELECT i, 123456789012345.12345::numeric(20,5) AS amount FROM generate_series(1,100123) i");
    app.run_complete(cx, "BEGIN");
    app.run_complete(
        cx,
        "INSERT INTO qrow_export_state VALUES (999999, -0.00001)",
    );
    app.type_sql(
        cx,
        "SELECT i, amount, DATE '2026-10-09' AS day FROM qrow_export_state ORDER BY i",
    );
    app.run_export(cx);
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("all.csv");
    save_export(&app, cx, &output);
    let mut count = 0;
    for (index, row) in csv::Reader::from_path(&output)
        .unwrap()
        .records()
        .enumerate()
    {
        let row = row.unwrap();
        assert_eq!(
            &row[0],
            if index == 100123 {
                "999999".into()
            } else {
                (index + 1).to_string()
            }
        );
        assert_eq!(
            &row[1],
            if index == 100123 {
                "-0.00001"
            } else {
                "123456789012345.12345"
            }
        );
        assert_eq!(&row[2], "09/10/2026");
        count += 1;
    }
    assert_eq!(count, 100124);
    app.wait_status(cx, "Preview: Export download complete");
    app.wait_cell(cx, 0, 1, "1");
    app.update(cx, |window, _| {
        assert_eq!(window.find("result-loaded").label(), Some("1000 loaded"))
    });
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 4);
    assert_eq!(app.logs(cx).matches("Connected to").count(), 1);
    // All rows reuses the completed spool and its captured DateStyle.
    app.click(cx, "export-results");
    app.select(cx, "export-rows", "All rows");
    app.select(cx, "export-format", "Parquet");
    app.wait_for(cx, "export-date-style-warning");
    let replay = directory.path().join("replay.parquet");
    save_export(&app, cx, &replay);
    use parquet::{
        file::reader::{FileReader, SerializedFileReader},
        record::RowAccessor,
    };
    let reader = SerializedFileReader::new(std::fs::File::open(replay).unwrap()).unwrap();
    assert_eq!(reader.metadata().file_metadata().num_rows(), 100124);
    let row = reader.get_row_iter(None).unwrap().next().unwrap().unwrap();
    assert_eq!(row.get_string(2).unwrap(), "09/10/2026");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 4);
    app.run_complete(cx, "ROLLBACK");
    app.run_complete(cx, "SELECT count(*) AS value FROM qrow_export_state");
    app.wait_cell(cx, 0, 1, "100123");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn run_export_exceeds_the_preview_byte_limit(cx: &mut TestAppContext) {
    let (workspace, credentials) =
        workspace("SELECT i, repeat('x',80000) AS wide FROM generate_series(1,2000) i");
    let app = TestApp::launch_with(cx, workspace, credentials);
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
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn run_again_uses_result_sql_and_warns_after_disconnect(cx: &mut TestAppContext) {
    let sql = "SELECT i AS value FROM generate_series(1,2001) i";
    let (workspace, credentials) = workspace(sql);
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
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn completed_spool_retry_survives_new_sql_and_tab_close(cx: &mut TestAppContext) {
    let (workspace, credentials) =
        workspace("SELECT i AS value, '未知😀' AS text FROM generate_series(1,2001) i");
    let app = TestApp::launch_with(cx, workspace, credentials);
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
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn empty_export_writes_a_header_and_a_statement_without_results_preserves_output(
    cx: &mut TestAppContext,
) {
    let (workspace, credentials) = workspace("SELECT 1 AS empty WHERE false");
    let app = TestApp::launch_with(cx, workspace, credentials);
    let directory = tempfile::tempdir().unwrap();
    let empty = directory.path().join("empty.csv");
    app.run_export(cx);
    save_export(&app, cx, &empty);
    assert_eq!(std::fs::read_to_string(empty).unwrap(), "empty\r\n");
    app.wait_status(cx, "Preview: Export download complete");
    app.type_sql(cx, "CREATE TEMP TABLE qrow_no_export (i int)");
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
    app.run_complete(cx, "SELECT count(*) AS value FROM qrow_no_export");
    app.wait_cell(cx, 0, 1, "0");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 3);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn csv_export_preserves_postgres_decimal_text_and_nulls(cx: &mut TestAppContext) {
    let (workspace, credentials) = workspace(
        "SELECT 'a,b' AS value, NULL::text AS absent, ''::text AS empty, 12345678901234567890.12345::numeric AS amount UNION ALL SELECT 'NULL', 'NULL', E'\\\\N', -0.01::numeric",
    );
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.click(cx, "export-results");
    app.select(cx, "export-null", "NULL");
    let text = super::export::copy_query(cx, &app);
    super::export::assert_reader_rows(
        &text,
        "NULL",
        serde_json::json!([
            ["a,b", null, "", "12345678901234567890.12345"],
            ["NULL", "NULL", "\\N", "-0.01"]
        ]),
    );
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn markdown_json_export_normalizes_real_postgres_values(cx: &mut TestAppContext) {
    let (workspace, credentials) = workspace(
        "SELECT 'a|b' AS text, NULL::text AS absent, ''::text AS empty, 123.45::numeric AS amount, 9223372036854775807::bigint AS id, TRUE AS ok, decode('005cff','hex') AS bytes, '[\n1,\r\n2\n]'::json AS items, DATE '2026-10-09' AS day, TIMESTAMP '2026-10-09 01:02:03.123456' AS moment",
    );
    let app = TestApp::launch_with(cx, workspace, credentials);
    super::export::assert_markdown_json(cx, &app);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn query_results_page_and_recover_after_a_sql_error(cx: &mut TestAppContext) {
    let (workspace, credentials) = workspace("SELECT i AS value FROM generate_series(1,1001) i");
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.wait_label(cx, "Postgres database");
    app.context_menu(cx, connection_row(app.saved().profiles[0].id));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_label(cx, "Connection Type");
    app.wait_label(cx, "TLS Mode");
    app.wait_label(cx, "The database account used to connect.");
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
    app.run_complete(
        cx,
        "SELECT ssl AS value FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
    );
    app.wait_cell(cx, 0, 1, "t");
    app.run_complete(cx, "SELECT $$a;b$$ AS value");
    app.wait_cell(cx, 0, 1, "a;b");
    app.click(cx, "disconnect");
    app.wait_status(cx, "Disconnected");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn cancel_a_query_and_reuse_its_tab(cx: &mut TestAppContext) {
    let (workspace, credentials) = workspace("SELECT pg_sleep(30)");
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Executing");
    app.click(cx, "cancel");
    app.wait_status(cx, "Cancelled");
    app.run_complete(cx, "SELECT 42 AS value");
    app.wait_cell(cx, 0, 1, "42");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn the_first_page_is_visible_while_the_server_tail_is_blocked(cx: &mut TestAppContext) {
    use qrow::connector::{Connector, Secret, postgres::PostgresConnector, wait_for_completion};
    let key = (uuid::Uuid::new_v4().as_u128() as u64) & i64::MAX as u64;
    let sql =
        format!("SELECT i AS value, pg_temp.export_tail(i, {key}) FROM generate_series(1, 1001) i");
    let (workspace, credentials) = workspace(&sql);
    let mut observer = PostgresConnector::default()
        .connect(
            &workspace.profiles[0],
            Secret::password("qrow-test-password"),
        )
        .unwrap();
    observer
        .execute(&format!("SELECT pg_advisory_lock({key})"))
        .unwrap();
    wait_for_completion(&mut *observer, None).unwrap();
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.run_complete(cx, "CREATE FUNCTION pg_temp.export_tail(i int, key bigint) RETURNS bool LANGUAGE plpgsql AS $$ BEGIN IF i = 1001 THEN RAISE NOTICE 'flush first page'; PERFORM pg_advisory_lock(key); END IF; RETURN true; END $$");
    app.run_sql(cx, &sql);
    app.wait_cell(cx, 0, 1, "1");
    app.update(cx, |window, _| {
        assert!(window.try_find("cancel").is_some());
        assert!(window.try_find("run").is_none());
    });
    observer
        .execute(&format!("SELECT pg_advisory_unlock({key})"))
        .unwrap();
    wait_for_completion(&mut *observer, None).unwrap();
    app.wait_status(cx, "Preview: More rows available");
    app.click(cx, "next-page");
    app.wait_cell(cx, 0, 1, "1001");
    app.wait_status(cx, "Complete");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 2);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn parquet_non_iso_dates_remain_text_and_keep_the_session_setting(cx: &mut TestAppContext) {
    use parquet::{
        file::reader::{FileReader, SerializedFileReader},
        record::RowAccessor,
    };
    let (mut workspace, credentials) = workspace(
        "SELECT DATE '2026-10-09' AS day, TIMESTAMP '2026-10-09 01:02:03.123456' AS moment",
    );
    workspace.profiles[0]
        .parameters
        .insert("DateStyle".into(), "SQL, DMY".into());
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "09/10/2026");
    app.click(cx, "export-results");
    app.select(cx, "export-format", "Parquet");
    app.wait_label(
        cx,
        "The result uses a non-ISO DateStyle. Dates and timestamps remain text.",
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dates.parquet");
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    app.wait_gone(cx, "export-save");
    let reader = SerializedFileReader::new(std::fs::File::open(&path).unwrap()).unwrap();
    let row = reader.get_row_iter(None).unwrap().next().unwrap().unwrap();
    assert_eq!(row.get_string(0).unwrap(), "09/10/2026");
    assert_eq!(row.get_string(1).unwrap(), "09/10/2026 01:02:03.123456");
    assert_eq!(app.logs(cx).matches("Submitted query:").count(), 1);
    app.run_complete(cx, "SELECT current_setting('DateStyle') AS style");
    app.wait_cell(cx, 0, 1, "SQL, DMY");
}
