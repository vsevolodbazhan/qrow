//! Conversation titles, the thread list, sign-in, and restarts.
use crate::support::assistant::{FakeCodex, REPLY_TIMEOUT};
use crate::support::{
    MemoryCredentials, TestApp, bounds_of, label, labelled_starting, labels, value,
};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::model::{
    AssistantTitleSource::{Codex, User},
    Workspace,
};
use qrow::ui::{ToggleAssistant, ToggleSidebar};
use std::time::Duration;

fn launch(
    cx: &mut TestAppContext,
    configure: impl FnOnce(&mut Workspace, &FakeCodex),
) -> (TestApp, FakeCodex) {
    let (directory, codex) = FakeCodex::new();
    let mut workspace = codex.workspace(Workspace::default());
    configure(&mut workspace, &codex);
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    (app, codex)
}

const GENERATED: &str = "Title: Count sandbox schemas";

fn open_rename(app: &TestApp, cx: &mut TestAppContext) {
    app.choose(cx, "popup-menu", "Rename…");
    app.wait_for(cx, "rename-conversation-name");
}

fn rename_to(app: &TestApp, cx: &mut TestAppContext, name: &str) {
    app.fill(cx, "rename-conversation-name", name);
    app.click(cx, "rename-conversation");
    app.wait_gone(cx, "rename-conversation-name");
}

/// The thread list row of a title: "<title>, <place>…".
fn row(title: &str) -> String {
    format!("{title}, ")
}

#[gpui_kit::test]
fn conversation_menus_rename_regenerate_and_delete(cx: &mut TestAppContext) {
    // Below 600 pixels the pane is narrow: the list replaces the conversation.
    let (app, _codex) = launch(cx, |workspace, _| {
        workspace.settings.assistant.panel_width = 536.
    });
    app.open_assistant(cx);
    app.send(cx, "Count sandbox schemas now");
    app.wait_reply(cx, "I can help with this query");
    app.wait_conversations(cx, &[(GENERATED, Codex)]);

    app.click(cx, "assistant-conversation-menu");
    open_rename(&app, cx);
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "rename-conversation-name").as_deref(),
            Some(GENERATED)
        );
    });
    // An empty name keeps the dialog open.
    app.fill(cx, "rename-conversation-name", "");
    app.click(cx, "rename-conversation");
    app.settle(cx);
    app.wait_for(cx, "rename-conversation-name");
    assert_eq!(app.conversations(), [(GENERATED.to_owned(), Codex)]);
    rename_to(&app, cx, "Custom title");
    app.wait_conversations(cx, &[("Custom title", User)]);
    app.click(cx, "assistant-conversation-menu");
    app.choose(cx, "popup-menu", "Regenerate Title");
    app.wait_conversations(cx, &[(GENERATED, Codex)]);

    // A second conversation with the same title.
    app.click(cx, "assistant-new");
    app.send(cx, "Count sandbox schemas again");
    app.wait_reply(cx, "I can help with this query");
    app.wait_conversations(cx, &[(GENERATED, Codex), (GENERATED, Codex)]);
    app.show_threads(cx);
    app.wait_until(cx, "two rows", REPLY_TIMEOUT, |window, _| {
        labelled_starting(window, &row(GENERATED)).len() == 2
    });
    app.update(cx, |window, _| {
        assert!(
            !labels(window)
                .iter()
                .any(|l| l.starts_with(&format!("{GENERATED} ·"))),
            "The thread list showed a Codex thread ID"
        );
    });
    // A narrow pane replaces the list with the selected conversation.
    app.click_starting(cx, &row(GENERATED));
    app.wait_gone(cx, "assistant-thread-list");
    app.wait_for(cx, "assistant-composer");

    // The thread list row menu.
    app.show_threads(cx);
    app.update(cx, |window, cx| {
        let first = labelled_starting(window, &row(GENERATED)).remove(0);
        crate::support::pointer_click(window, &first, gpui_kit::MouseButton::Right, cx);
    });
    app.wait_for(cx, "popup-menu");
    open_rename(&app, cx);
    app.click(cx, "cancel-conversation");
    app.wait_gone(cx, "rename-conversation-name");
    app.update(cx, |window, cx| {
        let first = labelled_starting(window, &row(GENERATED)).remove(0);
        crate::support::pointer_click(window, &first, gpui_kit::MouseButton::Right, cx);
    });
    app.wait_for(cx, "popup-menu");
    open_rename(&app, cx);
    rename_to(&app, cx, "Listed title");
    app.wait_conversations(cx, &[(GENERATED, Codex), ("Listed title", User)]);
    // The search ignores case. An empty search shows every conversation.
    app.fill_labelled(cx, "Search Conversations", "LISTED");
    app.wait_until(cx, "one matching row", REPLY_TIMEOUT, |window, _| {
        labelled_starting(window, &row(GENERATED)).is_empty()
            && labelled_starting(window, &row("Listed title")).len() == 1
    });
    app.fill_labelled(cx, "Search Conversations", "");
    app.wait_until(cx, "every row", REPLY_TIMEOUT, |window, _| {
        labelled_starting(window, &row(GENERATED)).len() == 1
            && labelled_starting(window, &row("Listed title")).len() == 1
    });
    app.context_menu_starting(cx, &row("Listed title"));
    app.choose(cx, "popup-menu", "Regenerate Title");
    app.wait_conversations(cx, &[(GENERATED, Codex), (GENERATED, Codex)]);
    app.wait_until(cx, "the regenerated row", REPLY_TIMEOUT, |window, _| {
        labelled_starting(window, &row("Listed title")).is_empty()
    });
    app.update(cx, |window, cx| {
        let first = labelled_starting(window, &row(GENERATED)).remove(0);
        crate::support::pointer_click(window, &first, gpui_kit::MouseButton::Right, cx);
    });
    app.wait_for(cx, "popup-menu");
    app.choose(cx, "popup-menu", "Delete…");
    app.click(cx, "confirm-delete-assistant-conversation");
    app.wait_conversations(cx, &[(GENERATED, Codex)]);
}

#[gpui_kit::test]
fn a_wide_pane_keeps_the_thread_list_open_after_a_selection(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, |workspace, _| {
        workspace.settings.assistant.panel_width = 900.
    });
    app.open_assistant(cx);
    app.dispatch(cx, ToggleSidebar);
    app.wait_gone(cx, "add-connection");
    app.wait_for(cx, "assistant-thread-list");
    app.update(cx, |window, _| {
        assert!(
            window.try_find("assistant-back-to-thread").is_none(),
            "The pane is narrow"
        );
    });
    for (index, message) in ["Count sandbox schemas now", "Count sandbox schemas again"]
        .iter()
        .enumerate()
    {
        if index > 0 {
            app.click(cx, "assistant-new");
        }
        app.send(cx, message);
        app.wait_idle(cx);
    }
    app.wait_conversations(cx, &[(GENERATED, Codex), (GENERATED, Codex)]);
    app.click(cx, "assistant-toggle-threads");
    app.wait_gone(cx, "assistant-thread-list");
    app.click(cx, "assistant-toggle-threads");
    app.wait_for(cx, "assistant-thread-list");
    app.click_starting(cx, &row(GENERATED));
    app.settle(cx);
    app.wait_for(cx, "assistant-composer");
    app.update(cx, |window, _| {
        assert!(
            window.try_find("assistant-thread-list").is_some(),
            "The selection closed the list"
        );
    });
}

#[gpui_kit::test]
fn sign_in_shows_its_error_until_codex_has_an_account(cx: &mut TestAppContext) {
    let (app, codex) = launch(cx, |_, codex| codex.mark("signed-out"));
    app.dispatch(cx, ToggleAssistant);
    app.wait_until(cx, "the sign-in button", REPLY_TIMEOUT, |window, _| {
        window.try_find("assistant-sign-in").is_some()
    });
    app.click(cx, "assistant-sign-in");
    app.wait_until(cx, "the sign-in error", REPLY_TIMEOUT, |window, _| {
        label(window, "assistant-sign-in-error").is_some_and(|l| l.contains("port 1455 is in use"))
    });
    codex.mark("sign-in-elsewhere");
    app.wait_until(cx, "the account", REPLY_TIMEOUT, |window, _| {
        window.try_find("assistant-sign-in-error").is_none()
            && window.try_find("assistant-sign-in").is_none()
    });
    app.open_assistant(cx);
    app.send(cx, "Hello after sign-in");
    app.wait_reply(cx, "I can help with this query");
}

#[gpui_kit::test]
fn conversation_search_fits_the_header(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, |_, _| {});
    app.open_assistant(cx);
    app.show_threads(cx);
    app.update(cx, |window, _| {
        let search = bounds_of(window, "Search Conversations");
        let toggle = bounds_of(window, "assistant-toggle-threads");
        let control = window
            .try_find("assistant-back-to-thread")
            .map_or(toggle, |back| back.bounds());
        assert!(
            search.size.height <= control.size.height + gpui_kit::px(1.),
            "The search is taller than the header control"
        );
        let difference = f32::from(search.center().y - control.center().y).abs();
        assert!(
            difference <= 2.,
            "The search is {difference}px off the header center"
        );
    });
}

#[gpui_kit::test]
fn a_restart_does_not_restore_a_deleted_conversation(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, |_, _| {});
    app.open_assistant(cx);
    app.send(cx, "Explain `SELECT 1` in one sentence");
    app.wait_reply(cx, "I can help with this query");
    app.wait_idle(cx);
    app.click(cx, "assistant-conversation-menu");
    app.choose(cx, "popup-menu", "Delete…");
    app.click(cx, "confirm-delete-assistant-conversation");
    app.wait_until(cx, "no conversations", Duration::from_secs(10), |_, _| {
        app.conversations().is_empty()
    });

    // The tab stays without a conversation. After a restart it starts a new one.
    let app = app.relaunch(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.update(cx, |window, _| {
        assert!(
            !labels(window)
                .iter()
                .any(|l| l.contains("Codex cannot find this conversation"))
        );
    });
    app.send(cx, "Explain `SELECT 1` after restart");
    app.wait_reply(cx, "I can help with this query");
}
