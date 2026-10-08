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
fn reordering_connections_preserves_a_live_session_and_its_results(cx: &mut TestAppContext) {
    let (workspace, credentials) =
        Kyuubi::get().connections(&["Alpha", "Beta", "Gamma"], "SELECT 42 AS value");
    let alpha = workspace.profiles[0].id;
    let gamma = workspace.profiles[2].id;
    let query = workspace.tabs[0].id;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.run_complete(cx, "SELECT 42 AS value");
    let reads = app.credentials.reads();
    app.update(cx, |window, cx| {
        window.drag_to(
            format!("profile-{gamma}"),
            format!("connection-drop-before-{alpha}"),
            cx,
        );
    });
    app.wait_until(cx, "the saved connection order", QUERY_TIMEOUT, |_, _| {
        app.saved()
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .eq(["Gamma", "Alpha", "Beta"])
    });
    app.update(cx, |window, cx| {
        assert_eq!(cell(window, 0, 1).as_deref(), Some("42"));
        assert!(
            window.find(connection_row(gamma)).bounds().top()
                < window.find(connection_row(alpha)).bounds().top()
        );
        assert_tab_dot(window, query, Some(cx.theme().info.opacity(0.4)));
    });
    app.run_complete(cx, "SELECT 43 AS value");
    app.wait_cell(cx, 0, 1, "43");
    assert_eq!(
        app.credentials.reads(),
        reads,
        "Reordering must keep the live session"
    );
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn assistant_notes_save_in_connection_settings_without_interrupting_queries(
    cx: &mut TestAppContext,
) {
    let (mut workspace, credentials) = Kyuubi::get().workspace("SELECT 42 AS value", PASSWORD);
    let connection = workspace.profiles[0].id;
    workspace.settings.assistant.enabled = true;
    workspace.settings.assistant.data_sharing_notice_version =
        qrow::model::ASSISTANT_DATA_SHARING_NOTICE_VERSION;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.run_complete(cx, "SELECT 42 AS value");
    app.context_menu(cx, connection_row(connection));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    app.connection_page(cx, "Assistant");
    app.scroll_to(cx, "connection-assistant-notes");
    app.update(cx, |window, cx| {
        assert!(labels(window).iter().any(|text| text == "Assistant Notes"));
        window.click("connection-assistant-notes", cx);
        window.input("Dates are UTC.", cx);
    });
    app.click(cx, "save-profile");
    app.wait_gone(cx, "save-profile");
    app.wait_until(cx, "the saved notes", QUERY_TIMEOUT, |_, _| {
        app.saved().profiles[0].assistant_notes == "Dates are UTC."
    });
    app.run_complete(cx, "SELECT 43 AS value");
    app.wait_cell(cx, 0, 1, "43");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn refresh_errors_keep_connection_tooltips_short_and_details_in_activity(cx: &mut TestAppContext) {
    let (mut workspace, credentials) = Kyuubi::get().workspace("SELECT 1", "not-the-password");
    let connection = workspace.profiles[0].id;
    workspace.profiles[0].catalog.refresh = qrow::model::CatalogRefresh::Manual;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.context_menu(cx, connection_row(connection));
    app.choose(cx, "popup-menu", "Refresh");
    app.wait_until(
        cx,
        "the unread schema refresh error",
        QUERY_TIMEOUT,
        |window, _| {
            label(window, connection_row(connection)).as_deref()
                == Some("Spark, unread error, schema refresh error")
        },
    );
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
            Some("Unread error")
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
    // The session never opened, so the connection failed; it was not lost.
    app.wait_status(cx, "Error: Connection failed");
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
