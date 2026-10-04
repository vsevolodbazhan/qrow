use crate::support::assistant::{FakeCodex, approval, editor_text};
use crate::support::fixture::{Kyuubi, QUERY_TIMEOUT};
use crate::support::{TestApp, assert_tab_dot, bounds_of, label};
use gpui_kit::TestAppContext;
use gpui_kit::component::ActiveTheme;
use gpui_kit::test::TestWindowExt;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn assistant_turns_leave_a_connected_query_tab_idle_until_sql_runs(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let (mut workspace, credentials) =
        Kyuubi::get().workspace("SELECT 1 AS value", crate::support::fixture::PASSWORD);
    workspace.tabs[0].title = "SQL only".into();
    let tab = workspace.tabs[0].id;
    let dot = format!("query-status-{tab}");
    let app = TestApp::launch_in(cx, directory, codex.workspace(workspace), credentials);
    app.run_complete(cx, "SELECT 1 AS value");
    app.wait_cell(cx, 0, 1, "1");
    app.open_assistant(cx);
    app.send(cx, "Title before first reply");
    app.wait_until(cx, "the held reply", QUERY_TIMEOUT, |_, _| {
        codex.marked("first-reply-pending")
    });
    app.wait_label(cx, "Toggle Assistant, working");
    app.update(cx, |window, cx| {
        assert_tab_dot(window, tab, Some(cx.theme().info.opacity(0.4)));
        assert_eq!(
            label(window, dot.clone()).as_deref(),
            Some("SQL only, connected, idle")
        );
    });

    // A reply completed while the pane is hidden stays on the assistant icon.
    app.click(cx, "toggle-assistant");
    app.wait_gone(cx, "assistant-composer");
    codex.mark("first-reply-release");
    app.wait_label(cx, "Toggle Assistant, reply ready");
    app.update(cx, |window, cx| {
        assert_tab_dot(window, tab, Some(cx.theme().info.opacity(0.4)))
    });
    app.click(cx, "toggle-assistant");
    app.wait_reply(cx, "I can help with this query");
    app.wait_idle(cx);

    app.send(cx, "Run selected SQL with approval");
    app.wait_approval(cx, "Run in SQL only · Spark? SELECT 1 AS value");
    app.update(cx, |window, cx| {
        assert_tab_dot(window, tab, Some(cx.theme().info.opacity(0.4)));
        window.click("assistant-approve-query", cx);
        assert_tab_dot(window, tab, Some(cx.theme().info));
    });
    app.wait_reply(cx, "I ran the query.");
    app.wait_idle(cx);
    app.wait_cell(cx, 0, 1, "1");
    app.update(cx, |window, cx| {
        assert_tab_dot(window, tab, Some(cx.theme().info.opacity(0.4)))
    });
}

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

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn the_assistant_reads_columns_through_a_connected_tab(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let schema = format!("qrow_tools_{}", uuid::Uuid::new_v4().simple());
    let (mut workspace, credentials) = Kyuubi::get().connections(&["Alpha"], "SELECT 1");
    // Other tests make schemas too. The filter keeps the catalog to this one.
    workspace.profiles[0].catalog.include = vec![schema.clone()];
    workspace.profiles[0].catalog.refresh = qrow::model::CatalogRefresh::Manual;
    let app = TestApp::launch_in(cx, directory, codex.workspace(workspace), credentials);
    app.run_complete(cx, &format!("CREATE DATABASE {schema}"));
    app.run_complete(
        cx,
        &format!("CREATE TABLE {schema}.bookings (id BIGINT, gate STRING) USING parquet"),
    );

    // Qrow never read the catalog. The tab is connected, so the tool reads
    // it before it answers.
    app.open_assistant(cx);
    app.send(cx, "List live schemas");
    app.wait_reply(cx, &format!("Live schemas: {schema}"));
    app.wait_idle(cx);
    app.send(cx, &format!("Describe the live table {schema}.bookings"));
    app.wait_until(cx, "the live columns", QUERY_TIMEOUT, |window, _| {
        crate::support::assistant::transcript(window)
            .iter()
            .any(|entry| entry.contains("Live columns: id BIGINT,gate STRING"))
    });
    app.wait_idle(cx);
    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn the_assistant_reads_columns_of_another_connected_connection(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let schema = format!("qrow_other_{}", uuid::Uuid::new_v4().simple());
    let (mut workspace, credentials) = Kyuubi::get().connections(&["Alpha", "Beta"], "SELECT 1");
    // Other tests make schemas too. The filter keeps the catalog to this one.
    for profile in &mut workspace.profiles {
        profile.catalog.include = vec![schema.clone()];
        profile.catalog.refresh = qrow::model::CatalogRefresh::Manual;
    }
    let (alpha, beta) = (workspace.profiles[0].clone(), workspace.profiles[1].clone());
    let app = TestApp::launch_in(cx, directory, codex.workspace(workspace), credentials);
    // Beta gets a live session. Qrow has not loaded its catalog cache.
    app.select_connection(cx, &beta);
    app.run_complete(cx, &format!("CREATE DATABASE {schema}"));
    app.run_complete(
        cx,
        &format!("CREATE TABLE {schema}.bookings (id BIGINT, gate STRING) USING parquet"),
    );

    // The tool waits for the cache of Beta, then reads the catalog with it.
    app.select_connection(cx, &alpha);
    app.open_assistant(cx);
    app.send(
        cx,
        &format!("Describe the live table {schema}.bookings on Beta"),
    );
    app.wait_until(cx, "the live columns", QUERY_TIMEOUT, |window, _| {
        crate::support::assistant::transcript(window)
            .iter()
            .any(|entry| entry.contains("Live columns: id BIGINT,gate STRING"))
    });
    app.wait_idle(cx);
    app.select_connection(cx, &beta);
    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}
