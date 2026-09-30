//! Each conversation belongs to one query tab, also while other tabs change.
use crate::support::assistant::{FakeCodex, REPLY_TIMEOUT, approval};
use crate::support::{
    MemoryCredentials, TestApp, connection_row, labelled, labels, offline_profile,
};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::model::{
    AssistantTitleSource::{Codex, Temporary},
    SavedTab, Workspace,
};
use qrow::ui::ToggleSidebar;
use std::collections::BTreeSet;
use uuid::Uuid;

struct Connections {
    app: TestApp,
    codex: FakeCodex,
    alpha: Uuid,
    beta: Uuid,
}

/// Connections Alpha and Beta with one tab each, and a narrow assistant pane.
fn connections(cx: &mut TestAppContext, alpha_sql: &str) -> Connections {
    let (directory, codex) = FakeCodex::new();
    let (alpha, beta) = (offline_profile("Alpha"), offline_profile("Beta"));
    let (a, b) = (alpha.id, beta.id);
    let mut first = SavedTab::new(1, Some(a));
    first.sql = alpha_sql.into();
    let mut workspace = codex.workspace(Workspace {
        profiles: vec![alpha, beta],
        tabs: vec![first, SavedTab::new(1, Some(b))],
        ..Workspace::default()
    });
    workspace.settings.assistant.panel_width = 536.;
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    Connections {
        app,
        codex,
        alpha: a,
        beta: b,
    }
}

fn tab_of(app: &TestApp, profile: Uuid) -> Option<SavedTab> {
    app.saved()
        .tabs
        .into_iter()
        .find(|tab| tab.profile == Some(profile))
}

fn tabs_of(app: &TestApp, profile: Uuid) -> Vec<SavedTab> {
    app.saved()
        .tabs
        .into_iter()
        .filter(|tab| tab.profile == Some(profile))
        .collect()
}

#[gpui_kit::test]
fn a_conversation_keeps_its_tab_when_another_tab_is_renamed_and_selected(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let profile = offline_profile("Synthetic");
    let id = profile.id;
    let tab = |title: &str, sql: &str| SavedTab {
        title: title.into(),
        sql: sql.into(),
        ..SavedTab::new(1, Some(id))
    };
    let workspace = codex.workspace(Workspace {
        profiles: vec![profile],
        tabs: vec![tab("Query 1", "SELECT 1;"), tab("Query 2", "SELECT 99;")],
        ..Workspace::default()
    });
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.wait_editor(cx, "SELECT 1;");
    app.open_assistant(cx);
    app.send(cx, "Run tab selected after rename");

    app.context_menu_labelled(cx, "Query 2");
    app.choose(cx, "popup-menu", "Rename…");
    app.fill(cx, "rename-tab-name", "Default");
    app.click(cx, "rename-tab");
    app.wait_gone(cx, "rename-tab-name");
    app.click_labelled(cx, "Default");
    app.wait_editor(cx, "SELECT 99;");
    codex.mark("retarget-ready");

    // The request waits in the conversation tab. The selected tab does not show it.
    app.wait_label(cx, "Query 1, assistant waiting for approval");
    app.wait_label(cx, "Toggle Assistant, waiting for approval");
    app.update(cx, |window, _| {
        assert_eq!(approval(window), None, "The request showed in another tab")
    });
    app.click_labelled(cx, "Query 1, assistant waiting for approval");
    app.wait_approval(cx, "Run in Query 1 · Synthetic? SELECT 1;");
    app.wait_editor(cx, "SELECT 1;");
    app.click(cx, "assistant-cancel-query");
    app.wait_gone(cx, "assistant-query-approval");
}

#[gpui_kit::test]
fn a_tool_call_with_another_tab_id_appends_to_the_conversation_tab(cx: &mut TestAppContext) {
    let Connections {
        app,
        codex,
        alpha,
        beta,
    } = connections(cx, "SELECT 1;");
    app.wait_editor(cx, "SELECT 1;");
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.send(cx, "Write query with another tab ID");
    app.click(cx, connection_row(beta));
    app.wait_editor(cx, "");
    codex.mark("wrong-tab-ready");
    app.wait_until(
        cx,
        "the append to the conversation tab only",
        REPLY_TIMEOUT,
        |_, _| {
            tab_of(&app, alpha).is_some_and(|t| {
                t.sql == "SELECT 1;\n\n-- Tables in dwh_meta\nSHOW TABLES IN dwh_meta"
            }) && tab_of(&app, beta).is_some_and(|t| t.sql.is_empty())
        },
    );
    app.click(cx, connection_row(alpha));
    app.wait_reply(cx, "I updated the SQL.");
    app.update(cx, |window, _| {
        assert!(!labels(window).iter().any(|l| l.contains("Failed")))
    });
}

#[gpui_kit::test]
fn two_conversations_work_at_the_same_time_in_their_own_tabs(cx: &mut TestAppContext) {
    let Connections {
        app,
        codex,
        alpha,
        beta,
    } = connections(cx, "");
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.send(cx, "Hold parallel Alpha");
    app.wait_for(cx, "assistant-working");
    app.wait_label(cx, "Query 1, assistant working");
    app.click(cx, connection_row(beta));
    // The Beta tab has no conversation, so its pane does not wait.
    app.wait_gone(cx, "assistant-working");
    app.send(cx, "Hold parallel Beta");
    app.wait_for(cx, "assistant-working");
    app.show_threads(cx);
    app.wait_label_containing(cx, "Alpha, assistant working");
    app.wait_label_containing(cx, "Beta, assistant working");

    codex.mark("release-Alpha");
    codex.mark("release-Beta");
    app.wait_label_containing(cx, "Alpha, assistant waiting for approval");
    app.wait_label_containing(cx, "Beta, assistant waiting for approval");
    app.wait_label(cx, "Toggle Assistant, waiting for approval");
    app.show_conversation(cx);
    // Each conversation changed only its own tab.
    app.wait_editor(cx, "SELECT 22");
    app.wait_approval(cx, "Run in Query 1 · Beta? SELECT 22");
    app.click(cx, "assistant-cancel-query");
    app.wait_gone(cx, "assistant-query-approval");

    // Selecting the other conversation selects its tab and connection.
    app.show_threads(cx);
    app.update(cx, |window, cx| {
        let row = crate::support::labelled_starting(window, "")
            .into_iter()
            .find(|e| {
                e.label()
                    .is_some_and(|l| l.contains("Alpha, assistant waiting for approval"))
            })
            .unwrap();
        crate::support::click_element(window, &row, cx);
    });
    app.show_conversation(cx);
    app.wait_editor(cx, "SELECT 11");
    app.wait_approval(cx, "Run in Query 1 · Alpha? SELECT 11");
    // The Beta turn ends only now, while another conversation is shown.
    codex.mark("finish-Beta");
    app.show_threads(cx);
    app.wait_label_containing(cx, "Beta, assistant reply ready");
    app.show_conversation(cx);
    app.click(cx, "assistant-cancel-query");
    app.wait_reply(cx, "Finished Alpha: approval_cancelled");
    app.wait_label(cx, "Toggle Assistant, reply ready");
    app.show_threads(cx);
    app.update(cx, |window, cx| {
        let row = crate::support::labelled_starting(window, "")
            .into_iter()
            .find(|e| {
                e.label()
                    .is_some_and(|l| l.contains("Beta, assistant reply ready"))
            })
            .unwrap();
        crate::support::click_element(window, &row, cx);
    });
    app.show_conversation(cx);
    app.wait_reply(cx, "Finished Beta: approval_cancelled");
    app.wait_label(cx, "Toggle Assistant");
    assert!(
        !codex.marked("duplicate-answer"),
        "Qrow answered a replayed tool call twice"
    );
    app.wait_until(
        cx,
        "each conversation owns its tab",
        REPLY_TIMEOUT,
        |_, _| {
            let (Some(a), Some(b)) = (tab_of(&app, alpha), tab_of(&app, beta)) else {
                return false;
            };
            let linked: BTreeSet<_> = app
                .saved()
                .assistant
                .conversations
                .iter()
                .filter_map(|c| c.tab_id)
                .collect();
            a.sql == "SELECT 11" && b.sql == "SELECT 22" && linked == BTreeSet::from([a.id, b.id])
        },
    );
}

const TITLE: &str = "Title: Name sales report";

fn tab_label_is(app: &TestApp, cx: &mut TestAppContext, title: &str) {
    app.wait_until(
        cx,
        &format!("the tab {title}"),
        REPLY_TIMEOUT,
        |window, _| labelled(window, title).is_some(),
    );
}

#[gpui_kit::test]
fn new_conversation_tabs_take_generated_titles(cx: &mut TestAppContext) {
    let Connections {
        app, codex, alpha, ..
    } = connections(cx, "");
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.dispatch(cx, ToggleSidebar);
    app.wait_gone(cx, "add-connection");

    app.click(cx, "assistant-new");
    tab_label_is(&app, cx, "Query 2");
    app.send(cx, "Hold title generation");
    app.wait_reply(cx, "I can help with this query");
    app.wait_until(cx, "the title request", REPLY_TIMEOUT, |_, _| {
        codex.marked("title-generation-pending")
    });
    app.wait_label_containing(cx, "generating title");
    app.wait_label(cx, "Generating title for New conversation");
    codex.mark("title-generation-release");
    tab_label_is(&app, cx, "Title: Hold title generation");

    // A title can arrive before the first reply. The tab and the header agree.
    app.click(cx, "assistant-new");
    tab_label_is(&app, cx, "Query 2");
    app.send(cx, "Title before first reply");
    app.wait_for(cx, "assistant-working");
    app.wait_until(cx, "the early title", REPLY_TIMEOUT, |window, _| {
        labels(window)
            .iter()
            .any(|l| l.starts_with("Title: Title before first"))
    });
    app.wait_label(cx, "Conversation title: Title: Title before first");
    codex.mark("first-reply-release");
    app.wait_reply(cx, "I can help with this query");
    // Cmd+W closes the active tab, which is the tab of this conversation.
    app.click(cx, "sql-editor");
    app.press(cx, "cmd-w");
    app.wait_until(cx, "the closed tab", REPLY_TIMEOUT, |window, _| {
        labelled(window, "Title: Title before first").is_none()
    });

    for expected in [TITLE.to_owned(), format!("{TITLE} (Copy)")] {
        app.click(cx, "assistant-new");
        tab_label_is(&app, cx, "Query 2");
        app.send(cx, "Name sales report");
        app.wait_reply(cx, "I can help with this query");
        tab_label_is(&app, cx, &expected);
    }
    app.wait_until(cx, "both titles", REPLY_TIMEOUT, |_, _| {
        let names: BTreeSet<_> = tabs_of(&app, alpha).into_iter().map(|t| t.title).collect();
        let titled: Vec<_> = app
            .saved()
            .assistant
            .conversations
            .into_iter()
            .filter(|c| c.title == TITLE)
            .collect();
        names.contains(TITLE)
            && names.contains(&format!("{TITLE} (Copy)"))
            && titled.len() == 2
            && titled.iter().all(|c| c.title_source == Codex)
    });
    app.click(cx, "assistant-conversation-menu");
    app.choose(cx, "popup-menu", "Rename…");
    app.fill(cx, "rename-conversation-name", "Sales summary");
    app.click(cx, "rename-conversation");
    app.wait_gone(cx, "rename-conversation-name");
    tab_label_is(&app, cx, "Sales summary");
    app.click(cx, "assistant-conversation-menu");
    app.choose(cx, "popup-menu", "Regenerate Title");
    tab_label_is(&app, cx, &format!("{TITLE} (Copy)"));

    // A failed title keeps the default tab name.
    app.click(cx, "assistant-new");
    tab_label_is(&app, cx, "Query 2");
    app.send(cx, "Fail title generation");
    app.wait_reply(cx, "I can help with this query");
    app.wait_until(cx, "the failed title", REPLY_TIMEOUT, |_, _| {
        codex.marked("title-failure-once")
    });
    let conversation_of = |app: &TestApp, title: &str| {
        let tab = tabs_of(app, alpha).into_iter().find(|t| t.title == title)?;
        app.saved()
            .assistant
            .conversations
            .into_iter()
            .find(|c| c.tab_id == Some(tab.id))
    };
    app.wait_until(cx, "the default tab name", REPLY_TIMEOUT, |_, _| {
        conversation_of(&app, "Query 2").is_some_and(|c| c.title_source == Temporary)
    });
    // A user tab name stays after a later generated title.
    app.click(cx, "toggle-assistant");
    app.wait_gone(cx, "assistant-composer");
    app.context_menu_labelled(cx, "Query 2");
    app.choose(cx, "popup-menu", "Rename…");
    app.fill(cx, "rename-tab-name", "My SQL");
    app.click(cx, "rename-tab");
    app.wait_gone(cx, "rename-tab-name");
    tab_label_is(&app, cx, "My SQL");
    app.click(cx, "toggle-assistant");
    app.send(cx, "Continue the report");
    app.wait_until(cx, "the user tab name", REPLY_TIMEOUT, |_, _| {
        conversation_of(&app, "My SQL")
            .is_some_and(|c| c.title == "Title: Fail title generation" && c.title_source == Codex)
    });
}

#[gpui_kit::test]
fn a_conversation_outlives_its_tab_and_moves_with_its_next_tab(cx: &mut TestAppContext) {
    let Connections {
        app, alpha, beta, ..
    } = connections(cx, "SELECT 5;");
    app.wait_editor(cx, "SELECT 5;");
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.send(cx, "Report the tab SQL");
    app.wait_reply(cx, "Tab SQL: SELECT 5;");
    app.wait_idle(cx);
    app.wait_conversations(cx, &[("Title: Report the tab", Codex)]);
    let first = tab_of(&app, alpha).unwrap().id;
    assert_eq!(app.saved().assistant.conversations[0].tab_id, Some(first));

    app.hover_labelled(cx, "Query 1");
    app.click_labelled(cx, "Close Query 1");
    app.wait_until(cx, "the detached conversation", REPLY_TIMEOUT, |_, _| {
        let saved = app.saved();
        let c = &saved.assistant.conversations[0];
        c.tab_id.is_none()
            && c.detached_profile == Some(alpha)
            && saved.tabs.iter().all(|t| t.id != first)
    });
    app.show_threads(cx);
    app.update(cx, |window, cx| {
        let row = crate::support::labelled_starting(window, "")
            .into_iter()
            .find(|e| e.label().is_some_and(|l| l.contains("Alpha · Tab closed")))
            .expect("A row names the closed tab");
        crate::support::click_element(window, &row, cx);
    });
    app.show_conversation(cx);
    app.wait_reply(cx, "Tab SQL: SELECT 5;");
    // Reading a closed conversation does not open a tab.
    app.settle(cx);
    assert!(app.saved().assistant.conversations[0].tab_id.is_none());

    app.send(cx, "Continue the report");
    app.wait_reply(cx, "I can help with this query");
    let title = "Title: Report the tab";
    app.wait_until(cx, "the new tab", REPLY_TIMEOUT, |_, _| {
        let saved = app.saved();
        saved.assistant.conversations[0].tab_id.is_some_and(|id| {
            saved
                .tabs
                .iter()
                .any(|t| t.id == id && t.profile == Some(alpha) && t.title == title)
        })
    });
    let reopened = app.saved().assistant.conversations[0].tab_id.unwrap();

    app.context_menu_labelled(cx, title);
    app.choose_in_submenu(cx, "Move to Connection…", "Beta");
    app.wait_until(cx, "the moved conversation", REPLY_TIMEOUT, |_, _| {
        let saved = app.saved();
        saved.assistant.conversations[0].tab_id == Some(reopened)
            && saved
                .tabs
                .iter()
                .any(|t| t.id == reopened && t.profile == Some(beta))
    });
    app.show_threads(cx);
    app.wait_label_containing(cx, ", Beta");
    app.show_conversation(cx);

    // A tab with a conversation cannot start another one. The item ignores the click.
    app.context_menu_labelled(cx, title);
    app.choose(cx, "popup-menu", "Start Conversation");
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(
            window.try_find("popup-menu").is_some(),
            "Start Conversation was available"
        )
    });
    app.press(cx, "escape");
    app.wait_gone(cx, "popup-menu");

    app.click(cx, "assistant-conversation-menu");
    app.choose(cx, "popup-menu", "Delete…");
    app.click(cx, "confirm-delete-assistant-conversation");
    app.wait_until(cx, "the deleted conversation", REPLY_TIMEOUT, |_, _| {
        let saved = app.saved();
        saved.assistant.conversations.is_empty() && saved.tabs.iter().any(|t| t.id == reopened)
    });

    // The tab menu starts a conversation in a tab without one.
    app.click(cx, "toggle-assistant");
    app.wait_gone(cx, "assistant-composer");
    app.type_sql(cx, "SELECT 7;");
    app.context_menu_labelled(cx, title);
    app.choose(cx, "popup-menu", "Start Conversation");
    app.wait_gone(cx, "popup-menu");
    app.show_conversation(cx);
    app.send(cx, "Report the tab SQL");
    app.wait_reply(cx, "Tab SQL: SELECT 7;");
    app.wait_until(cx, "the conversation in this tab", REPLY_TIMEOUT, |_, _| {
        app.saved()
            .assistant
            .conversations
            .first()
            .is_some_and(|c| c.tab_id == Some(reopened))
    });
}
