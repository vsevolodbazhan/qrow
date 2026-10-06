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

/// The parts of the reply "Stream a reply in bursts", and the reply.
fn bursts() -> (usize, String) {
    let burst = |number: usize| -> String {
        (0..20)
            .map(|index| format!("burst{number}word{index} "))
            .collect()
    };
    let reply = (0..3).map(burst).collect::<String>() + "End of Stream a reply in bursts";
    (burst(0).len(), reply)
}

/// The texts of the reply that the transcript showed until the whole reply
/// showed, each one time.
fn shown_steps(app: &TestApp, cx: &mut TestAppContext, reply: &str) -> Vec<String> {
    let mut steps: Vec<String> = Vec::new();
    app.wait_until(cx, "the whole reply", REPLY_TIMEOUT, |window, _| {
        let Some(shown) = transcript(window)
            .into_iter()
            .find_map(|entry| entry.strip_prefix("Assistant: ").map(str::to_owned))
        else {
            return false;
        };
        let done = shown == reply;
        if !shown.is_empty() && steps.last() != Some(&shown) {
            steps.push(shown);
        }
        done
    });
    steps
}

#[gpui_kit::test]
fn a_streamed_reply_shows_word_by_word(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let workspace = codex.workspace(Workspace::default());
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    // The reveal steps use the test clock.
    cx.update(|cx| cx.set_reduce_motion(false));
    app.open_assistant(cx);
    app.send(cx, "Stream a reply in bursts");
    let (first_burst, reply) = bursts();
    let steps = shown_steps(&app, cx, &reply);

    // Each step adds whole words to the text before it.
    for (before, after) in std::iter::once("")
        .chain(steps.iter().map(String::as_str))
        .zip(&steps)
    {
        assert!(
            after.starts_with(before) && after.len() > before.len(),
            "{steps:?}"
        );
        assert!(
            after.ends_with(' ')
                || reply[after.len()..].is_empty()
                || reply[after.len()..].starts_with(' '),
            "a step ends in a word: {after:?}"
        );
    }
    // Codex sent the first 20 words at once. They show in several steps.
    let first = steps
        .iter()
        .filter(|step| step.len() <= first_burst)
        .count();
    assert!(
        first >= 4,
        "the first part showed in {first} steps: {steps:?}"
    );
}

#[gpui_kit::test]
fn a_streamed_reply_shows_at_once_with_reduced_motion(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let workspace = codex.workspace(Workspace::default());
    // TestApp turns on reduced motion.
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    app.send(cx, "Stream a reply in bursts");
    let (first_burst, reply) = bursts();
    let steps = shown_steps(&app, cx, &reply);

    // Each part shows when it arrives.
    assert_eq!(steps[0].len(), first_burst, "{steps:?}");
    assert!(steps.len() <= 4, "{steps:?}");
}
