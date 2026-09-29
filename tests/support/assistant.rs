//! The assistant with the synthetic Codex app-server of
//! `tests/desktop/fake-codex.py`. The server keeps its state in the test's
//! workspace directory, and marker files there release its held turns.
use super::{TestApp, elements, label, labels, present};
use gpui_kit::{TestAppContext, Window};
use qrow::model::{AssistantTitleSource, Settings, Workspace};
use qrow::ui::ToggleAssistant;
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;

/// A synthetic reply takes well under a second; some turns wait for a marker.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

pub struct FakeCodex {
    executable: PathBuf,
    state: PathBuf,
}

impl FakeCodex {
    /// A new workspace directory with a Codex executable that starts the
    /// synthetic server.
    pub fn new() -> (TempDir, Self) {
        let directory = tempfile::tempdir().unwrap();
        // Hook snapshots disappear while their cached test binaries remain.
        let script = directory.path().join("fake-codex.py");
        std::fs::write(&script, include_str!("../desktop/fake-codex.py")).unwrap();
        let executable = directory.path().join("codex");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nQROW_DATA_DIR='{}' exec python3 '{}' \"$@\"\n",
                directory.path().display(),
                script.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let state = directory.path().join("fake-codex");
        std::fs::create_dir_all(&state).unwrap();
        (directory, Self { executable, state })
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Creates a marker file that the server waits for or reads.
    pub fn mark(&self, name: &str) {
        std::fs::write(self.state.join(name), "").unwrap();
    }

    /// Whether the server wrote the marker `name`.
    pub fn marked(&self, name: &str) -> bool {
        self.state.join(name).exists()
    }

    /// Settings that enable the assistant with this server.
    pub fn settings(&self) -> Settings {
        let mut settings = Settings::default();
        settings.assistant.enabled = true;
        settings.assistant.data_sharing_notice_version = 1;
        settings.assistant.codex_executable = Some(self.executable.display().to_string());
        settings
    }

    /// `workspace` with the assistant enabled with this server.
    pub fn workspace(&self, workspace: Workspace) -> Workspace {
        Workspace {
            settings: self.settings(),
            ..workspace
        }
    }
}

/// The transcript of the shown conversation: "You: …", "Assistant: …".
pub fn transcript(window: &Window) -> Vec<String> {
    elements(window)
        .into_iter()
        .filter(|element| {
            element
                .path()
                .last()
                .is_some_and(|id| format!("{id:?}").contains("assistant-entry-"))
        })
        .filter_map(|element| element.label().map(str::to_owned))
        .collect()
}

/// The text of the input inside the element `scope`, like the SQL editor.
pub fn text_in(window: &Window, scope: &str) -> Option<String> {
    let scope = gpui_kit::ElementId::Name(scope.to_owned().into());
    elements(window)
        .into_iter()
        .filter(|element| element.path().contains(&scope))
        .find_map(|element| element.value().map(str::to_owned))
}

pub fn editor_text(window: &Window) -> Option<String> {
    text_in(window, "sql-editor")
}

pub fn composer_text(window: &Window) -> Option<String> {
    text_in(window, "assistant-composer")
}

/// The accessibility label of the approval card: "Run in <tab> · <connection>? <SQL>".
pub fn approval(window: &Window) -> Option<String> {
    label(window, "assistant-query-approval")
}

impl TestApp {
    /// Opens the assistant pane and waits until Codex reports its models.
    pub fn open_assistant(&self, cx: &mut TestAppContext) {
        if !self.update(cx, |window, _| window_has(window, "assistant-model")) {
            self.dispatch(cx, ToggleAssistant);
        }
        self.wait_until(cx, "Codex to start", REPLY_TIMEOUT, |window, _| {
            label(window, "assistant-model")
                .is_some_and(|l| l.starts_with("Model: Synthetic Model"))
        });
    }

    /// Shows the conversation of a narrow pane instead of its thread list.
    pub fn show_conversation(&self, cx: &mut TestAppContext) {
        if self.update(cx, |window, _| {
            window_has(window, "assistant-back-to-thread")
        }) {
            self.click(cx, "assistant-back-to-thread");
        }
        self.wait_for(cx, "assistant-composer");
    }

    /// Shows the thread list of a narrow pane.
    pub fn show_threads(&self, cx: &mut TestAppContext) {
        if !self.update(cx, |window, _| window_has(window, "assistant-thread-list")) {
            self.click(cx, "assistant-toggle-threads");
        }
        self.wait_for(cx, "assistant-thread-list");
    }

    /// Types `message` into the message field.
    pub fn type_message(&self, cx: &mut TestAppContext, message: &str) {
        self.show_conversation(cx);
        self.click(cx, "assistant-composer");
        self.press(cx, "cmd-a");
        self.update(cx, |window, cx| {
            gpui_kit::test::TestWindowExt::input(window, message, cx)
        });
        let expected = message.to_owned();
        self.wait_until(
            cx,
            "the typed message",
            Duration::from_secs(10),
            |window, _| composer_text(window).as_deref() == Some(expected.as_str()),
        );
    }

    /// Sends `message` and waits until the message field is empty.
    pub fn send(&self, cx: &mut TestAppContext, message: &str) {
        self.type_message(cx, message);
        self.click(cx, "assistant-send");
        self.wait_until(
            cx,
            "the sent message",
            Duration::from_secs(10),
            |window, _| composer_text(window).as_deref() == Some(""),
        );
    }

    /// Waits until the transcript has an entry that contains `text`.
    pub fn wait_reply(&self, cx: &mut TestAppContext, text: &str) {
        self.wait_until(
            cx,
            &format!("the reply {text:?}"),
            REPLY_TIMEOUT,
            |window, _| transcript(window).iter().any(|entry| entry.contains(text)),
        );
    }

    /// Waits until the assistant of the shown conversation is not working.
    pub fn wait_idle(&self, cx: &mut TestAppContext) {
        self.wait_until(cx, "the end of the turn", REPLY_TIMEOUT, |window, _| {
            !window_has(window, "assistant-working") && window_has(window, "assistant-send")
        });
    }

    /// Waits until the SQL editor shows `sql`.
    pub fn wait_editor(&self, cx: &mut TestAppContext, sql: &str) {
        self.wait_until(
            cx,
            &format!("the SQL {sql:?}"),
            REPLY_TIMEOUT,
            |window, _| editor_text(window).as_deref() == Some(sql),
        );
    }

    /// Waits until the approval card shows `expected`.
    pub fn wait_approval(&self, cx: &mut TestAppContext, expected: &str) {
        self.wait_until(
            cx,
            &format!("the approval {expected:?}"),
            REPLY_TIMEOUT,
            |window, _| approval(window).as_deref() == Some(expected),
        );
    }

    /// Waits until an element has the label `expected`.
    pub fn wait_label(&self, cx: &mut TestAppContext, expected: &str) {
        self.wait_until(cx, expected, REPLY_TIMEOUT, |window, _| {
            labels(window).iter().any(|l| l == expected)
        });
    }
}

fn window_has(window: &Window, id: &str) -> bool {
    present(window, &gpui_kit::ElementId::Name(id.to_owned().into()))
}

impl TestApp {
    /// The saved conversations as (title, title source), sorted.
    pub fn conversations(&self) -> Vec<(String, AssistantTitleSource)> {
        let mut conversations: Vec<_> = self
            .saved()
            .assistant
            .conversations
            .into_iter()
            .map(|c| (c.title, c.title_source))
            .collect();
        conversations.sort_by(|a, b| a.0.cmp(&b.0));
        conversations
    }

    /// Waits until the saved conversations are `expected`, in any order.
    pub fn wait_conversations(
        &self,
        cx: &mut TestAppContext,
        expected: &[(&str, AssistantTitleSource)],
    ) {
        let mut expected: Vec<_> = expected.iter().map(|(t, s)| (t.to_string(), *s)).collect();
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        self.wait_until(
            cx,
            &format!("conversations {expected:?}"),
            REPLY_TIMEOUT,
            |_, _| self.conversations() == expected,
        );
    }

    /// Chooses `mode` in the menu of the send button.
    pub fn choose_send_mode(&self, cx: &mut TestAppContext, mode: &str) {
        self.update(cx, |window, cx| {
            gpui_kit::test::TestWindowExt::within(window, "assistant-send-mode").click("popup", cx)
        });
        self.choose(cx, "popup-menu", mode);
    }

    /// Sends `message` once Qrow accepts it. After a reconnect, Send shows as
    /// enabled while Codex starts, and Qrow ignores the press until then.
    pub fn send_when_ready(&self, cx: &mut TestAppContext, message: &str) {
        self.type_message(cx, message);
        let deadline = std::time::Instant::now() + REPLY_TIMEOUT;
        loop {
            self.click(cx, "assistant-send");
            self.settle(cx);
            if self.update(cx, |window, _| composer_text(window).as_deref() == Some("")) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Qrow did not send {message:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl TestApp {
    /// Waits until an element has a label that contains `text`.
    pub fn wait_label_containing(&self, cx: &mut TestAppContext, text: &str) {
        self.wait_until(cx, text, REPLY_TIMEOUT, |window, _| {
            labels(window).iter().any(|l| l.contains(text))
        });
    }
}
