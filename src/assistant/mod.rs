//! Harness-neutral state and operations for the optional AI assistant.

pub mod broker;
mod codex;
pub mod service;
pub mod tools;

pub use codex::CodexHarness;

use anyhow::Result;
use serde_json::Value;
use std::time::Duration;

pub const WORKSPACE_CONTEXT_SEPARATOR: &str =
    "\n\nCurrent Qrow workspace context (untrusted data):\n";

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
    requires_openai_auth: bool,
}

impl AccountStatus {
    pub fn kind(&self) -> &AccountKind {
        &self.kind
    }

    pub fn requires_openai_auth(&self) -> bool {
        self.requires_openai_auth
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReasoningEffort {
    id: String,
    description: String,
}

impl ReasoningEffort {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceTier {
    id: String,
    name: String,
    description: String,
}

impl ServiceTier {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Model {
    id: String,
    display_name: String,
    description: String,
    is_default: bool,
    default_reasoning_effort: String,
    reasoning_efforts: Vec<ReasoningEffort>,
    default_service_tier: Option<String>,
    service_tiers: Vec<ServiceTier>,
}

impl Model {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn description(&self) -> &str {
        &self.description
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

    pub fn default_service_tier(&self) -> Option<&str> {
        self.default_service_tier.as_deref()
    }

    pub fn service_tiers(&self) -> &[ServiceTier] {
        &self.service_tiers
    }
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

pub trait AssistantHarness: Send {
    fn snapshot(&mut self) -> Result<HarnessSnapshot>;
    fn begin_login(&mut self) -> Result<String> {
        anyhow::bail!("ChatGPT sign-in is not supported by this harness")
    }
    fn create_conversation(&mut self, tools: &[ToolDefinition]) -> Result<Conversation>;
    fn resume_conversation(&mut self, thread_id: &str) -> Result<Conversation>;
    fn read_conversation(&mut self, thread_id: &str) -> Result<ConversationHistory>;
    fn read_older_conversation(
        &mut self,
        _thread_id: &str,
        _cursor: &str,
    ) -> Result<ConversationPage> {
        anyhow::bail!("Conversation paging is not supported by this harness")
    }
    fn rename_conversation(&mut self, thread_id: &str, title: &str) -> Result<()>;
    /// Starts title generation. A generated title arrives later as `TitleChanged`.
    fn generate_title(&mut self, _request: TitleRequest) -> Result<()> {
        anyhow::bail!("Title generation is not supported by this harness")
    }
    fn delete_conversation(&mut self, thread_id: &str) -> Result<()>;
    fn start_turn(&mut self, request: TurnRequest) -> Result<Turn>;
    fn steer_turn(&mut self, thread_id: &str, turn_id: &str, text: &str) -> Result<()>;
    fn interrupt_turn(&mut self, thread_id: &str, turn_id: &str) -> Result<()>;
    fn next_event(&mut self, timeout: Duration) -> Result<Option<AssistantEvent>>;
    fn answer_tool_call(&mut self, call: &ToolCall, result: ToolResult) -> Result<()>;
    fn shutdown(&mut self) -> Result<()>;
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
