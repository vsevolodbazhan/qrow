use crate::support::{MemoryCredentials, TestApp, cell, connection_row, header};
use gpui_kit::TestAppContext;
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
