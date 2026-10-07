use crate::support::fixture::{Kyuubi, PASSWORD, QUERY_TIMEOUT};
use crate::support::{
    TestApp, assert_workspace_divider_alignment, assert_workspace_header_alignment, bounds_of,
    cell, header, label,
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, px, size};
use qrow::model::SavedTab;
use std::time::Duration;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn run_executes_the_last_statement_unless_text_is_selected(cx: &mut TestAppContext) {
    let sql = "SELECT '最初😀;' AS value;\nSELECT '最後😀;' AS value;\n\n-- trailing ;\n/* trailing ; /* nested ; */ */\n";
    let (workspace, credentials) = Kyuubi::get().workspace(sql, PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "sql-editor");
    app.press(cx, "cmd-a");
    app.press(cx, "left");
    app.press(cx, "cmd-enter");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "最後😀;");
    assert_eq!(app.saved().tabs[0].sql, sql);

    // The caret is at the start. Select the first line through the editor.
    app.click(cx, "sql-editor");
    app.press(cx, "cmd-a");
    app.press(cx, "left");
    app.press(cx, "cmd-shift-right");
    app.press(cx, "cmd-enter");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "最初😀;");
    assert_eq!(app.saved().tabs[0].sql, sql);

    // Run has the same default, including a statement without a separator.
    let sql = "invalid earlier SQL;\nSELECT 3 AS value\n-- trailing ;\n";
    app.type_sql(cx, sql);
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
        assert!(window.try_find("result-detail-elapsed").is_none());
    });
    app.press(cx, "escape");
    app.click(cx, "run");
    app.wait_status(cx, "Preview: More rows available");
    app.click(cx, "result-details");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "result-detail-range").as_deref(),
            Some("Visible Rows: 1–1000")
        );
        assert_eq!(
            label(window, "result-detail-loaded").as_deref(),
            Some("Loaded Rows: 1000")
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
        |window, _| label(window, "result-detail-loaded").as_deref() == Some("Loaded Rows: 1001"),
    );
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "result-detail-range").as_deref(),
            Some("Visible Rows: 1001–1001")
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
