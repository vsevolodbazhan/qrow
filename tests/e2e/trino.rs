use crate::support::{MemoryCredentials, SignIns, TestApp, cell, connection_row, header};
use gpui_kit::TestAppContext;
use qrow::{
    model::{DatabaseType, Profile, SavedTab, Workspace},
    storage::Credentials,
    tls::Trust,
};

fn launch(cx: &mut TestAppContext, sql: &str) -> TestApp {
    let fixture = std::env::var("QROW_TRINO_FIXTURE").expect("Run with ./qtest run trino");
    assert!(fixture.starts_with("qrow-e2e-trino-"));
    let profile = Profile {
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
    let app = launch(
        cx,
        "SELECT concat(repeat('x',99990), lpad(CAST(i AS varchar),10,'0')) AS value FROM UNNEST(sequence(1,700)) t(i)",
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
            qrow::model::MAX_RESULT_BYTES / (100000 + std::mem::size_of::<Option<String>>());
        assert_eq!(
            crate::support::label(window, "result-loaded").as_deref(),
            Some(format!("{expected} loaded").as_str())
        );
        assert!(cell(window, 0, 1).unwrap().starts_with("xxx"));
    });
}
