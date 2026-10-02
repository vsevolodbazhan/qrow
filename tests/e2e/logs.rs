use crate::support::TestApp;
use crate::support::fixture::Kyuubi;
use gpui_kit::TestAppContext;

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
