use crate::support::fixture::{Kyuubi, PASSWORD, QUERY_TIMEOUT};
use crate::support::{TestApp, assert_connection_dot, assert_tab_dot, cell, label};
use gpui_kit::TestAppContext;
use gpui_kit::component::ActiveTheme;
use gpui_kit::test::TestWindowExt;

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
    app.wait_status(cx, "Error · Connection lost");
    app.update(cx, |window, _| assert_eq!(cell(window, 0, 1), None));
    assert_eq!(app.credentials.reads(), 1);
}
