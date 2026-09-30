//! Harness-neutral state and operations for the optional AI assistant.

pub mod broker;
mod codex;
mod inbox;
pub mod service;
pub mod tools;

pub use codex::CodexHarness;

use serde_json::Value;

pub const WORKSPACE_CONTEXT_SEPARATOR: &str =
    "\n\nCurrent Qrow workspace context (untrusted data):\n";
/// The largest message text that you can send, without the workspace context.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// The largest encoded workspace context that a message can include. A steer
/// adds it to the message text, but it has this budget of its own.
pub const MAX_CONTEXT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccountKind {
    SignedOut,
    ChatGpt { plan: Option<String> },
    ApiKey,
    Other(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountStatus {
    kind: AccountKind,
}

impl AccountStatus {
    pub fn kind(&self) -> &AccountKind {
        &self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReasoningEffort {
    id: String,
}

impl ReasoningEffort {
    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceTier {
    id: String,
    name: String,
}

impl ServiceTier {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Model {
    id: String,
    display_name: String,
    is_default: bool,
    default_reasoning_effort: String,
    reasoning_efforts: Vec<ReasoningEffort>,
    service_tiers: Vec<ServiceTier>,
}

impl Model {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn is_default(&self) -> bool {
        self.is_default
    }

    pub fn default_reasoning_effort(&self) -> &str {
        &self.default_reasoning_effort
    }

    pub fn reasoning_efforts(&self) -> &[ReasoningEffort] {
        &self.reasoning_efforts
    }

    pub fn service_tiers(&self) -> &[ServiceTier] {
        &self.service_tiers
    }
}

/// A browser sign-in that Codex started. Codex reports the result later in
/// `account/login/completed` with the same login ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoginStart {
    pub login_id: String,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessSnapshot {
    account: AccountStatus,
    models: Vec<Model>,
}

impl HarnessSnapshot {
    pub fn account(&self) -> &AccountStatus {
        &self.account
    }

    pub fn models(&self) -> &[Model] {
        &self.models
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conversation {
    pub id: String,
    pub title: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationHistory {
    pub conversation: Conversation,
    pub turns: Vec<HistoryTurn>,
    pub older_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationPage {
    pub thread_id: String,
    pub turns: Vec<HistoryTurn>,
    pub older_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryTurn {
    pub id: String,
    pub status: String,
    pub items: Vec<Value>,
}

/// Text shown for a Codex history item. Non-text attachments stay outside Qrow's transcript.
pub fn history_item_text(item: &Value) -> Option<(&'static str, String)> {
    match item.get("type")?.as_str()? {
        "userMessage" => {
            let text = item
                .get("content")?
                .as_array()?
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            let visible = text
                .rsplit_once(WORKSPACE_CONTEXT_SEPARATOR)
                .map_or(text.as_str(), |(message, _)| message);
            (!visible.is_empty()).then_some(("user", visible.to_owned()))
        }
        "agentMessage" => item
            .get("text")?
            .as_str()
            .map(|text| ("assistant", text.to_owned())),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnRequest {
    pub thread_id: String,
    pub text: String,
    pub context: Value,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub service_tier: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TitleRequest {
    pub thread_id: String,
    /// Recent `(role, text)` messages, oldest first. Roles are `user` or `assistant`.
    pub messages: Vec<(&'static str, String)>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Turn {
    pub id: String,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCall {
    pub request_id: Value,
    pub call_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResult {
    pub success: bool,
    pub content: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssistantEvent {
    MessageDelta {
        thread_id: String,
        turn_id: String,
        text: String,
    },
    TurnCompleted {
        thread_id: String,
        turn: Turn,
        error: Option<String>,
    },
    TitleChanged {
        thread_id: String,
        title: String,
    },
    /// Codex finished a title request without a usable title.
    TitleFailed {
        thread_id: String,
    },
    ToolCall(ToolCall),
    UnsupportedRequest {
        request_id: Value,
        method: String,
    },
    Other {
        method: String,
        params: Value,
    },
}

/// Writes an executable test script. Tests start processes in parallel. A
/// process forked while a test holds a write descriptor for a script inherits
/// it, and running the script then fails with ETXTBSY. So only a short-lived
/// child process writes the executable file.
#[cfg(test)]
pub(crate) fn write_test_executable(path: &std::path::Path, script: &str) {
    let source = path.with_extension("source");
    std::fs::write(&source, script).unwrap();
    let status = std::process::Command::new("/usr/bin/install")
        .args(["-m", "700"])
        .arg(&source)
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "could not install {}", path.display());
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_text_from_codex_history_shapes() {
        assert_eq!(
            history_item_text(
                &json!({"type":"userMessage","content":[{"type":"text","text":"SELECT 1"}]})
            ),
            Some(("user", "SELECT 1".into()))
        );
        assert_eq!(
            history_item_text(&json!({"type":"agentMessage","text":"Done"})),
            Some(("assistant", "Done".into()))
        );
        assert_eq!(
            history_item_text(
                &json!({"type":"userMessage","content":[{"type":"image","url":"test"}]})
            ),
            None
        );
    }

    #[test]
    fn hides_appended_workspace_context_from_steered_message() {
        let steered = format!(
            "Another question{WORKSPACE_CONTEXT_SEPARATOR}{}",
            json!({"version":1,"connections":[{"name":"analytics"}],"tabs":[],"selected_tab":null})
        );
        assert_eq!(
            history_item_text(
                &json!({"type":"userMessage","content":[{"type":"text","text":steered}]})
            ),
            Some(("user", "Another question".into()))
        );
    }
}
