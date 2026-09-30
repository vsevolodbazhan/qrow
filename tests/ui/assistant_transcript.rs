//! The transcript list: pages of older messages, and frames that render only
//! the views that changed, like the window does.
use crate::support::assistant::{FakeCodex, REPLY_TIMEOUT, composer_text, transcript};
use crate::support::{MemoryCredentials, TestApp, label, offline_profile, present};
use gpui_kit::{ElementId, Keystroke, TestAppContext, Window};
use qrow::model::{AssistantConversation, SavedTab, Workspace};
use qrow::ui::NewTab;
use std::time::{Duration, Instant};

/// A workspace whose tab has a saved conversation with three pages of history.
fn with_history(codex: &FakeCodex) -> Workspace {
    let thread = "synthetic-history-1";
    codex.save_rollout(thread);
    let tab = SavedTab::new(1, None);
    let mut conversation = AssistantConversation::new(thread, Default::default());
    conversation.tab_id = Some(tab.id);
    let mut workspace = codex.workspace(Workspace {
        tabs: vec![tab],
        ..Workspace::default()
    });
    workspace.assistant.conversations.push(conversation);
    workspace
}

fn history(turns: std::ops::RangeInclusive<u32>) -> Vec<String> {
    turns
        .flat_map(|turn| {
            [
                format!("You: History question {turn}"),
                format!("Assistant: History reply {turn}"),
            ]
        })
        .collect()
}

#[gpui_kit::test]
fn older_messages_load_above_the_conversation(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let workspace = with_history(&codex);
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    app.wait_reply(cx, "History reply 6");
    app.wait_for(cx, "assistant-load-older");
    app.update(cx, |window, _| {
        assert_eq!(transcript(window), history(5..=6))
    });

    app.click(cx, "assistant-load-older");
    app.wait_reply(cx, "History reply 4");
    app.update(cx, |window, _| {
        assert_eq!(transcript(window), history(3..=6))
    });

    // The last page has no older messages.
    app.click(cx, "assistant-load-older");
    app.wait_reply(cx, "History reply 2");
    app.wait_gone(cx, "assistant-load-older");
    app.update(cx, |window, _| {
        assert_eq!(transcript(window), history(1..=6))
    });
}

/// Draws a frame without a full refresh, so that a view that did not change
/// shows its last frame again.
fn draw(window: &mut Window, cx: &mut gpui_kit::App) {
    window.draw(cx).clear(cx);
}

/// Types `text` into the focused input, one frame for each character.
fn type_text(app: &TestApp, cx: &mut TestAppContext, text: &str) {
    app.update(cx, |window, cx| {
        for character in text.chars() {
            let mut key = Keystroke::parse(&character.to_string()).unwrap();
            key.key_char = Some(character.to_string());
            window.dispatch_keystroke(key, cx);
            draw(window, cx);
        }
    });
}

/// Waits until `done` is true, with frames that do not refresh the window.
fn wait_drawn(
    app: &TestApp,
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&Window) -> bool,
) {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        cx.executor().advance_clock(Duration::from_millis(10));
        cx.run_until_parked();
        if app.update(cx, |window, cx| {
            draw(window, cx);
            done(window)
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for {what}. Labels: {:?}",
            app.update(cx, |window, _| crate::support::labels(window))
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui_kit::test]
fn the_pane_and_the_workspace_stay_current_between_their_own_changes(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let profile = offline_profile("Synthetic");
    let workspace = codex.workspace(Workspace {
        tabs: vec![SavedTab::new(1, Some(profile.id))],
        profiles: vec![profile],
        ..Workspace::default()
    });
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    app.click(cx, "assistant-composer");

    // The message field shows each typed character.
    type_text(&app, cx, "Stream a long reply");
    app.update(cx, |window, _| {
        assert_eq!(
            composer_text(window).as_deref(),
            Some("Stream a long reply")
        )
    });

    // A reply streams into the pane without a change of the workspace.
    app.update(cx, |window, cx| {
        gpui_kit::test::TestWindowExt::click(window, "assistant-send", cx)
    });
    wait_drawn(&app, cx, "the streamed reply", |window| {
        transcript(window)
            .iter()
            .any(|entry| entry.contains("End of Stream a long reply"))
    });
    wait_drawn(&app, cx, "the end of the turn", |window| {
        !present(window, &ElementId::from("assistant-working"))
    });
    // Codex names the conversation. Only the workspace state changes.
    wait_drawn(&app, cx, "the generated title", |window| {
        label(window, "assistant-conversation-title").as_deref()
            == Some("Conversation title: Title: Stream a long")
    });

    // A new tab changes the workspace and the pane: its conversation is new,
    // and the message field shows the empty draft of the tab.
    app.dispatch(cx, NewTab);
    wait_drawn(&app, cx, "the new tab", |window| {
        present(window, &ElementId::from("Close Query 2"))
            || crate::support::labelled(window, "Query 2").is_some()
    });
    wait_drawn(&app, cx, "the conversation of the new tab", |window| {
        label(window, "assistant-conversation-title").as_deref()
            == Some("Conversation title: New conversation")
            && composer_text(window).as_deref() == Some("")
            && transcript(window).is_empty()
    });
}
