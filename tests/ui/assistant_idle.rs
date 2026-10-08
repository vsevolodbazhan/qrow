//! Qrow stops an idle Codex when the assistant pane stays closed, and starts
//! it again when the pane opens.
use crate::support::assistant::{FakeCodex, REPLY_TIMEOUT, transcript};
use crate::support::{MemoryCredentials, TestApp, label, offline_profile, present};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ElementId, TestAppContext};
use qrow::model::{SavedTab, Workspace};
use qrow::ui::{CODEX_IDLE_TIMEOUT, ToggleAssistant};
use std::time::Duration;

/// Just less than the idle period.
const ALMOST_IDLE: Duration = CODEX_IDLE_TIMEOUT.saturating_sub(Duration::from_secs(1));

/// Qrow with an offline connection, an empty tab, and the synthetic Codex.
fn launch(cx: &mut TestAppContext) -> (TestApp, FakeCodex) {
    let (directory, codex) = FakeCodex::new();
    let profile = offline_profile("Synthetic");
    let tab = SavedTab::new(1, Some(profile.id));
    let workspace = codex.workspace(Workspace {
        profiles: vec![profile],
        tabs: vec![tab],
        ..Workspace::default()
    });
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    (app, codex)
}

fn has(window: &gpui_kit::Window, id: &str) -> bool {
    present(window, &ElementId::Name(id.to_owned().into()))
}

/// Closes the pane and waits until it is closed.
fn close_pane(app: &TestApp, cx: &mut TestAppContext) {
    app.dispatch(cx, ToggleAssistant);
    app.wait_gone(cx, "assistant-composer");
}

/// The only Codex process that runs.
fn running_codex(codex: &FakeCodex) -> u32 {
    let running: Vec<_> = codex
        .processes()
        .into_iter()
        .filter(|pid| FakeCodex::running(*pid))
        .collect();
    assert_eq!(running.len(), 1, "Codex processes {:?}", codex.processes());
    running[0]
}

/// Checks that the open pane uses the Codex process `pid`. After a stop, the
/// pane waits for a new Codex process.
fn assert_connected(app: &TestApp, cx: &mut TestAppContext, codex: &FakeCodex, pid: u32) {
    app.update(cx, |window, cx| {
        window.render_frame(cx);
        assert_eq!(
            label(window, "assistant-model").as_deref(),
            Some("Model: Synthetic Model"),
            "Codex stopped"
        );
    });
    assert_eq!(running_codex(codex), pid);
}

/// Opens the pane, checks that Codex did not stop, and closes the pane.
fn assert_connected_on_open(app: &TestApp, cx: &mut TestAppContext, codex: &FakeCodex, pid: u32) {
    app.click(cx, "toggle-assistant");
    app.wait_for(cx, "assistant-model");
    assert_connected(app, cx, codex, pid);
    close_pane(app, cx);
}

/// Waits until the Codex process `pid` has stopped and Qrow has reaped it.
fn wait_stopped(app: &TestApp, cx: &mut TestAppContext, pid: u32) {
    app.wait_until(cx, "the idle stop of Codex", REPLY_TIMEOUT, |_, _| {
        !FakeCodex::running(pid)
    });
}

#[gpui_kit::test]
fn codex_stops_after_the_pane_stays_closed(cx: &mut TestAppContext) {
    let (app, codex) = launch(cx);
    app.open_assistant(cx);
    app.send(cx, "Hello");
    app.wait_reply(cx, "I can help with this query");
    app.wait_idle(cx);
    let first = running_codex(&codex);

    close_pane(&app, cx);
    app.pass_time(cx, ALMOST_IDLE);
    assert_connected_on_open(&app, cx, &codex, first);
    app.pass_time(cx, CODEX_IDLE_TIMEOUT);
    wait_stopped(&app, cx, first);
    // A deliberate stop is not a failure.
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "toggle-assistant").as_deref(),
            Some("Toggle assistant")
        );
    });

    // The next open starts Codex like the first open, without Reconnect.
    app.dispatch(cx, ToggleAssistant);
    app.update(cx, |window, cx| {
        window.render_frame(cx);
        assert_eq!(
            label(window, "assistant-model").as_deref(),
            Some("Model: waiting for Codex")
        );
        assert!(!has(window, "assistant-reconnect"));
    });
    app.open_assistant(cx);
    let second = running_codex(&codex);
    assert_ne!(first, second);
    // The conversation stays selected, with its messages.
    app.update(cx, |window, _| {
        let entries = transcript(window);
        assert!(
            entries.iter().any(|entry| entry == "You: Hello"),
            "{entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.contains("I can help with this query")),
            "{entries:?}"
        );
    });
    // The new Codex process continues the conversation.
    app.send(cx, "Hello again");
    app.wait_until(cx, "the second reply", REPLY_TIMEOUT, |window, _| {
        transcript(window)
            .into_iter()
            .filter(|entry| entry.contains("I can help with this query"))
            .count()
            == 2
    });
    app.wait_idle(cx);
    assert_eq!(app.saved().assistant.conversations.len(), 1);
}

#[gpui_kit::test]
fn codex_keeps_running_while_the_pane_is_open(cx: &mut TestAppContext) {
    let (app, codex) = launch(cx);
    app.open_assistant(cx);
    let pid = running_codex(&codex);
    app.pass_time(cx, CODEX_IDLE_TIMEOUT * 2);
    assert_connected(&app, cx, &codex, pid);

    // Each close starts a new idle period.
    close_pane(&app, cx);
    app.pass_time(cx, ALMOST_IDLE);
    assert_connected_on_open(&app, cx, &codex, pid);
    app.pass_time(cx, ALMOST_IDLE);
    assert_connected_on_open(&app, cx, &codex, pid);
    app.pass_time(cx, CODEX_IDLE_TIMEOUT);
    wait_stopped(&app, cx, pid);
}

#[gpui_kit::test]
fn codex_keeps_running_while_a_conversation_starts_or_works(cx: &mut TestAppContext) {
    let (app, codex) = launch(cx);
    app.open_assistant(cx);
    app.click(cx, "assistant-new");
    let pid = running_codex(&codex);

    // Codex creates the conversation of the first message.
    codex.mark("hold-create");
    app.send(cx, "Title before first reply");
    app.wait_until(cx, "the held conversation", REPLY_TIMEOUT, |_, _| {
        codex.marked("create-pending")
    });
    close_pane(&app, cx);
    app.pass_time(cx, CODEX_IDLE_TIMEOUT * 2);
    assert_connected_on_open(&app, cx, &codex, pid);

    // The turn waits for its reply.
    codex.mark("release-create");
    app.wait_until(cx, "the held turn", REPLY_TIMEOUT, |_, _| {
        codex.marked("first-reply-pending")
    });
    app.pass_time(cx, CODEX_IDLE_TIMEOUT * 2);
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "toggle-assistant").as_deref(),
            Some("Toggle assistant, working")
        );
    });
    assert_connected_on_open(&app, cx, &codex, pid);

    // The turn ends, then Qrow reads its history. Wait for the notification
    // after that response before advancing the idle period on the test clock.
    codex.mark("notify-idle-history");
    codex.mark("first-reply-release");
    app.wait_until(cx, "the reply", REPLY_TIMEOUT, |window, _| {
        label(window, "toggle-assistant").as_deref() == Some("Toggle assistant, reply ready")
    });
    // The new tab follows the conversation title. Observe the notification
    // there, because waiting for the disk save also advances the test clock.
    app.wait_label_containing(cx, "Idle history loaded");
    app.pass_time(cx, ALMOST_IDLE);
    assert_eq!(running_codex(&codex), pid);
    app.pass_time(cx, Duration::from_secs(1));
    wait_stopped(&app, cx, pid);
    // The unseen reply stays unseen after the stop.
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "toggle-assistant").as_deref(),
            Some("Toggle assistant, reply ready")
        );
    });
    app.wait_until(cx, "the saved history title", REPLY_TIMEOUT, |_, _| {
        app.saved()
            .assistant
            .conversations
            .iter()
            .any(|conversation| conversation.title == "Idle history loaded")
    });
    app.click(cx, "toggle-assistant");
    app.open_assistant(cx);
    app.wait_reply(cx, "I can help with this query");
}
