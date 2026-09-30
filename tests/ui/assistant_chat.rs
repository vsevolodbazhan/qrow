//! Assistant chat in the docked pane, with the synthetic Codex server.
use crate::support::assistant::{FakeCodex, REPLY_TIMEOUT, composer_text};
use crate::support::{
    MemoryCredentials, TestApp, bounds_of, label, labelled, labels, offline_profile, present,
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ElementId, ScrollDelta, TestAppContext, point, px};
use qrow::model::{SavedTab, Workspace};
use qrow::ui::{Quit, ToggleAssistant};
use std::time::Duration;

fn launch(cx: &mut TestAppContext, workspace: Workspace) -> (TestApp, FakeCodex) {
    let (directory, codex) = FakeCodex::new();
    let workspace = codex.workspace(workspace);
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    (app, codex)
}

/// A workspace with an offline connection and an empty tab.
fn with_connection() -> Workspace {
    let profile = offline_profile("Synthetic");
    let tab = SavedTab::new(1, Some(profile.id));
    Workspace {
        profiles: vec![profile],
        tabs: vec![tab],
        ..Workspace::default()
    }
}

fn has(window: &gpui_kit::Window, id: &str) -> bool {
    present(window, &ElementId::Name(id.to_owned().into()))
}

#[gpui_kit::test]
fn controls_wait_until_codex_starts(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    // The synthetic server does not answer `initialize` before the release.
    codex.mark("hold-initialize");
    let workspace = codex.workspace(Workspace::default());
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.dispatch(cx, ToggleAssistant);
    app.update(cx, |window, cx| {
        window.render_frame(cx);
        assert!(
            has(window, "add-connection"),
            "Opening the assistant hid the Connections sidebar"
        );
        for (id, expected) in [
            ("assistant-model", "Model: waiting for Codex"),
            ("assistant-reasoning", "Reasoning: waiting for Codex"),
            ("assistant-tier", "Service tier: waiting for Codex"),
        ] {
            assert_eq!(label(window, id).as_deref(), Some(expected));
            // A control that waits for Codex does not open its menu.
            window.click(id, cx);
            assert!(
                window.try_find("popup-menu").is_none(),
                "{id} opened while Codex started"
            );
        }
        assert!(!labels(window).iter().any(|l| l.contains("Starting Codex")));
    });
    codex.mark("initialize-release");
    app.open_assistant(cx);
    for id in [
        "assistant-new",
        "assistant-conversation-menu",
        "assistant-toggle-threads",
    ] {
        app.wait_for(cx, id);
    }
    // The conversation list toggles in the pane.
    let list_open = app.update(cx, |window, _| has(window, "assistant-thread-list"));
    app.click(cx, "assistant-toggle-threads");
    if list_open {
        app.wait_gone(cx, "assistant-thread-list");
    } else {
        app.wait_for(cx, "assistant-thread-list");
    }
    app.show_conversation(cx);
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "assistant-send").as_deref(),
            Some("Send · Ask")
        );
    });
}

#[gpui_kit::test]
fn composer_controls_show_the_selected_codex_settings(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let mut workspace = codex.workspace(Workspace::default());
    workspace.settings.assistant.service_tier = Some("retired".into());
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    // Codex no longer offers the saved tier, so its default replaces it.
    app.update(cx, |window, _| {
        for (id, expected) in [
            ("assistant-model", "Model: Synthetic Model"),
            ("assistant-reasoning", "Reasoning: Medium"),
            ("assistant-tier", "Service tier: Default"),
        ] {
            assert_eq!(label(window, id).as_deref(), Some(expected));
        }
    });
    app.wait_until(cx, "the replaced tier", Duration::from_secs(10), |_, _| {
        let saved = app.saved().settings.assistant;
        saved.model.as_deref() == Some("synthetic-model") && saved.service_tier.is_none()
    });
    app.click(cx, "assistant-tier");
    app.choose(cx, "popup-menu", "Fast");
    app.wait_until(cx, "the Fast tier", Duration::from_secs(10), |window, _| {
        label(window, "assistant-tier").as_deref() == Some("Service tier: Fast")
    });
    app.wait_until(cx, "the saved tier", Duration::from_secs(10), |_, _| {
        app.saved().settings.assistant.service_tier.as_deref() == Some("fast")
    });
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "assistant-model").as_deref(),
            Some("Model: Synthetic Model")
        );
    });
}

#[gpui_kit::test]
fn a_hidden_turn_reports_its_state_on_the_toggle(cx: &mut TestAppContext) {
    let (app, codex) = launch(cx, Workspace::default());
    app.open_assistant(cx);
    // The synthetic server holds this turn until the release.
    app.send(cx, "Return to the latest message while hidden");
    app.wait_for(cx, "assistant-working");
    // Quit asks before it stops a working assistant.
    app.dispatch(cx, Quit);
    app.wait_for(cx, "keep-working");
    app.click(cx, "keep-working");
    app.wait_gone(cx, "keep-working");

    app.dispatch(cx, ToggleAssistant);
    app.wait_gone(cx, "assistant-composer");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "toggle-assistant").as_deref(),
            Some("Toggle Assistant, working")
        );
    });
    codex.mark("latest-release");
    app.wait_until(cx, "the reply-ready toggle", REPLY_TIMEOUT, |window, _| {
        label(window, "toggle-assistant").as_deref() == Some("Toggle Assistant, reply ready")
    });
    app.click(cx, "toggle-assistant");
    app.wait_reply(cx, "I can help with this query");
}

#[gpui_kit::test]
fn toolbar_run_keeps_the_draft_and_cmd_enter_sends_it(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, with_connection());
    app.open_assistant(cx);
    app.type_message(cx, "Keep this draft");
    app.click(cx, "run");
    app.settle(cx);
    app.update(cx, |window, _| {
        assert_eq!(
            composer_text(window).as_deref(),
            Some("Keep this draft"),
            "Toolbar Run sent the draft"
        );
    });
    app.type_message(
        cx,
        "Help me understand how `SELECT 1` behaves in the currently selected query tab and explain its result",
    );
    // Cmd+Enter sends in the message field.
    app.press(cx, "cmd-enter");
    app.wait_until(
        cx,
        "the sent message",
        Duration::from_secs(10),
        |window, _| composer_text(window).as_deref() == Some(""),
    );
    app.wait_reply(cx, "I can help with this query");
}

#[gpui_kit::test]
fn the_append_tool_writes_sql_and_undo_removes_it_in_one_step(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, with_connection());
    app.open_assistant(cx);
    app.send(cx, "Write SELECT 1 into this tab");
    app.wait_editor(cx, "SELECT 1");
    app.wait_reply(cx, "I updated the SQL.");

    // A tool call is a full-width card that starts collapsed.
    let card = "Tool call: Append query";
    app.wait_until(cx, card, REPLY_TIMEOUT, |window, _| {
        labelled(window, card).is_some()
    });
    app.update(cx, |window, _| {
        assert!(
            !labels(window).iter().any(|l| l == "Tab: Query 1"),
            "The collapsed card showed its tab"
        );
        let card = f32::from(bounds_of(window, card).size.width);
        let composer = f32::from(bounds_of(window, "assistant-composer").size.width);
        assert!(
            card >= composer * 0.9,
            "The card is {card} wide, the composer {composer}"
        );
    });
    app.click_labelled(cx, card);
    app.wait_label(cx, "Tab: Query 1");
    app.update(cx, |window, _| {
        assert!(labels(window).iter().any(|l| l.starts_with("Arguments:")))
    });
    app.click_labelled(cx, card);
    app.wait_until(
        cx,
        "the collapsed card",
        Duration::from_secs(10),
        |window, _| !labels(window).iter().any(|l| l == "Tab: Query 1"),
    );

    // A user edit does not merge with the assistant edit.
    app.click(cx, "sql-editor");
    app.press(cx, "cmd-down");
    app.update(cx, |window, cx| window.input("a", cx));
    app.wait_editor(cx, "SELECT 1a");
    app.press(cx, "cmd-z");
    app.wait_editor(cx, "SELECT 1");
    app.press(cx, "cmd-z");
    app.wait_editor(cx, "");

    app.send(cx, "Write SELECT 1 into this tab");
    app.wait_editor(cx, "SELECT 1");
    app.wait_idle(cx);
    app.send(cx, "Write SELECT 2 into this tab");
    app.wait_editor(cx, "SELECT 1;\n\nSELECT 2");
}

fn jump_visible(app: &TestApp, cx: &mut TestAppContext) -> bool {
    app.update(cx, |window, _| {
        window.try_find("assistant-jump-latest").is_some()
    })
}

fn scroll_transcript(app: &TestApp, cx: &mut TestAppContext, pixels: f32) {
    app.update(cx, |window, cx| {
        window.scroll(
            "assistant-transcript",
            ScrollDelta::Pixels(point(px(0.), px(pixels))),
            cx,
        );
    });
    app.settle(cx);
}

#[gpui_kit::test]
fn the_transcript_jumps_to_the_latest_message(cx: &mut TestAppContext) {
    let (app, codex) = launch(cx, Workspace::default());
    app.open_assistant(cx);
    app.send(cx, "Show many lines");
    app.wait_reply(cx, "Line 40");
    app.wait_idle(cx);
    assert!(!jump_visible(&app, cx));
    scroll_transcript(&app, cx, 1200.);
    app.wait_for(cx, "assistant-jump-latest");
    app.click(cx, "assistant-jump-latest");
    app.wait_gone(cx, "assistant-jump-latest");

    // A new message returns to the end of the transcript.
    scroll_transcript(&app, cx, 1200.);
    app.wait_for(cx, "assistant-jump-latest");
    app.send(cx, "Return to the latest message");
    app.wait_for(cx, "assistant-working");
    codex.mark("latest-release");
    app.wait_idle(cx);
    // Only this reply has this text in the conversation.
    app.wait_reply(cx, "I can help with this query");
    app.wait_gone(cx, "assistant-jump-latest");

    app.dispatch(cx, ToggleAssistant);
    app.wait_gone(cx, "assistant-toggle-threads");
}
