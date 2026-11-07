use crate::support::fixture::{Kyuubi, PASSWORD, QUERY_TIMEOUT};
use crate::support::{
    TestApp, assert_workspace_divider_alignment, assert_workspace_header_alignment, bounds_of,
    cell, header, label,
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ScrollDelta, TestAppContext, point, px, size};
use qrow::model::SavedTab;
use std::time::Duration;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn run_executes_the_cursor_statement_unless_text_is_selected(cx: &mut TestAppContext) {
    let sql = "SELECT '最初😀;' AS value;\n\nSELECT\n  'middle😀;' AS value;\n\n-- between queries;\nSELECT 3 AS value;\n";
    let (workspace, credentials) = Kyuubi::get().workspace(sql, PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "sql-editor");
    app.press(cx, "secondary-a");
    app.press(cx, "left");
    for _ in 0..3 {
        app.press(cx, "down");
    }
    app.press(cx, "secondary-enter");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "middle😀;");
    assert_eq!(app.saved().tabs[0].sql, sql);

    // Run uses the same cursor position after the editor loses focus.
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "middle😀;");

    // A blank line or standalone comment must not submit SQL.
    let submissions = app.logs(cx).matches("Submitted query:").count();
    for line in [4, 5, 7] {
        app.click(cx, "sql-editor");
        app.press(cx, "secondary-a");
        app.press(cx, "left");
        for _ in 0..line {
            app.press(cx, "down");
        }
        app.press(cx, "secondary-enter");
        app.wait_status(cx, "Rejected");
        let logs = app.logs(cx);
        assert!(logs.contains("Move the cursor into a SQL statement or select SQL to run."));
        assert_eq!(logs.matches("Submitted query:").count(), submissions);
    }

    // Select the first line through the editor, including Unicode text.
    app.click(cx, "sql-editor");
    app.press(cx, "secondary-a");
    app.press(cx, "left");
    app.press(cx, "shift-end");
    app.press(cx, "secondary-enter");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "最初😀;");
    assert_eq!(app.saved().tabs[0].sql, sql);

    // An unterminated statement still runs when the cursor is inside it.
    let sql = "invalid earlier SQL;\nSELECT 3 AS value\n-- trailing ;\n";
    app.type_sql(cx, sql);
    app.press(cx, "secondary-a");
    app.press(cx, "left");
    app.press(cx, "down");
    app.press(cx, "right");
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "3");
    let logs = app.logs(cx);
    assert!(logs.contains("SELECT 3 AS value"));
    assert!(!logs.contains("invalid earlier SQL"));
    assert_eq!(app.saved().tabs[0].sql, sql);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn query_metadata_separates_status_detail_duration_and_counts(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (mut workspace, credentials) = kyuubi.workspace("SELECT missing_column", PASSWORD);
    workspace.settings.ui_scale = 1.1;
    let query = workspace.tabs[0].id;
    let app = TestApp::launch_with(cx, workspace, credentials);
    cx.simulate_window_scale_factor_change(app.window, 2.);
    app.settle(cx);
    app.update(cx, |window, cx| {
        assert_workspace_header_alignment(window, "add-connection");
        assert_workspace_divider_alignment(window, cx);
    });
    let assert_tooltip = |cx: &mut TestAppContext, expected| {
        if expected == "Not connected" {
            app.hover_labelled(cx, "Query 1");
        } else {
            app.update(cx, |window, cx| {
                window.hover(format!("query-status-{query}"), cx)
            });
        }
        cx.executor().advance_clock(Duration::from_millis(800));
        app.settle(cx);
        app.update(cx, |window, _| {
            assert_eq!(
                label(window, "status-tooltip-title").as_deref(),
                Some("Query 1")
            );
            assert_eq!(
                label(window, "status-tooltip-status").as_deref(),
                Some(expected)
            );
            assert!(window.try_find("status-tooltip-detail").is_none());
        });
    };
    app.click(cx, "run");
    app.wait_status(cx, "Error: Query failed");
    assert_eq!(cx.update(|cx| app.status(cx)), "Error: Query failed");
    assert_tooltip(cx, "Idle");
    app.run_complete(cx, "SELECT 42 AS value");
    app.update(cx, |window, cx| {
        assert_workspace_header_alignment(window, "add-connection");
        assert_workspace_divider_alignment(window, cx);
    });
    // The status of the new run has no detail of the previous error.
    assert_eq!(cx.update(|cx| app.status(cx)), "Complete");
    app.update(cx, |window, _| {
        assert!(
            label(window, "result-elapsed")
                .unwrap()
                .starts_with("Elapsed: ")
        );
        assert_eq!(label(window, "result-range").as_deref(), Some("Rows 1–1"));
        assert_eq!(label(window, "result-loaded").as_deref(), Some("1 loaded"));
        assert_eq!(label(window, "result-columns").as_deref(), Some("1 column"));
    });
    assert_tooltip(cx, "Idle");
    app.click(cx, "disconnect");
    app.wait_status(cx, "Disconnected");
    assert_tooltip(cx, "Not connected");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn result_details_follow_fetched_pages_and_empty_results(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (workspace, credentials) = kyuubi.workspace("SELECT id FROM range(1001)", PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    cx.simulate_window_resize(app.window, size(px(600.), px(650.)));
    app.settle(cx);
    // Before the first result, the result toolbar and its details do not show.
    app.update(cx, |window, _| {
        assert!(window.try_find("result-details").is_none());
    });
    app.click(cx, "run");
    app.wait_status(cx, "Preview: More rows available");
    app.click(cx, "result-details");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "result-detail-range").as_deref(),
            Some("Visible Rows: 1–1,000")
        );
        assert_eq!(
            label(window, "result-detail-loaded").as_deref(),
            Some("Loaded Rows: 1,000")
        );
        assert_eq!(
            label(window, "result-detail-columns").as_deref(),
            Some("Columns: 1")
        );
    });
    app.click(cx, "next-page");
    app.click(cx, "result-details");
    app.wait_until(
        cx,
        "the fetched page in Result Details",
        QUERY_TIMEOUT,
        |window, _| label(window, "result-detail-loaded").as_deref() == Some("Loaded Rows: 1,001"),
    );
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "result-detail-range").as_deref(),
            Some("Visible Rows: 1,001–1,001")
        );
        assert_eq!(cell(window, 0, 1).as_deref(), Some("1000"));
    });
    app.press(cx, "escape");
    app.run_complete(cx, "SELECT 1 AS value WHERE false");
    app.click(cx, "result-details");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "result-detail-range").as_deref(),
            Some("Visible Rows: 0")
        );
        assert_eq!(
            label(window, "result-detail-loaded").as_deref(),
            Some("Loaded Rows: 0")
        );
        assert_eq!(
            label(window, "result-detail-columns").as_deref(),
            Some("Columns: 1")
        );
        assert!(
            label(window, "result-detail-elapsed")
                .unwrap()
                .starts_with("Query Duration: ")
        );
    });
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn query_rows_reach_the_results_table(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (workspace, credentials) = kyuubi.workspace(
        "SELECT id, CONCAT('row-', CAST(id AS STRING)) AS label FROM range(3)",
        PASSWORD,
    );
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.update(cx, |window, _| assert_eq!(cell(window, 0, 1), None));

    app.update(cx, |window, cx| window.click("run", cx));
    app.wait_until(cx, "the last result row", QUERY_TIMEOUT, |window, _| {
        cell(window, 2, 2).as_deref() == Some("row-2")
    });
    app.update(cx, |window, _| {
        assert_eq!(header(window, 1).as_deref(), Some("id"));
        assert_eq!(header(window, 2).as_deref(), Some("label"));
        let rows: Vec<_> = (0..3)
            .map(|row| (cell(window, row, 1), cell(window, row, 2)))
            .collect();
        assert_eq!(
            rows,
            [("0", "row-0"), ("1", "row-1"), ("2", "row-2")]
                .map(|(id, label)| (Some(id.to_owned()), Some(label.to_owned())))
        );
        assert_eq!(cell(window, 3, 1), None, "The query returned three rows");
    });
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn query_dots_keep_unread_results_until_their_own_tabs_show(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (mut workspace, credentials) = kyuubi.workspace("SELECT 42 AS value", PASSWORD);
    let profile = workspace.profiles[0].id;
    let first = workspace.tabs[0].id;
    let second = SavedTab::new(2, Some(profile));
    let second_id = second.id;
    workspace.tabs.push(second);
    let app = TestApp::launch_with(cx, workspace, credentials);
    let dot = |id| format!("query-status-{id}");
    let connection = format!("connection-status-{profile}");

    app.update(cx, |window, cx| {
        assert!(window.try_find(dot(first)).is_none());
        assert!(window.try_find(connection.clone()).is_none());
        let idle_button = bounds_of(window, "toggle-activity");
        assert_eq!(idle_button.size.width, idle_button.size.height);
        window.click("run", cx);
        assert_eq!(bounds_of(window, "toggle-activity"), idle_button);
        assert!(
            window
                .find(dot(first))
                .label()
                .unwrap()
                .contains("connecting")
        );
        assert_eq!(window.find("toggle-activity").label(), Some("Activity"));
        let second = crate::support::labelled(window, "Query 2").unwrap();
        crate::support::click_element(window, &second, cx);
    });
    app.wait_until(cx, "the unread first result", QUERY_TIMEOUT, |window, _| {
        window
            .try_find(dot(first))
            .is_some_and(|status| status.label().unwrap().contains("unread query result"))
    });
    app.update(cx, |window, _| {
        assert!(
            window
                .find(connection.clone())
                .label()
                .unwrap()
                .contains("unread query result")
        );
        assert!(window.try_find(dot(second_id)).is_none());
    });

    app.type_sql(cx, "SELECT 2 AS value");
    app.update(cx, |window, cx| {
        window.click("run", cx);
        let status = window.find(connection.clone());
        let label = status.label().unwrap();
        assert!(
            label.contains("connecting") && label.contains("unread query result"),
            "{label}"
        );
        let first = crate::support::labelled(window, "Query 1, unread query result").unwrap();
        crate::support::click_element(window, &first, cx);
    });
    app.wait_cell(cx, 0, 1, "42");
    app.wait_until(
        cx,
        "the unread second result",
        QUERY_TIMEOUT,
        |window, _| {
            window
                .try_find(dot(second_id))
                .is_some_and(|status| status.label().unwrap().contains("unread query result"))
        },
    );
    app.update(cx, |window, _| {
        assert_eq!(
            window.find(dot(first)).label(),
            Some("Query 1, connected, idle")
        );
    });
    app.click(cx, "toggle-activity");
    app.wait_for(cx, "activity");
    app.press(cx, "escape");
    app.wait_gone(cx, "activity");
    app.update(cx, |window, _| {
        assert!(
            window
                .find(dot(second_id))
                .label()
                .unwrap()
                .contains("unread query result")
        );
    });
    app.click_labelled(cx, "Query 2, unread query result");
    app.wait_cell(cx, 0, 1, "2");
    app.update(cx, |window, _| {
        assert_eq!(
            window.find(dot(second_id)).label(),
            Some("Query 2, connected, idle")
        );
        assert_eq!(
            window.find(connection.clone()).label(),
            Some("Spark, connected, idle")
        );
    });
    app.click(cx, "disconnect");
    app.wait_until(
        cx,
        "the closed second session",
        QUERY_TIMEOUT,
        |window, _| window.try_find(dot(second_id)).is_none(),
    );
    app.update(cx, |window, _| {
        assert!(window.try_find(connection.clone()).is_some())
    });
    app.click_labelled(cx, "Query 1");
    app.click(cx, "disconnect");
    app.wait_until(cx, "both closed sessions", QUERY_TIMEOUT, |window, _| {
        window.try_find(connection.clone()).is_none()
    });
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_query_result_hidden_by_activity_stays_unread(cx: &mut TestAppContext) {
    let (workspace, credentials) = Kyuubi::get().workspace("SELECT 42 AS value", PASSWORD);
    let first = workspace.tabs[0].id;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.update(cx, |window, cx| {
        window.click("run", cx);
        window.click("toggle-activity", cx);
    });
    app.wait_for(cx, "activity");
    // The query completes while Activity covers the tab strip.
    app.wait_status(cx, "Complete");
    app.dispatch(cx, qrow::ui::NewTab);
    app.press(cx, "escape");
    app.wait_gone(cx, "activity");
    app.update(cx, |window, _| {
        assert!(
            window
                .find(format!("query-status-{first}"))
                .label()
                .unwrap()
                .contains("unread query result")
        );
    });
    app.click_labelled(cx, "Query 1, unread query result");
    app.wait_cell(cx, 0, 1, "42");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_pinned_result_keeps_its_rows_after_the_next_run_closes_its_cursor(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let sql = "SELECT id FROM range(1001)";
    let (workspace, credentials) = kyuubi.workspace(sql, PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Preview: More rows available");
    app.click(cx, "pin-result");
    app.run_complete(cx, "SELECT 7 AS value");
    app.wait_cell(cx, 0, 1, "7");
    app.update(cx, |window, _| {
        assert!(window.try_find("result-partial").is_none());
    });

    app.click(cx, "result-tab-1");
    app.wait_cell(cx, 0, 1, "0");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "result-partial").as_deref(),
            Some("Partial result")
        );
        assert_eq!(
            label(window, "result-range").as_deref(),
            Some("Rows 1–1,000")
        );
    });
    // The cursor is closed, so Next page does not fetch.
    app.click(cx, "next-page");
    app.settle(cx);
    app.update(cx, |window, cx| {
        assert_eq!(app.status(cx), "Complete");
        assert_eq!(
            label(window, "result-range").as_deref(),
            Some("Rows 1–1,000")
        );
    });
    // A compact toolbar shows the Partial tag between the range and the details.
    cx.simulate_window_resize(app.window, size(px(700.), px(650.)));
    app.settle(cx);
    app.update(cx, |window, _| {
        let range = bounds_of(window, "result-range");
        let partial = bounds_of(window, "result-partial");
        let details = bounds_of(window, "result-details");
        assert!(range.right() <= partial.left() && partial.right() <= details.left());
    });
    // A narrow toolbar hides result metadata, but not the Partial tag.
    cx.simulate_window_resize(app.window, size(px(500.), px(650.)));
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(window.try_find("result-range").is_none());
        let toolbar = bounds_of(window, "query-footer");
        let partial = bounds_of(window, "result-partial");
        assert!(partial.left() >= toolbar.left() && partial.right() <= toolbar.right());
    });
    app.click(cx, "copy-result-query");
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some(sql)
    );
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn format_sql_runs_and_undo_restores_original(cx: &mut TestAppContext) {
    let sql = "select '日本語😀;' as value";
    let (workspace, credentials) = Kyuubi::get().workspace(sql, PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "sql-editor");
    app.press(cx, "secondary-a");
    app.press(cx, "alt-secondary-l");
    app.wait_until(cx, "formatted query", QUERY_TIMEOUT, |_, _| {
        app.saved().tabs[0].sql == "SELECT '日本語😀;' AS value"
    });
    app.press(cx, "secondary-enter");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "日本語😀;");
    app.click(cx, "sql-editor");
    app.press(cx, "secondary-z");
    app.wait_until(cx, "original SQL", QUERY_TIMEOUT, |_, _| {
        app.saved().tabs[0].sql == sql
    });
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn running_query_restores_minimized_results(cx: &mut TestAppContext) {
    let (workspace, credentials) = Kyuubi::get().workspace("SELECT 233 AS value", PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    let height = app.update(cx, |window, _| {
        window.find("sql-editor").bounds().size.height
    });
    app.click(cx, "toggle-results");
    app.update(cx, |window, _| {
        assert!(window.find("sql-editor").bounds().size.height > height);
        assert_eq!(window.find("toggle-results").label(), Some("Expand"));
    });
    app.click(cx, "run");
    app.wait_cell(cx, 0, 1, "233");
    app.update(cx, |window, _| {
        assert_eq!(window.find("toggle-results").label(), Some("Collapse"));
        assert_eq!(window.find("sql-editor").bounds().size.height, height);
    });
    app.press(cx, "secondary-j");
    app.press(cx, "secondary-j");
    app.wait_cell(cx, 0, 1, "233");
    app.wait_until(
        cx,
        "saved restored results",
        Duration::from_secs(10),
        |_, _| !app.saved().settings.results_minimized,
    );
}

#[cfg(target_os = "linux")]
#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn linux_application_menu_opens_settings_after_a_query(cx: &mut TestAppContext) {
    let (workspace, credentials) = Kyuubi::get().workspace("SELECT 1 AS value", PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "1");
    app.click(cx, "application-menu");
    app.wait_label(cx, "Settings…");
    app.click_labelled(cx, "Settings…");
    cx.run_until_parked();
    assert!(app.settings_window(cx).is_some());
    app.press(cx, "secondary-w");
    cx.run_until_parked();
    assert!(app.settings_window(cx).is_none());
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn smooth_motion_keeps_typed_sql_and_query_execution_immediate(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let sql = format!("SELECT 4;\n{}", "-- Synthetic scroll padding\n".repeat(100));
    let (mut workspace, credentials) = kyuubi.workspace(&sql, PASSWORD);
    workspace.settings.editor_smooth_caret = true;
    workspace.settings.editor_smooth_scrolling = true;
    workspace.settings.smooth_transitions = true;
    let app = TestApp::launch_with(cx, workspace, credentials);
    cx.update(|cx| cx.set_reduce_motion(false));
    app.dispatch(cx, qrow::ui::ToggleActivity);
    cx.executor().advance_clock(Duration::from_millis(40));
    app.update(cx, |window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    app.dispatch(cx, qrow::ui::ToggleActivity);
    app.dispatch(cx, qrow::ui::ShowSignIns);
    app.dispatch(cx, qrow::ui::ShowConnections);
    app.dispatch(cx, qrow::ui::ToggleSidebar);
    cx.executor().advance_clock(Duration::from_millis(40));
    app.update(cx, |window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    app.click_labelled(cx, "SQL Editor");
    app.press(cx, "secondary-a");
    app.press(cx, "left");
    for _ in 0..8 {
        app.press(cx, "right");
    }
    app.press(cx, "2");
    app.update(cx, |window, cx| {
        window.scroll("sql-editor", ScrollDelta::Lines(point(0., -20.)), cx)
    });
    cx.executor().advance_clock(Duration::from_millis(40));
    app.update(cx, |window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    app.press(cx, "secondary-enter");
    app.wait_until(
        cx,
        "the SQL typed and scrolled with smooth motion",
        QUERY_TIMEOUT,
        |window, _| cell(window, 0, 1).as_deref() == Some("42"),
    );
    let expected_sql = sql.replacen("SELECT 4;", "SELECT 42;", 1);
    app.wait_until(
        cx,
        "the typed SQL to be saved",
        Duration::from_secs(10),
        |_, _| app.saved().tabs[0].sql == expected_sql,
    );
    let saved = app.saved();
    assert!(saved.settings.editor_smooth_caret);
    assert!(saved.settings.editor_smooth_scrolling);
    assert!(saved.settings.smooth_transitions);
    assert_eq!(saved.tabs[0].sql, expected_sql);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn numbered_shortcuts_keep_query_results_with_their_tabs(cx: &mut TestAppContext) {
    let (mut workspace, credentials) = Kyuubi::get().workspace("SELECT 111 AS value", PASSWORD);
    let profile = workspace.tabs[0].profile;
    workspace.tabs.push(SavedTab {
        sql: "SELECT 222 AS value".into(),
        ..SavedTab::new(2, profile)
    });
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "111");
    // Switch from the result grid, then run from the newly focused editor.
    app.update(cx, |window, cx| {
        window.within(("row", 0_usize)).click(("cell", 1_usize), cx)
    });
    app.press(cx, "secondary-2");
    app.press(cx, "secondary-enter");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "222");
    app.press(cx, "secondary-1");
    app.wait_cell(cx, 0, 1, "111");
    app.press(cx, "secondary-2");
    app.wait_cell(cx, 0, 1, "222");
    app.press(cx, "secondary-9");
    app.wait_cell(cx, 0, 1, "222");
}
