use crate::support::assistant::{FakeCodex, approval, editor_text};
use crate::support::fixture::{Kyuubi, QUERY_TIMEOUT};
use crate::support::{TestApp, bounds_of, label};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn the_assistant_runs_approved_and_automatic_queries_in_its_tab(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let (workspace, credentials) =
        Kyuubi::get().connections(&["Alpha"], "SELECT 1 AS assistant_value");
    let app = TestApp::launch_in(cx, directory, codex.workspace(workspace), credentials);
    app.wait_editor(cx, "SELECT 1 AS assistant_value");
    app.open_assistant(cx);

    app.send(cx, "Run selected SQL with approval");
    app.wait_until(cx, "the approval", QUERY_TIMEOUT, |window, _| {
        approval(window).is_some_and(|a| a.starts_with("Run in Query 1 · Alpha?"))
    });
    app.update(cx, |window, _| {
        assert!(
            window.try_find("assistant-working").is_none(),
            "Working showed during the approval"
        );
        assert!(
            window.try_find("assistant-send").is_none(),
            "Send showed during the turn"
        );
        let stop = bounds_of(window, "assistant-stop");
        let composer = bounds_of(window, "assistant-composer");
        assert!(
            stop.top() >= composer.bottom(),
            "Cancel is not below the message field"
        );
    });
    app.click(cx, "assistant-approve-query");
    app.wait_reply(cx, "I ran the query.");
    app.wait_for(cx, "assistant-send");
    app.wait_cell(cx, 0, 1, "1");

    app.choose_send_mode(cx, "Run automatically");
    app.click(cx, "confirm-conversation-auto-run");
    app.wait_until(cx, "Send · Run", QUERY_TIMEOUT, |window, _| {
        label(window, "assistant-send").as_deref() == Some("Send · Run")
    });
    app.type_sql(cx, "SELECT 2 AS assistant_value");
    app.send(cx, "Run selected SQL automatically");
    app.wait_until(cx, "the automatic result", QUERY_TIMEOUT, |window, _| {
        assert!(
            approval(window).is_none(),
            "Automatic mode requested approval"
        );
        crate::support::cell(window, 0, 1).as_deref() == Some("2")
    });
    app.wait_idle(cx);

    app.send(cx, "Append and run SQL without revision");
    app.wait_reply(cx, "I ran the appended query.");
    app.wait_until(cx, "the appended query", QUERY_TIMEOUT, |window, _| {
        editor_text(window)
            .is_some_and(|sql| sql.contains("-- Assistant value\nSELECT 3 AS assistant_value"))
    });
    app.wait_cell(cx, 0, 1, "3");
    app.wait_idle(cx);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn the_assistant_reads_170_results_past_a_wide_row(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let sql = "SELECT id, CASE WHEN id = 25 THEN repeat('x', 64400) ELSE 'value' END AS payload FROM range(170) ORDER BY id";
    let (workspace, credentials) = Kyuubi::get().connections(&["Alpha"], sql);
    let app = TestApp::launch_in(cx, directory, codex.workspace(workspace), credentials);
    app.wait_editor(cx, sql);
    app.open_assistant(cx);
    app.choose_send_mode(cx, "Run automatically");
    app.click(cx, "confirm-conversation-auto-run");
    app.send(cx, "Run 170 rows and read results");
    app.wait_reply(cx, "Read all 170 row positions.");
    app.wait_idle(cx);
}
