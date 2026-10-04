//! Each tab keeps its own session and results while you work in others.
use crate::support::fixture::{Kyuubi, QUERY_TIMEOUT};
use crate::support::{TestApp, cell, connection_row, label, labelled};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::model::Profile;

fn launch(cx: &mut TestAppContext, names: &[&str], sql: &str) -> (TestApp, Vec<Profile>) {
    let (workspace, credentials) = Kyuubi::get().connections(names, sql);
    let profiles = workspace.profiles.clone();
    (TestApp::launch_with(cx, workspace, credentials), profiles)
}

fn tab_sql(app: &TestApp, profile: &Profile, title: &str) -> Option<String> {
    app.saved()
        .tabs
        .into_iter()
        .find(|tab| tab.profile == Some(profile.id) && tab.title == title)
        .map(|tab| tab.sql)
}

fn edit(app: &TestApp, cx: &mut TestAppContext, profile: &Profile) {
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
}

/// Chooses the idle behavior of the open connection form. The field is below
/// the fold of the form.
fn idle_behavior(app: &TestApp, cx: &mut TestAppContext, behavior: &str) {
    app.scroll_to(cx, "connection-idle-behavior");
    app.select(cx, "connection-idle-behavior", behavior);
}

fn wait_tab_sql(app: &TestApp, cx: &mut TestAppContext, profile: &Profile, title: &str, sql: &str) {
    app.wait_until(
        cx,
        &format!("{title} with {sql:?}"),
        QUERY_TIMEOUT,
        |_, _| tab_sql(app, profile, title).as_deref() == Some(sql),
    );
}

fn save_form(app: &TestApp, cx: &mut TestAppContext) {
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn copy_and_move_carry_the_sql_but_not_the_results(cx: &mut TestAppContext) {
    let sql = "SELECT 'connected-result' AS result";
    let (app, profiles) = launch(cx, &["Alpha", "Beta"], sql);
    let (alpha, beta) = (&profiles[0], &profiles[1]);
    app.run_complete(cx, sql);
    app.wait_cell(cx, 0, 1, "connected-result");

    app.context_menu_labelled(cx, "Query 1");
    app.choose_in_submenu(cx, "Copy to Connection…", "Beta");
    app.select_connection(cx, beta);
    app.wait_until(cx, "the copy", QUERY_TIMEOUT, |window, _| {
        labelled(window, "Query 1 (Copy)").is_some()
    });
    wait_tab_sql(&app, cx, beta, "Query 1 (Copy)", sql);
    app.update(cx, |window, _| {
        assert_eq!(cell(window, 0, 1), None, "Copy carried the results")
    });

    // The source keeps its rows, and selecting a connection adds no log entry.
    app.select_connection(cx, alpha);
    app.wait_cell(cx, 0, 1, "connected-result");
    assert!(!app.logs(cx).contains("Selected connection"));

    app.context_menu_labelled(cx, "Query 1");
    app.choose_in_submenu(cx, "Move to Connection…", "Beta");
    app.select_connection(cx, beta);
    app.wait_until(cx, "the moved tab", QUERY_TIMEOUT, |window, _| {
        labelled(window, "Query 1 (Copy 2)").is_some()
    });
    wait_tab_sql(&app, cx, beta, "Query 1 (Copy 2)", sql);
    app.update(cx, |window, _| {
        assert_eq!(cell(window, 0, 1), None, "Move carried the results")
    });
    // The source gets a new blank tab without the rows.
    app.select_connection(cx, alpha);
    wait_tab_sql(&app, cx, alpha, "Query 1", "");
    app.update(cx, |window, _| {
        assert_eq!(cell(window, 0, 1), None, "Move left the rows")
    });
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn disconnect_and_connection_edits_act_on_their_own_session(cx: &mut TestAppContext) {
    let (app, profiles) = launch(cx, &["Alpha", "Beta"], "");
    let (alpha, beta) = (&profiles[0], &profiles[1]);

    // Beta keeps a result cursor with more rows. It uses the server time zone.
    app.select_connection(cx, beta);
    app.run_sql(
        cx,
        "SELECT concat('b-', lpad(CAST(id AS STRING), 4, '0'), '-', current_timezone()) AS value FROM range(4001) ORDER BY id",
    );
    app.wait_status(cx, "Preview: More rows available");
    app.wait_cell(cx, 0, 1, "b-0000-UTC");

    // Disconnect acts on the active tab only.
    app.select_connection(cx, alpha);
    app.run_complete(cx, "SELECT 'a' AS value");
    app.click(cx, "disconnect");
    app.wait_status(cx, "Disconnected");
    app.select_connection(cx, beta);
    app.click(cx, "next-page");
    app.wait_cell(cx, 0, 1, "b-1000-UTC");

    // A lifecycle edit of Alpha while Beta is selected keeps Beta's session.
    edit(&app, cx, alpha);
    idle_behavior(&app, cx, "Keep connected");
    save_form(&app, cx);
    app.click(cx, "next-page");
    app.wait_cell(cx, 0, 1, "b-2000-UTC");

    // A lifecycle edit of Beta updates its live session.
    app.select_connection(cx, alpha);
    edit(&app, cx, beta);
    idle_behavior(&app, cx, "Keep connected");
    app.scroll_to(cx, "connection-keep-alive-query");
    app.fill(cx, "connection-keep-alive-interval", "3");
    app.fill(cx, "connection-keep-alive-query", "SELECT 'updated-b'");
    save_form(&app, cx);
    app.select_connection(cx, beta);
    app.wait_status(cx, "Connected: Keep-alive enabled");
    app.wait_cell(cx, 0, 1, "b-2000-UTC");
    app.click(cx, "next-page");
    app.wait_cell(cx, 0, 1, "b-3000-UTC");

    // A new password closes Beta's session, also while Alpha is selected.
    app.select_connection(cx, alpha);
    edit(&app, cx, beta);
    app.fill(cx, "connection-password", crate::support::fixture::PASSWORD);
    idle_behavior(&app, cx, "Disconnect after");
    save_form(&app, cx);
    app.select_connection(cx, beta);
    app.wait_status(cx, "Not connected");
    app.wait_cell(cx, 0, 1, "b-3000-UTC");
    app.run_complete(cx, "SELECT 'b-reconnected' AS value");
    app.wait_cell(cx, 0, 1, "b-reconnected");

    // Deleting a connection closes its session.
    app.select_connection(cx, alpha);
    app.context_menu(cx, connection_row(beta.id));
    app.choose(cx, "popup-menu", "Delete");
    app.click(cx, "confirm-delete-connection");
    app.wait_gone(cx, connection_row(beta.id));
    assert!(app.logs(cx).contains("Disconnected"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_metadata_edit_keeps_the_sessions_of_its_tabs(cx: &mut TestAppContext) {
    let (app, profiles) = launch(cx, &["Alpha"], "");
    let alpha = &profiles[0];
    let view = "CREATE TEMPORARY VIEW qrow_live AS SELECT 'preserved' AS value";
    app.run_complete(cx, view);
    app.click(cx, "new-tab");
    app.wait_until(cx, "the second tab", QUERY_TIMEOUT, |window, _| {
        labelled(window, "Query 2").is_some()
    });
    app.run_complete(cx, view);

    app.click_labelled(cx, "Query 1");
    edit(&app, cx, alpha);
    app.fill(cx, "connection-name", "Alpha live");
    save_form(&app, cx);
    app.wait_until(cx, "the new name", QUERY_TIMEOUT, |window, _| {
        label(window, connection_row(alpha.id)).as_deref() == Some("Alpha live")
    });
    // Temporary views prove that the sessions did not reconnect.
    for tab in ["Query 1", "Query 2"] {
        app.click_labelled(cx, tab);
        app.run_complete(cx, "SELECT * FROM qrow_live");
        app.wait_cell(cx, 0, 1, "preserved");
    }
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn pages_move_through_a_long_result(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, &["Alpha"], "");
    app.run_sql(cx, "SELECT concat('row-', lpad(CAST(id AS STRING), 4, '0')) AS value FROM range(1001) ORDER BY id");
    app.wait_status(cx, "Preview: More rows available");
    app.wait_cell(cx, 0, 1, "row-0000");
    app.click(cx, "next-page");
    app.wait_cell(cx, 0, 1, "row-1000");
    app.update(cx, |window, _| {
        assert_eq!(label(window, "page-label").as_deref(), Some("Page 2"))
    });
    app.click(cx, "previous-page");
    app.wait_cell(cx, 0, 1, "row-0000");
    app.update(cx, |window, _| {
        assert_eq!(label(window, "page-label").as_deref(), Some("Page 1"))
    });
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn only_the_selected_statement_runs(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, &["Alpha"], "");
    // A bad first statement proves that only the selection reaches Spark.
    app.click(cx, "sql-editor");
    app.press(cx, "cmd-a");
    app.update(cx, |window, cx| window.input("invalid prefix;", cx));
    app.press(cx, "enter");
    app.update(cx, |window, cx| {
        window.input("SELECT '日本語😀' AS selected_value", cx)
    });
    // Select the second line, which has multi-byte text.
    app.press(cx, "cmd-shift-left");
    app.press(cx, "cmd-enter");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "日本語😀");
}
