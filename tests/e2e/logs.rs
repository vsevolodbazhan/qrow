use crate::support::fixture::{Kyuubi, PASSWORD};
use crate::support::{TestApp, connection_row, elements};
use gpui_kit::TestAppContext;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn copy_actions_include_timestamps_on_server_errors(cx: &mut TestAppContext) {
    let (workspace, credentials) = Kyuubi::get().workspace(
        "SELECT qrow_timestamp_missing_column FROM range(1)",
        PASSWORD,
    );
    let connection = workspace.profiles[0].id;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "run");
    app.wait_status(cx, "Error: Query failed");

    let logs = app.logs(cx);
    let submitted = logs
        .lines()
        .find(|line| line.contains("Submitted query:"))
        .expect("Logs contain the submitted query");
    assert_timestamped(submitted);

    app.click(cx, "output-panel-tab");
    app.click(cx, "output-copy-error");
    let error = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("Copy error writes to the clipboard");
    assert_timestamped(&error);
    assert!(error.contains("qrow_timestamp_missing_column"), "{error}");
    assert!(logs.contains(&error), "{logs}");

    app.context_menu(cx, connection_row(connection));
    app.choose(cx, "popup-menu", "Show activity");
    app.wait_for(cx, "activity");
    app.click(cx, "activity-errors");
    let activity_error = app.copy_activity(cx);
    assert_timestamped(&activity_error);
    assert!(
        activity_error.ends_with("Query 1: Query failed"),
        "{activity_error}"
    );
    let copy = app.update(cx, |window, _| {
        elements(window)
            .into_iter()
            .flat_map(|element| element.path().to_vec())
            .find(|id| matches!(id, gpui_kit::ElementId::NamedInteger(name, _) if name.as_ref() == "activity-copy"))
            .expect("The failed query has a Copy action")
    });
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(String::new()));
    app.click(cx, copy);
    app.settle(cx);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some(activity_error),
        "Activity Copy and Copy All include the same recorded timestamp"
    );
    app.click(cx, "activity-close");
    app.wait_gone(cx, "activity");

    app.run_complete(cx, "SELECT 1 AS value");
    app.click(cx, "output-panel-tab");
    app.click(cx, "output-copy-error");
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some(error.clone()),
        "Copy error keeps the recorded error after a successful query"
    );
    assert!(app.logs(cx).contains(&error));
}

fn assert_timestamped(text: &str) {
    let (timestamp, _) = text
        .split_once("] ")
        .unwrap_or_else(|| panic!("No timestamp in {text:?}"));
    let pattern = "[0000-00-00 00:00:00";
    assert_eq!(timestamp.len(), pattern.len(), "{text}");
    for (actual, expected) in timestamp.chars().zip(pattern.chars()) {
        if expected == '0' {
            assert!(actual.is_ascii_digit(), "{text}");
        } else {
            assert_eq!(actual, expected, "{text}");
        }
    }
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn logs_record_when_older_entries_are_removed(cx: &mut TestAppContext) {
    let (workspace, credentials) = Kyuubi::get().connections(&["Alpha"], "");
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "output-panel-tab");
    app.click(cx, "output-clear");
    app.click(cx, "results-panel-tab");

    // Rejected SQL makes a group without an execution and without a server
    // request. Logs keep 50 such groups, and they do not remove query history.
    app.run_complete(cx, "SELECT 'retention-oldest' AS value");
    app.wait_cell(cx, 0, 1, "retention-oldest");
    app.type_sql(cx, "SELECT 'retention-rejected'; SELECT 2");
    for _ in 0..100 {
        app.click(cx, "run");
    }
    app.run_complete(cx, "SELECT 'retention-latest' AS value");
    app.wait_cell(cx, 0, 1, "retention-latest");

    let logs = app.logs(cx);
    assert!(
        logs.starts_with("Older log entries were removed\n"),
        "Logs start with {:?}",
        &logs[..logs.len().min(120)]
    );
    assert!(
        logs.contains("retention-oldest"),
        "Rejected SQL removed the oldest query"
    );
    assert!(logs.contains("retention-latest"));
    assert_eq!(
        logs.matches("Run one statement at a time").count(),
        qrow::logs::MAX_NON_EXECUTION_GROUPS
    );
}
