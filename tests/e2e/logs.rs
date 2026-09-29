use crate::support::TestApp;
use crate::support::fixture::Kyuubi;
use gpui_kit::TestAppContext;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn logs_record_when_older_activity_is_removed(cx: &mut TestAppContext) {
    let (workspace, credentials) = Kyuubi::get().connections(&["Alpha"], "");
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.click(cx, "output-panel-tab");
    app.click(cx, "output-clear");
    app.click(cx, "results-panel-tab");

    // Logs keep 100 activity groups. A rejected statement makes a group
    // without a server request, so only the first and last queries reach Spark.
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
        logs.starts_with("Older activity was removed\n"),
        "Logs start with {:?}",
        &logs[..logs.len().min(120)]
    );
    assert!(logs.contains("retention-latest"));
    assert!(logs.contains("Run one statement at a time"));
    assert!(
        !logs.contains("retention-oldest"),
        "Logs kept the oldest query after the limit"
    );
}
