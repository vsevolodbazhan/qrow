//! The assistant on Claude Code, and conversations of both harnesses in one
//! workspace, with the synthetic Codex and Claude Code of `tests/desktop/`.
use crate::support::assistant::{FakeClaude, FakeCodex, REPLY_TIMEOUT, composer_text, transcript};
use crate::support::{
    MemoryCredentials, TestApp, label, labelled, labels, offline_profile, present,
};
use gpui_kit::{ElementId, TestAppContext};
use qrow::model::{AssistantExecutionMode, AssistantHarness, SavedTab, Workspace};
use qrow::ui::{OpenSettings, ToggleAssistant};

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

/// Launches Qrow with Claude Code as the selected harness. `before` runs
/// before the launch, for example to create marker files.
fn launch_claude(
    cx: &mut TestAppContext,
    before: impl FnOnce(&FakeClaude),
) -> (TestApp, FakeClaude) {
    let (directory, codex) = FakeCodex::new();
    let claude = FakeClaude::beside(&directory);
    before(&claude);
    let workspace = Workspace {
        settings: claude.select(codex.settings()),
        ..with_connection()
    };
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    (app, claude)
}

fn has(window: &gpui_kit::Window, id: &str) -> bool {
    present(window, &ElementId::Name(id.to_owned().into()))
}

impl TestApp {
    /// Sends `message` with ⌘↩ while a turn runs and Send is hidden.
    fn send_during_turn(&self, cx: &mut TestAppContext, message: &str) {
        self.type_message(cx, message);
        self.press(cx, "cmd-enter");
        self.wait_until(cx, "the sent message", REPLY_TIMEOUT, |window, _| {
            composer_text(window).as_deref() == Some("")
        });
    }

    /// Opens the assistant pane and waits until Claude Code reports its
    /// models.
    fn open_claude(&self, cx: &mut TestAppContext) {
        if !self.update(cx, |window, _| has(window, "assistant-model")) {
            self.dispatch(cx, ToggleAssistant);
        }
        self.wait_claude(cx);
    }

    fn wait_claude(&self, cx: &mut TestAppContext) {
        self.wait_until(cx, "Claude Code to start", REPLY_TIMEOUT, |window, _| {
            label(window, "assistant-model").as_deref() == Some("Model: Default (recommended)")
        });
    }
}

#[gpui_kit::test]
fn claude_code_runs_a_turn_with_a_qrow_tool(cx: &mut TestAppContext) {
    let (app, claude) = launch_claude(cx, |_| {});
    app.open_claude(cx);
    app.update(cx, |window, _| {
        // Claude Code has no service tiers.
        assert!(!has(window, "assistant-tier"));
        assert_eq!(
            label(window, "assistant-reasoning").as_deref(),
            Some("Reasoning: Default")
        );
    });
    app.send(cx, "Write SELECT 1 into this tab");
    app.wait_editor(cx, "SELECT 1");
    app.wait_reply(cx, "I updated the SQL.");
    app.wait_idle(cx);
    assert!(
        claude
            .log()
            .iter()
            .any(|line| line.starts_with("tool result") && line.contains("\"sql_bytes\":8")),
        "The tool call did not succeed: {:?}",
        claude.log()
    );
    app.wait_until(cx, "the saved conversation", REPLY_TIMEOUT, |_, _| {
        let conversations = app.saved().assistant.conversations;
        conversations.len() == 1 && conversations[0].harness == AssistantHarness::Claude
    });
}

#[gpui_kit::test]
fn a_message_during_a_claude_code_turn_starts_after_it(cx: &mut TestAppContext) {
    let (app, claude) = launch_claude(cx, |_| {});
    app.open_claude(cx);
    app.send(cx, "Hold the reply");
    app.wait_reply(cx, "Holding the reply.");
    // Claude Code cannot add a message to a running turn. The message shows
    // at once and starts the next turn.
    app.send_during_turn(cx, "Then answer this");
    app.wait_reply(cx, "You: Then answer this");
    claude.mark("release");
    app.wait_reply(cx, "Claude reply: Then answer this");
    app.wait_idle(cx);
    app.update(cx, |window, _| {
        assert_eq!(
            transcript(window),
            [
                "You: Hold the reply",
                "Assistant: Holding the reply.",
                "You: Then answer this",
                "Assistant: Released.",
                "Assistant: Claude reply: Then answer this",
            ]
        );
    });
    assert_eq!(claude.turns(), ["Hold the reply", "Then answer this"]);
}

#[gpui_kit::test]
fn the_reply_messages_of_a_turn_show_as_paragraphs(cx: &mut TestAppContext) {
    // The held turn sends a second reply message at once.
    let (app, _claude) = launch_claude(cx, |claude| claude.mark("release"));
    app.open_claude(cx);
    app.send(cx, "Hold the reply");
    app.wait_reply(cx, "Released.");
    app.wait_idle(cx);
    app.update(cx, |window, _| {
        assert_eq!(
            transcript(window),
            [
                "You: Hold the reply",
                "Assistant: Holding the reply.\n\nReleased."
            ]
        );
    });
}

#[gpui_kit::test]
fn cancel_returns_a_waiting_message_to_the_message_field(cx: &mut TestAppContext) {
    let (app, claude) = launch_claude(cx, |_| {});
    app.open_claude(cx);
    app.send(cx, "Hold the reply");
    app.wait_reply(cx, "Holding the reply.");
    app.send_during_turn(cx, "Never sent");
    app.wait_reply(cx, "You: Never sent");
    app.click(cx, "assistant-stop");
    app.wait_until(cx, "the restored message", REPLY_TIMEOUT, |window, _| {
        composer_text(window).as_deref() == Some("Never sent")
    });
    app.wait_idle(cx);
    app.update(cx, |window, _| {
        let entries = transcript(window);
        assert!(
            !entries.iter().any(|entry| entry.contains("Never sent")),
            "The canceled message stayed in the transcript: {entries:?}"
        );
    });
    assert!(
        !claude
            .turns()
            .iter()
            .any(|turn| turn.contains("Never sent"))
    );
}

#[gpui_kit::test]
fn claude_code_sign_in_happens_in_terminal(cx: &mut TestAppContext) {
    let (app, claude) = launch_claude(cx, |claude| claude.mark("signed-out"));
    app.dispatch(cx, ToggleAssistant);
    app.wait_until(cx, "the sign-in command", REPLY_TIMEOUT, |window, _| {
        label(window, "assistant-claude-sign-in-command").as_deref() == Some("claude auth login")
    });
    // Qrow does not offer its own sign-in for Claude Code.
    app.update(cx, |window, _| {
        assert!(!labels(window).iter().any(|l| l.contains("ChatGPT")));
    });
    claude.unmark("signed-out");
    app.click(cx, "assistant-check-sign-in");
    app.wait_claude(cx);
    app.wait_gone(cx, "assistant-claude-sign-in-command");
}

#[gpui_kit::test]
fn a_conversation_of_the_other_harness_shows_its_saved_copy(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let claude = FakeClaude::beside(&directory);
    // Codex runs first. The synthetic Claude Code is configured before the
    // switch, so the switch cannot start an installed Claude Code.
    let mut settings = claude.select(codex.settings());
    settings.assistant.harness = AssistantHarness::Codex;
    let workspace = Workspace {
        settings,
        ..with_connection()
    };
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    app.send(cx, "Say hello");
    app.wait_reply(cx, "I can help with this query.");
    app.wait_idle(cx);
    let live = app.update(cx, |window, _| transcript(window));
    assert!(
        live.iter().any(|entry| entry == "You: Say hello"),
        "{live:?}"
    );

    // Select Claude Code in Settings.
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "harness");
    app.select(cx, "setting-assistant-harness", "Claude Code");
    // Settings apply as they change; ⌘W closes the dialog.
    app.press(cx, "cmd-w");
    app.wait_gone(cx, "reset-settings");
    app.wait_claude(cx);

    // The Codex conversation still shows, but it cannot get a message.
    app.wait_label(cx, "This conversation uses Codex.");
    app.update(cx, |window, _| {
        assert_eq!(transcript(window), live);
    });
    app.type_message(cx, "Not for Claude Code");
    app.click(cx, "assistant-send");
    app.settle(cx);
    assert_eq!(
        app.update(cx, |window, _| composer_text(window)).as_deref(),
        Some("Not for Claude Code")
    );
    app.show_threads(cx);
    app.wait_label(cx, "Codex");
    app.wait_saved(cx, "the saved conversation", |saved| {
        !saved.assistant.conversations.is_empty()
    });
    let thread = app.saved().assistant.conversations[0].thread_id.clone();
    assert_eq!(
        app.update(cx, |window, _| label(
            window,
            format!("assistant-thread-harness-{thread}")
        ))
        .as_deref(),
        Some("Codex")
    );

    // The notice switches back to Codex, and the conversation continues.
    app.show_conversation(cx);
    app.click(cx, "assistant-use-harness");
    app.open_assistant(cx);
    app.wait_gone(cx, "assistant-other-harness");
    app.wait_until(cx, "the saved harness", REPLY_TIMEOUT, |_, _| {
        app.saved().settings.assistant.harness == AssistantHarness::Codex
    });
}

#[gpui_kit::test]
fn a_claude_code_conversation_stays_read_only_while_claude_code_is_signed_out(
    cx: &mut TestAppContext,
) {
    let (app, claude) = launch_claude(cx, |_| {});
    app.open_claude(cx);
    app.send(cx, "Say hello");
    app.wait_reply(cx, "Claude reply: Say hello");
    app.wait_idle(cx);
    let live = app.update(cx, |window, _| transcript(window));

    // Claude Code signs out. After a restart, the pane asks for its sign-in.
    claude.mark("signed-out");
    let app = app.relaunch(cx);
    app.dispatch(cx, ToggleAssistant);
    app.wait_for(cx, "assistant-claude-sign-in-command");

    // Codex runs new conversations, and the Claude Code conversation shows
    // its saved copy without a send to Codex.
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "harness");
    app.select(cx, "setting-assistant-harness", "Codex");
    // Settings apply as they change; ⌘W closes the dialog.
    app.press(cx, "cmd-w");
    app.wait_gone(cx, "reset-settings");
    app.open_assistant(cx);
    app.wait_label(cx, "This conversation uses Claude Code.");
    app.update(cx, |window, _| assert_eq!(transcript(window), live));
    app.type_message(cx, "Not for Codex");
    app.click(cx, "assistant-send");
    app.settle(cx);
    assert_eq!(
        app.update(cx, |window, _| composer_text(window)).as_deref(),
        Some("Not for Codex")
    );

    // Use Claude Code leads to its sign-in.
    app.click(cx, "assistant-use-harness");
    app.wait_for(cx, "assistant-claude-sign-in-command");
}

#[gpui_kit::test]
fn a_web_search_shows_as_a_card_and_holds_automatic_runs(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let claude = FakeClaude::beside(&directory);
    let mut settings = claude.select(codex.settings());
    settings.assistant.web_search = true;
    settings.assistant.default_execution_mode = AssistantExecutionMode::RunAutomatically;
    let workspace = Workspace {
        settings,
        ..with_connection()
    };
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_claude(cx);
    app.send(cx, "Search the web and write SELECT 1 into this tab");
    // The conversation runs queries automatically, but not after a search.
    app.wait_approval(cx, "Run in Query 1 on Synthetic? SELECT 1");
    app.update(cx, |window, _| {
        assert!(has(window, "assistant-approval-web-search"))
    });
    // The card shows the search text and its links when it opens.
    let card = "Tool Call: Search the web: 2 links";
    app.wait_until(cx, card, REPLY_TIMEOUT, |window, _| {
        labelled(window, card).is_some()
    });
    app.click_labelled(cx, card);
    app.wait_until(cx, "the search detail", REPLY_TIMEOUT, |window, _| {
        labels(window).iter().any(|label| {
            label.starts_with("Search:\npostgres generate_series\n\nLinks:\nSet Returning Functions\nhttps://www.postgresql.org/docs/current/functions-srf.html")
        })
    });
    app.click(cx, "assistant-cancel-query");
    app.wait_reply(
        cx,
        "See https://www.postgresql.org/docs/current/functions-srf.html",
    );
    assert!(
        claude
            .log()
            .iter()
            .any(|line| line.starts_with("run result") && line.contains("approval_cancelled")),
        "{:?}",
        claude.log()
    );
}

#[gpui_kit::test]
fn web_search_asks_first_and_restarts_claude_code_with_it(cx: &mut TestAppContext) {
    let (app, claude) = launch_claude(cx, |_| {});
    app.open_claude(cx);
    app.send(cx, "Say hello");
    app.wait_reply(cx, "Claude reply: Say hello");
    app.wait_idle(cx);
    let tools = |arguments: &[String]| {
        let index = arguments
            .iter()
            .position(|argument| argument == "--tools")
            .unwrap();
        arguments[index + 1].clone()
    };
    assert_eq!(
        claude
            .session_arguments()
            .iter()
            .map(|arguments| tools(arguments))
            .collect::<Vec<_>>(),
        [""]
    );

    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "web search");
    app.click(cx, "assistant-web-search");
    // Cancel keeps web search off.
    app.wait_for(cx, "cancel-assistant-web-search");
    app.click(cx, "cancel-assistant-web-search");
    app.wait_gone(cx, "cancel-assistant-web-search");
    assert!(!app.saved().settings.assistant.web_search);
    app.click(cx, "assistant-web-search");
    app.wait_for(cx, "confirm-assistant-web-search");
    app.click(cx, "confirm-assistant-web-search");
    app.wait_saved(cx, "web search on", |saved| {
        saved.settings.assistant.web_search
    });
    // Settings apply as they change; ⌘W closes the dialog.
    app.press(cx, "cmd-w");
    app.wait_gone(cx, "reset-settings");

    // The next message starts Claude Code again with its search tool.
    app.wait_claude(cx);
    app.send_when_ready(cx, "Say again");
    app.wait_reply(cx, "Claude reply: Say again");
    assert!(
        claude
            .session_arguments()
            .iter()
            .any(|arguments| tools(arguments) == "WebSearch"),
        "{:?}",
        claude.session_arguments()
    );
}

#[gpui_kit::test]
fn turning_off_web_search_during_a_search_asks_and_cancels_its_card(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let claude = FakeClaude::beside(&directory);
    let mut settings = claude.select(codex.settings());
    settings.assistant.web_search = true;
    let workspace = Workspace {
        settings,
        ..with_connection()
    };
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_claude(cx);
    app.send(cx, "Search the web and hold");
    let running = "Tool Call: Search the web: Searching";
    app.wait_until(cx, running, REPLY_TIMEOUT, |window, _| {
        labelled(window, running).is_some()
    });

    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "web search");
    app.click(cx, "assistant-web-search");
    // The change stops the running turn, so Qrow asks first.
    app.wait_for(cx, "confirm-assistant-web-search");
    app.click(cx, "confirm-assistant-web-search");
    app.wait_saved(cx, "web search off", |saved| {
        !saved.settings.assistant.web_search
    });
    // Settings apply as they change; ⌘W closes the dialog.
    app.press(cx, "cmd-w");
    app.wait_gone(cx, "reset-settings");
    let cancelled = "Tool Call: Search the web: Cancelled";
    app.wait_until(cx, cancelled, REPLY_TIMEOUT, |window, _| {
        labelled(window, cancelled).is_some()
    });
}
