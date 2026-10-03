//! Tests that hold Spark executors with `qrow_block`. The fixture has two
//! executor cores, so nextest runs these tests one at a time.
use crate::support::fixture::{
    Kyuubi, QUERY_TIMEOUT, REGISTER_BLOCKING, blocking, evidence, token,
};
use crate::support::{TestApp, assert_tab_status, cell, connection_row, label, labelled, shows};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::model::Profile;
use qrow::ui::Quit;
use std::time::Duration;

fn launch(cx: &mut TestAppContext, names: &[&str]) -> (TestApp, Vec<Profile>) {
    let (workspace, credentials) = Kyuubi::get().connections(names, "");
    let profiles = workspace.profiles.clone();
    (TestApp::launch_with(cx, workspace, credentials), profiles)
}

fn wait_tab(app: &TestApp, cx: &mut TestAppContext, expected: &str) {
    app.wait_until(
        cx,
        &format!("the tab {expected}"),
        QUERY_TIMEOUT,
        |window, _| labelled(window, expected).is_some(),
    );
}

/// Opening the edit form of a busy connection does nothing.
fn edit_is_unavailable(app: &TestApp, cx: &mut TestAppContext, profile: &Profile) {
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Edit");
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(
            window.try_find("connection-name").is_none(),
            "Edit opened for a busy connection"
        );
    });
    app.press(cx, "escape");
    app.wait_gone(cx, "popup-menu");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn keep_alive_runs_while_the_connection_is_hidden(cx: &mut TestAppContext) {
    let (app, profiles) = launch(cx, &["Alpha", "Beta"]);
    let (alpha, beta) = (&profiles[0], &profiles[1]);
    let keep_alive = token("keep-alive");

    app.context_menu(cx, connection_row(alpha.id));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    app.scroll_to(cx, "connection-idle-behavior");
    app.select(cx, "connection-idle-behavior", "Keep connected");
    app.scroll_to(cx, "connection-keep-alive-query");
    // The worker sends a keep-alive when it is idle for the interval, and the
    // window cannot run or page during it. The interval is longer than any
    // step of this test on a loaded runner, so only the wait below meets one.
    app.fill(cx, "connection-keep-alive-interval", "15");
    app.scroll_to(cx, "connection-keep-alive-query");
    app.fill(
        cx,
        "connection-keep-alive-query",
        &format!("SELECT qrow_keep_alive(id, '{keep_alive}', CAST(30000 AS BIGINT)) FROM range(1)"),
    );
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");

    app.run_complete(
        cx,
        "CREATE TEMPORARY FUNCTION qrow_keep_alive AS 'io.qrow.fixture.Blocking'",
    );
    app.run_sql(cx, "SET spark.sql.session.timeZone=Asia/Tokyo");
    app.wait_until(cx, "the session setting", QUERY_TIMEOUT, |window, _| {
        shows(window, "Asia/Tokyo")
    });

    // A hidden connection keeps its cursor and downloaded rows.
    app.run_sql(
        cx,
        "SELECT concat('switch-a-', lpad(CAST(id AS STRING), 4, '0')) AS value FROM range(2001) ORDER BY id",
    );
    app.wait_cell(cx, 0, 1, "switch-a-0000");
    app.click(cx, "next-page");
    app.wait_cell(cx, 0, 1, "switch-a-1000");
    app.wait_status(cx, "Preview · More rows available");
    app.wait_status(cx, "Sending keep-alive");

    // The row of the hidden connection shows its running heartbeat.
    app.select_connection(cx, beta);
    app.wait_until(cx, "the running heartbeat", QUERY_TIMEOUT, |window, _| {
        label(window, connection_row(alpha.id)).as_deref() == Some("Alpha, running")
    });
    edit_is_unavailable(&app, cx, alpha);

    app.select_connection(cx, alpha);
    app.wait_cell(cx, 0, 1, "switch-a-1000");
    app.wait_status(cx, "Connected · Keep-alive enabled");
    app.wait_cell(cx, 0, 1, "switch-a-1000");
    app.click(cx, "next-page");
    app.wait_cell(cx, 0, 1, "switch-a-2000");
    app.update(cx, |window, _| {
        assert_eq!(label(window, "page-label").as_deref(), Some("Page 3"))
    });
    app.click(cx, "previous-page");
    app.wait_cell(cx, 0, 1, "switch-a-1000");
    // The same session keeps its time zone setting.
    app.run_sql(
        cx,
        "SELECT concat('same-session-', current_timezone()) AS value",
    );
    app.wait_cell(cx, 0, 1, "same-session-Asia/Tokyo");
    assert!(
        evidence(&keep_alive, "started") >= 1,
        "No keep-alive query ran"
    );
    // A completed keep-alive shows in Activity without its SQL. It stays out
    // of the tab Logs.
    let activity = app.activity(cx, alpha.id);
    assert!(activity.contains(": Keep-alive completed"), "{activity}");
    assert!(!activity.contains("qrow_keep_alive"), "{activity}");
    assert!(!app.logs(cx).contains("Keep-alive"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_retry_clears_the_error_badge(cx: &mut TestAppContext) {
    let (app, profiles) = launch(cx, &["Alpha"]);
    let alpha = &profiles[0];
    app.run_complete(cx, REGISTER_BLOCKING);

    // Issue #40: selecting Results does not acknowledge an unread error, and a
    // retry clears the badge before it finishes.
    for (panel_click, milliseconds) in [(true, 3000), (false, 1000)] {
        app.run_sql(cx, "SELECT missing_column AS value FROM range(1)");
        wait_tab(&app, cx, "Query 1, unread error");
        if panel_click {
            app.wait_status(cx, "Error · Query failed");
            app.update(cx, |window, _| {
                assert_eq!(
                    label(window, connection_row(alpha.id)).as_deref(),
                    Some("Alpha, unread error")
                );
                assert_eq!(cell(window, 0, 1), None, "The failed query shows rows");
            });
            app.click(cx, "results-panel-tab");
            app.settle(cx);
            app.update(cx, |window, _| {
                assert!(
                    labelled(window, "Query 1, unread error").is_some(),
                    "Results acknowledged the error"
                );
            });
        }
        let retry = token("badge");
        app.run_sql(cx, &blocking(&retry, milliseconds));
        app.wait_evidence(cx, &retry, "started", QUERY_TIMEOUT);
        wait_tab(&app, cx, "Query 1, running");
        app.update(cx, |window, _| {
            assert!(
                labelled(window, "Query 1, running, unread error").is_none(),
                "The retry kept the badge"
            );
        });
        if panel_click {
            app.click(cx, "output-panel-tab");
        }
        wait_tab(&app, cx, "Query 1");
        // The session that reported the error runs the retry.
        app.wait_cell(cx, 0, 1, "0");
    }
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_connection_shows_one_status_while_another_tab_runs_after_an_error(cx: &mut TestAppContext) {
    let (app, profiles) = launch(cx, &["Alpha"]);
    let profile = &profiles[0];
    app.run_sql(cx, "SELECT missing_column AS value FROM range(1)");
    wait_tab(&app, cx, "Query 1, unread error");
    let failed = app.saved().tabs[0].id;
    app.update(cx, |window, _| assert_tab_status(window, failed, "error"));

    app.click(cx, "new-tab");
    app.wait_editor(cx, "");
    app.wait_until(cx, "the saved new tab", QUERY_TIMEOUT, |_, _| {
        let workspace = app.saved();
        workspace.tabs[workspace.active_tab].id != failed
    });
    let workspace = app.saved();
    let running = workspace.tabs[workspace.active_tab].id;
    app.run_complete(cx, REGISTER_BLOCKING);
    let query = token("one-status");
    app.run_sql(cx, &blocking(&query, 15_000));
    app.wait_evidence(cx, &query, "started", QUERY_TIMEOUT);
    app.wait_label(cx, "Alpha, running, unread error");
    app.update(cx, |window, _| {
        assert_tab_status(window, failed, "error");
        assert_tab_status(window, running, "query");
        assert!(
            window
                .try_find(format!("connection-error-{}", profile.id))
                .is_some()
        );
        assert!(
            window
                .try_find(format!("connection-busy-{}", profile.id))
                .is_none()
        );
    });
    app.click(cx, format!("close-tab-{running}"));
    app.settle(cx);
    app.update(cx, |window, _| assert_tab_status(window, running, "query"));
    // Acknowledging the failed tab exposes the remaining work indicator.
    app.click_labelled(cx, "Query 1, unread error");
    app.wait_label(cx, "Alpha, running");
    app.update(cx, |window, _| {
        assert!(
            window
                .try_find(format!("connection-error-{}", profile.id))
                .is_none()
        );
        assert!(
            window
                .try_find(format!("connection-busy-{}", profile.id))
                .is_some()
        );
    });
    app.click(cx, format!("connection-busy-{}", profile.id));
    app.wait_for(cx, "activity");
    app.press(cx, "escape");
    app.wait_gone(cx, "activity");
    app.click_labelled(cx, "Query 2, running");
    app.wait_cell(cx, 0, 1, "0");
    app.wait_label(cx, "Alpha");
    app.update(cx, |window, _| {
        assert!(
            window
                .try_find(format!("connection-error-{}", profile.id))
                .is_none()
        );
        assert!(
            window
                .try_find(format!("connection-busy-{}", profile.id))
                .is_none()
        );
    });
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn cancel_stops_spark_and_keeps_the_partial_preview(cx: &mut TestAppContext) {
    let (app, profiles) = launch(cx, &["Alpha"]);
    let alpha = &profiles[0];
    app.run_complete(cx, REGISTER_BLOCKING);
    let query = token("cancel");
    app.run_sql(cx, &blocking(&query, 60_000));
    app.wait_evidence(cx, &query, "started", Duration::from_secs(150));
    wait_tab(&app, cx, "Query 1, running");

    // Quit asks before it stops a running query.
    app.dispatch(cx, Quit);
    app.wait_for(cx, "keep-working");
    app.click(cx, "keep-working");
    app.wait_gone(cx, "keep-working");

    // A busy connection cannot be edited or deleted.
    edit_is_unavailable(&app, cx, alpha);
    app.context_menu(cx, connection_row(alpha.id));
    app.choose(cx, "popup-menu", "Delete");
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(
            window.try_find("confirm-delete-connection").is_none(),
            "Delete opened for a busy connection"
        );
    });
    app.press(cx, "escape");
    app.wait_gone(cx, "popup-menu");

    // Another tab of the connection works while the first one runs.
    app.click(cx, "new-tab");
    wait_tab(&app, cx, "Query 2");
    app.run_sql(cx, "SELECT 'other-tab-works' AS result");
    app.wait_cell(cx, 0, 1, "other-tab-works");

    app.click_labelled(cx, "Query 1, running");
    app.wait_for(cx, "cancel");
    // The backend suite checks that Spark interrupts the task in time.
    app.click(cx, "cancel");
    app.wait_status(cx, "Cancelled · Partial preview retained");

    app.run_sql(cx, "SELECT 'after-cancel-works' AS result");
    app.wait_cell(cx, 0, 1, "after-cancel-works");
    app.click(cx, "disconnect");
    app.run_sql(cx, "SELECT 'reconnect-works' AS result");
    app.wait_cell(cx, 0, 1, "reconnect-works");
}
