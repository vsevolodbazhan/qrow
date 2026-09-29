//! The assistant edits, formats, and requests SQL in its tab.
use crate::support::assistant::{FakeCodex, approval};
use crate::support::{MemoryCredentials, TestApp, label, offline_profile, present};
use gpui_kit::{ElementId, TestAppContext};
use qrow::model::{AssistantTitleSource::Codex, SavedTab, Workspace};
use qrow::sql::KeywordCase;
use qrow::ui::OpenSettings;

fn launch(
    cx: &mut TestAppContext,
    sql: &str,
    configure: impl FnOnce(&mut Workspace),
) -> (TestApp, FakeCodex) {
    let (directory, codex) = FakeCodex::new();
    let profile = offline_profile("Synthetic");
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = sql.into();
    let mut workspace = codex.workspace(Workspace {
        profiles: vec![profile],
        tabs: vec![tab],
        ..Workspace::default()
    });
    configure(&mut workspace);
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    (app, codex)
}

const LONG_QUERY: &str = "SELECT 1;\n\nSELECT 2;\n\n-- Bookings by state\nSELECT\n  state,\n  COUNT(*) AS bookings,\n  MAX(booked_at) AS last_booked_at\nFROM integrations.bookings\nGROUP BY state;";

#[gpui_kit::test]
fn appended_sql_keeps_earlier_queries_and_titles_the_conversation(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, "", |_| {});
    app.open_assistant(cx);
    app.send(cx, "Write SELECT 1 into this tab");
    app.wait_editor(cx, "SELECT 1");
    app.wait_idle(cx);
    // The first message starts title generation alongside the reply.
    app.wait_conversations(cx, &[("Title: Write SELECT 1", Codex)]);
    app.send(cx, "Write SELECT 2 into this tab");
    app.wait_editor(cx, "SELECT 1;\n\nSELECT 2");
    app.wait_idle(cx);
    // A generated title stays after later replies.
    app.settle(cx);
    assert_eq!(
        app.conversations(),
        [("Title: Write SELECT 1".to_owned(), Codex)]
    );
    // Qrow formats a long query when the assistant appends it. The comment
    // above the query stays as written.
    app.send(cx, "Write a long query into this tab");
    app.wait_reply(cx, "I formatted the SQL.");
    app.wait_editor(cx, LONG_QUERY);
}

#[gpui_kit::test]
fn run_mode_needs_confirmation_and_reconnect_replaces_send(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, "", |_| {});
    app.open_assistant(cx);
    app.choose_send_mode(cx, "Run automatically");
    app.click(cx, "cancel-conversation-auto-run");
    app.wait_gone(cx, "cancel-conversation-auto-run");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "assistant-send").as_deref(),
            Some("Send · Ask")
        )
    });
    app.choose_send_mode(cx, "Run automatically");
    app.click(cx, "confirm-conversation-auto-run");
    app.wait_until(
        cx,
        "Send · Run",
        std::time::Duration::from_secs(10),
        |window, _| label(window, "assistant-send").as_deref() == Some("Send · Run"),
    );

    // Reconnect replaces Send and Cancel while Codex is disconnected.
    app.send(cx, "Disconnect Codex");
    app.wait_for(cx, "assistant-reconnect");
    app.update(cx, |window, _| {
        for id in ["assistant-send", "assistant-stop"] {
            assert!(
                !present(window, &ElementId::Name(id.into())),
                "{id} stayed visible"
            );
        }
    });
    app.click(cx, "assistant-reconnect");
    app.wait_gone(cx, "assistant-reconnect");
    app.wait_for(cx, "assistant-send");
    app.send_when_ready(cx, "Reply after reconnect");
    app.wait_reply(cx, "I can help with this query");
    app.wait_idle(cx);
}

#[gpui_kit::test]
fn requests_target_one_statement_and_edits_use_the_sql_style(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, "SELECT 0;", |workspace| {
        workspace.settings.editor_tab_size = 4;
        workspace.settings.assistant.sql_keyword_case = KeywordCase::Lowercase;
    });
    app.wait_editor(cx, "SELECT 0;");
    app.open_assistant(cx);
    app.send(cx, "Append two SQL statements with edit tool");
    app.wait_editor(cx, "SELECT 0;\n\nSELECT 1;\n\nSELECT 2");
    app.wait_idle(cx);

    // The newest statement, and an earlier one selected by its byte range.
    for (message, sql) in [
        ("Run selected SQL without range", "SELECT 2"),
        ("Run first SQL by range", "SELECT 0;"),
    ] {
        app.send(cx, message);
        app.wait_approval(cx, &format!("Run in Query 1 · Synthetic? {sql}"));
        app.click(cx, "assistant-cancel-query");
        app.wait_gone(cx, "assistant-query-approval");
        app.wait_idle(cx);
    }

    // An edit that supplies a long statement uses the saved style.
    app.send(cx, "Rewrite the last statement with edit tool");
    app.wait_reply(cx, "I formatted the SQL.");
    app.wait_editor(
        cx,
        "SELECT 0;\n\nSELECT 1;\n\nselect\n    state,\n    count(*) as bookings,\n    max(booked_at) as last_booked_at\nfrom integrations.bookings\ngroup by state",
    );
    app.wait_idle(cx);

    // A change in Settings applies to the next message.
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "tab size");
    app.wait_for(cx, "setting-editor-tab-size");
    assert_eq!(app.stepper(cx, "setting-editor-tab-size"), "4");
    app.step(cx, "setting-editor-tab-size", "increment", "5");
    app.fill_labelled(cx, "Search...", "keyword");
    app.wait_for(cx, "setting-sql-keyword-case");
    app.select(cx, "setting-sql-keyword-case", "Uppercase");
    app.click(cx, "save-settings");
    app.wait_gone(cx, "save-settings");
    app.send(cx, "Report the SQL style");
    app.wait_reply(cx, "SQL style: uppercase, 5 spaces");

    // A later selection cannot reuse the revision of the appended query.
    app.send(cx, "Append then retarget and run without revision");
    app.wait_approval(cx, "Run in Query 1 · Synthetic? SELECT 0;");
    app.click(cx, "assistant-cancel-query");
    app.wait_reply(cx, "Implicit run rejected: invalid_arguments");
    app.update(cx, |window, _| {
        assert!(approval(window).is_none(), "An implicit run targeted another statement");
    });
}
