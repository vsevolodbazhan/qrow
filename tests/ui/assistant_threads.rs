//! Conversation titles, the thread list, sign-in, and restarts.
use crate::support::assistant::{FakeCodex, REPLY_TIMEOUT};
use crate::support::{
    MemoryCredentials, TestApp, assert_tooltip_header_center, bounds_of, label, labelled_starting,
    labels, value,
};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::model::{
    AssistantTitleSource::{Codex, User},
    Workspace,
};
use qrow::ui::{OpenAbout, OpenSettings, Quit, ToggleAssistant, ToggleSidebar};
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
fn an_open_conversation_tooltip_follows_its_title_and_reply_state(cx: &mut TestAppContext) {
    let (app, codex) = launch(cx, |workspace, _| {
        workspace.settings.assistant.panel_width = 536.;
    });
    app.open_assistant(cx);
    app.send(cx, "Title before first reply Hold title generation");
    app.wait_until(cx, "the held reply and title", REPLY_TIMEOUT, |_, _| {
        codex.marked("first-reply-pending") && codex.marked("title-generation-pending")
    });
    app.show_threads(cx);
    let row_label = app.update(cx, |window, _| {
        label(window, "assistant-thread-synthetic-thread-1").unwrap()
    });
    app.hover_labelled(cx, &row_label);
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, cx| {
        assert_eq!(
            label(window, "status-tooltip-status").as_deref(),
            Some("Working")
        );
        assert_ne!(
            label(window, "status-tooltip-title").as_deref(),
            Some("Title: Title before first")
        );
        assert_tooltip_header_center(window, cx, "status-tooltip-status");
    });

    // The pointer remains on the same row through both changes.
    codex.mark("title-generation-release");
    app.wait_until(
        cx,
        "the generated tooltip title",
        REPLY_TIMEOUT,
        |window, _| {
            label(window, "status-tooltip-title").as_deref() == Some("Title: Title before first")
        },
    );
    codex.mark("first-reply-release");
    app.wait_until(
        cx,
        "the unread reply in the open tooltip",
        REPLY_TIMEOUT,
        |window, _| label(window, "status-tooltip-status").as_deref() == Some("Unread reply"),
    );
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
    app.choose(cx, "popup-menu", "Regenerate title");
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
        for conversation in &app.saved().assistant.conversations {
            assert!(
                !labels(window)
                    .iter()
                    .any(|label| label.contains(&conversation.thread_id)),
                "The thread list showed a Codex thread ID"
            );
        }
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
    app.fill_labelled(cx, "Search conversations", "LISTED");
    app.wait_until(cx, "one matching row", REPLY_TIMEOUT, |window, _| {
        labelled_starting(window, &row(GENERATED)).is_empty()
            && labelled_starting(window, &row("Listed title")).len() == 1
    });
    app.fill_labelled(cx, "Search conversations", "");
    app.wait_until(cx, "every row", REPLY_TIMEOUT, |window, _| {
        labelled_starting(window, &row(GENERATED)).len() == 1
            && labelled_starting(window, &row("Listed title")).len() == 1
    });
    app.context_menu_starting(cx, &row("Listed title"));
    app.choose(cx, "popup-menu", "Regenerate title");
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

fn delete_dialog_keeps_application_commands(cx: &mut TestAppContext, from_list: bool) {
    let (app, _codex) = launch(cx, |_, _| {});
    app.open_assistant(cx);
    app.send(cx, "Explain SELECT 1");
    app.wait_reply(cx, "I can help with this query");
    app.wait_idle(cx);
    if from_list {
        app.show_threads(cx);
    }
    for close in ["cancel", "escape", "delete"] {
        if from_list {
            app.context_menu_starting(cx, &row("Title: Explain SELECT 1"));
        } else {
            app.click(cx, "assistant-conversation-menu");
        }
        app.choose(cx, "popup-menu", "Delete…");
        app.wait_for(cx, "confirm-delete-assistant-conversation");
        match close {
            "cancel" => app.click(cx, "cancel-delete-assistant-conversation"),
            "escape" => app.press(cx, "escape"),
            _ => app.click(cx, "confirm-delete-assistant-conversation"),
        }
        app.wait_gone(cx, "confirm-delete-assistant-conversation");
        // Check the same focus path that macOS uses to enable menu commands.
        app.update(cx, |window, cx| {
            assert!(
                window.is_action_available(&OpenAbout, cx),
                "About after {close}"
            );
            assert!(
                window.is_action_available(&OpenSettings, cx),
                "Settings after {close}"
            );
            assert!(window.is_action_available(&Quit, cx), "Quit after {close}");
        });
        app.dispatch(cx, OpenAbout);
        app.wait_for(cx, "about-copyright");
        app.press(cx, "escape");
        app.wait_gone(cx, "about-copyright");
        if close != "delete" {
            assert_eq!(app.conversations().len(), 1);
        }
    }
    app.wait_until(cx, "no conversations", REPLY_TIMEOUT, |_, _| {
        app.conversations().is_empty()
    });
}

#[gpui_kit::test]
fn header_delete_dialog_keeps_application_commands(cx: &mut TestAppContext) {
    delete_dialog_keeps_application_commands(cx, false);
}

#[gpui_kit::test]
fn thread_list_delete_dialog_keeps_application_commands(cx: &mut TestAppContext) {
    delete_dialog_keeps_application_commands(cx, true);
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
        let search = bounds_of(window, "Search conversations");
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

#[gpui_kit::test]
fn a_thread_row_shows_its_state_beside_the_title_and_its_age_at_the_end(cx: &mut TestAppContext) {
    let (app, codex) = launch(cx, |workspace, _| {
        workspace.settings.assistant.panel_width = 536.;
    });
    app.open_assistant(cx);
    app.send(cx, "Title before first reply");
    app.wait_until(cx, "the held reply", REPLY_TIMEOUT, |_, _| {
        codex.marked("first-reply-pending")
    });
    app.show_threads(cx);
    app.wait_for(cx, "assistant-thread-dot-synthetic-thread-1");
    app.update(cx, |window, _| {
        let row = bounds_of(window, "assistant-thread-synthetic-thread-1");
        let dot = bounds_of(window, "assistant-thread-dot-synthetic-thread-1");
        let place = bounds_of(window, "assistant-thread-place-synthetic-thread-1");
        let age = bounds_of(window, "assistant-thread-age-synthetic-thread-1");
        assert!(
            dot.bottom() <= place.top(),
            "The state dot is not on the title line"
        );
        let difference = f32::from(age.center().y - place.center().y).abs();
        assert!(
            difference <= 1.,
            "The age is {difference}px off the connection line"
        );
        let gap = f32::from(dot.right() - age.right()).abs();
        assert!(gap <= 1., "The state dot and the age end {gap}px apart");
        assert!(age.right() <= row.right(), "The age is outside the row");
    });
    codex.mark("first-reply-release");
}
