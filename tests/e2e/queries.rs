use crate::support::fixture::{Kyuubi, PASSWORD, QUERY_TIMEOUT};
use crate::support::{TestApp, bounds_of, cell, header, label};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::model::SavedTab;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn query_metadata_separates_status_detail_duration_and_counts(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (workspace, credentials) = kyuubi.workspace("SELECT missing_column", PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Error: Query failed");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "query-status-title").as_deref(),
            Some("Error")
        );
        assert_eq!(
            label(window, "query-status-detail").as_deref(),
            Some("Query failed")
        );
    });
    app.run_complete(cx, "SELECT 42 AS value");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "query-status-title").as_deref(),
            Some("Complete")
        );
        assert!(
            window.try_find("query-status-detail").is_none(),
            "The previous error detail survived a new run"
        );
        assert!(
            label(window, "query-elapsed")
                .unwrap()
                .starts_with("Elapsed: ")
        );
        assert_eq!(label(window, "result-range").as_deref(), Some("Rows 1–1"));
        assert_eq!(label(window, "result-loaded").as_deref(), Some("1 loaded"));
        assert_eq!(label(window, "result-columns").as_deref(), Some("1 column"));
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
        assert!(
            window
                .find("toggle-activity")
                .label()
                .unwrap()
                .contains("work running")
        );
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
    app.wait_until(
        cx,
        "the completed hidden query",
        QUERY_TIMEOUT,
        |window, _| window.find("toggle-activity").label() == Some("Activity"),
    );
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
