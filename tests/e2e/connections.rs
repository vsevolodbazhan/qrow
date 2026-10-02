use crate::support::fixture::{Kyuubi, PASSWORD, QUERY_TIMEOUT};
use crate::support::{
    TestApp, assert_connection_dot, assert_tab_dot, cell, connection_row, label, labels,
};
use gpui_kit::TestAppContext;
use gpui_kit::component::ActiveTheme;
use gpui_kit::test::TestWindowExt;
use std::time::Duration;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn refresh_errors_keep_connection_tooltips_short_and_details_in_activity(cx: &mut TestAppContext) {
    let (mut workspace, credentials) = Kyuubi::get().workspace("SELECT 1", "not-the-password");
    let connection = workspace.profiles[0].id;
    workspace.profiles[0].catalog.refresh = qrow::model::CatalogRefresh::Manual;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.context_menu(cx, connection_row(connection));
    app.choose(cx, "popup-menu", "Refresh");
    app.wait_label(cx, "Spark, schema refresh error");
    app.update(cx, |window, cx| {
        window.hover(format!("connection-status-{connection}"), cx);
    });
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, cx| {
        assert_eq!(
            label(window, "status-tooltip-title").as_deref(),
            Some("Spark")
        );
        assert_eq!(
            label(window, "status-tooltip-status").as_deref(),
            Some("Unread Error")
        );
        assert_eq!(
            label(window, "status-tooltip-Host").as_deref(),
            Some("Host: 127.0.0.1")
        );
        assert_eq!(
            label(window, "status-tooltip-User").as_deref(),
            Some("User: qrow")
        );
        assert!(window.try_find("status-tooltip-error").is_none());
        assert!(
            labels(window)
                .iter()
                .all(|text| !text.contains("Activity shows the full error"))
        );
        assert_connection_dot(window, connection, cx.theme().danger);
    });
    app.click(cx, format!("connection-status-{connection}"));
    app.wait_for(cx, "activity");
    let activity = app.copy_activity(cx);
    assert!(activity.contains("Schema refresh failed"), "{activity}");
    assert!(activity.contains("authentication"), "{activity}");
    app.press(cx, "escape");
    app.wait_gone(cx, "activity");
    app.hover_labelled(cx, "Spark, schema refresh error");
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "status-tooltip-status").as_deref(),
            Some("Disconnected")
        );
        assert!(window.try_find("status-tooltip-error").is_none());
    });
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn connecting_dots_become_idle_after_the_query_completes(cx: &mut TestAppContext) {
    let (workspace, credentials) = Kyuubi::get().workspace("SELECT 42 AS value", PASSWORD);
    let connection = workspace.profiles[0].id;
    let query = workspace.tabs[0].id;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.update(cx, |window, cx| {
        window.click("run", cx);
        assert_tab_dot(window, query, Some(cx.theme().info.opacity(0.2)));
        assert_connection_dot(window, connection, cx.theme().info.opacity(0.2));
        assert_eq!(
            label(window, format!("query-status-{query}")).as_deref(),
            Some("Query 1, connecting")
        );
    });
    app.wait_cell(cx, 0, 1, "42");
    app.wait_status(cx, "Complete");
    app.update(cx, |window, cx| {
        assert_tab_dot(window, query, Some(cx.theme().info.opacity(0.4)));
        assert_connection_dot(window, connection, cx.theme().info.opacity(0.4));
        assert_eq!(
            label(window, format!("query-status-{query}")).as_deref(),
            Some("Query 1, connected, idle")
        );
    });
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn ldap_rejects_a_wrong_password(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (workspace, credentials) = kyuubi.workspace("SELECT 1", "not-the-password");
    let row = gpui_kit::ElementId::Name(format!("profile-{}", workspace.profiles[0].id).into());
    let app = TestApp::launch_with(cx, workspace, credentials);

    app.update(cx, |window, cx| window.click("run", cx));
    app.wait_until(cx, "the rejected sign-in", QUERY_TIMEOUT, |window, _| {
        window.find(row.clone()).label() == Some("Spark")
    });
    app.wait_status(cx, "Error: Connection lost");
    app.update(cx, |window, _| assert_eq!(cell(window, 0, 1), None));
    assert_eq!(app.credentials.reads(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_missing_initial_database_is_named_in_the_status_and_logs(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT 1", crate::support::fixture::PASSWORD);
    workspace.profiles[0].database = "qrow_missing_database".into();
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.update(cx, |window, cx| window.click("run", cx));
    app.wait_status(cx, "Error: Connection failed");
    let logs = app.logs(cx);
    assert!(
        logs.contains("Could not select the initial database \"qrow_missing_database\"."),
        "{logs}"
    );
    assert!(logs.contains("SCHEMA_NOT_FOUND"), "{logs}");
}
