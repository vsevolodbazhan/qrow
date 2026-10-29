//! The harness that the assistant worker runs: Codex or Claude Code.
use super::{
    ClaudeHarness, CodexHarness, Conversation, ConversationHistory, ConversationPage,
    HarnessSnapshot, LoginStart, TitleRequest, ToolCall, ToolDefinition, ToolResult, Turn,
    TurnRequest, codex::Input,
};
use anyhow::Result;
use serde_json::Value;
use std::time::Instant;

pub(crate) enum Harness {
    Codex(CodexHarness),
    Claude(ClaudeHarness),
}

macro_rules! delegate {
    ($(fn $name:ident(&mut self $(, $arg:ident: $type:ty)*) -> $output:ty;)*) => {
        $(
            pub fn $name(&mut self $(, $arg: $type)*) -> $output {
                match self {
                    Self::Codex(harness) => harness.$name($($arg),*),
                    Self::Claude(harness) => harness.$name($($arg),*),
                }
            }
        )*
    };
}

impl Harness {
    delegate! {
        fn snapshot(&mut self) -> Result<HarnessSnapshot>;
        fn begin_login(&mut self) -> Result<LoginStart>;
        fn cancel_login(&mut self, login_id: &str) -> Result<()>;
        fn create_conversation(&mut self, tools: &[ToolDefinition]) -> Result<Conversation>;
        fn resume_conversation(&mut self, thread_id: &str) -> Result<Conversation>;
        fn read_conversation(&mut self, thread_id: &str) -> Result<ConversationHistory>;
        fn read_older_conversation(&mut self, thread_id: &str, cursor: &str) -> Result<ConversationPage>;
        fn rename_conversation(&mut self, thread_id: &str, title: &str) -> Result<()>;
        fn generate_title(&mut self, request: TitleRequest) -> Result<()>;
        fn delete_conversation(&mut self, thread_id: &str) -> Result<()>;
        fn start_turn(&mut self, request: TurnRequest) -> Result<Turn>;
        fn steer_turn(&mut self, thread_id: &str, turn_id: &str, text: &str, context: &Value) -> Result<()>;
        fn interrupt_turn(&mut self, thread_id: &str, turn_id: &str) -> Result<()>;
        fn answer_tool_call(&mut self, call: &ToolCall, result: ToolResult) -> Result<()>;
        fn shutdown(&mut self) -> Result<()>;
        fn next_input(&mut self, deadline: Option<Instant>) -> Result<Option<Input>>;
    }

    pub(crate) fn delete_conversations_on_shutdown(
        &mut self,
        ids: impl IntoIterator<Item = String>,
    ) -> Result<()> {
        match self {
            Self::Codex(harness) => harness.delete_conversations_on_shutdown(ids),
            Self::Claude(harness) => harness.delete_conversations_on_shutdown(ids),
        }
    }
}
