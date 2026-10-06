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
fn query_results_page_and_recover_after_a_sql_error(cx: &mut TestAppContext) {
    let (workspace, credentials) = workspace("SELECT i AS value FROM generate_series(1,1001) i");
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.wait_label(cx, "Postgres database");
    app.context_menu(cx, connection_row(app.saved().profiles[0].id));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_label(cx, "Connection type");
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
