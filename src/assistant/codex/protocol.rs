//! JSON-RPC messages and the Codex app-server types that Qrow reads.
use super::*;

#[derive(Serialize)]
pub(super) struct Request {
    pub(super) id: u64,
    pub(super) method: &'static str,
    pub(super) params: Value,
}

#[derive(Serialize)]
pub(super) struct Notification {
    pub(super) method: &'static str,
    pub(super) params: Value,
}

#[derive(Serialize)]
pub(super) struct SuccessResponse<'a> {
    pub(super) id: &'a Value,
    pub(super) result: Value,
}

#[derive(Serialize)]
pub(super) struct ErrorResponse<'a> {
    pub(super) id: &'a Value,
    pub(super) error: Value,
}

#[derive(Debug)]
pub(super) struct CodexRequestError {
    pub(super) method: &'static str,
    pub(super) code: Value,
    pub(super) message: String,
}

impl CodexRequestError {
    pub(super) fn is_missing_rollout(&self, thread_id: &str) -> bool {
        self.code.as_i64() == Some(-32600)
            && self.message == format!("no rollout found for thread id {thread_id}")
    }
}

impl fmt::Display for CodexRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Codex request {} failed ({}): {}",
            self.method, self.code, self.message
        )
    }
}

impl std::error::Error for CodexRequestError {}

/// Whether Codex sent a response that is larger than the protocol limit.
pub(super) fn is_oversized(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<CodexRequestError>()
        .is_some_and(|error| error.code.as_i64() == Some(OVERSIZED_RESPONSE_CODE))
}

pub(super) fn is_missing_rollout(error: &anyhow::Error, thread_id: &str) -> bool {
    error
        .downcast_ref::<CodexRequestError>()
        .is_some_and(|error| error.is_missing_rollout(thread_id))
}

pub(super) fn explain_missing_rollout(error: anyhow::Error, thread_id: &str) -> anyhow::Error {
    if is_missing_rollout(&error, thread_id) {
        anyhow!("Codex cannot find this conversation. You can delete it from Qrow.")
    } else {
        error
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CodexThread {
    pub(super) id: String,
    pub(super) name: Option<String>,
    pub(super) updated_at: i64,
    #[serde(default)]
    pub(super) turns: Vec<CodexTurn>,
}

impl From<CodexThread> for Conversation {
    fn from(thread: CodexThread) -> Self {
        Self {
            id: thread.id,
            title: thread.name,
            updated_at: thread.updated_at,
        }
    }
}

#[derive(Deserialize)]
pub(super) struct ThreadResponse {
    pub(super) thread: CodexThread,
}

#[derive(Deserialize)]
pub(super) struct TurnResponse {
    pub(super) turn: CodexTurn,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SteerResponse {
    pub(super) turn_id: String,
}

#[derive(Clone, Deserialize)]
pub(super) struct CodexTurn {
    pub(super) id: String,
    pub(super) status: String,
    #[serde(default)]
    pub(super) items: Vec<Value>,
    #[serde(rename = "itemsView", default = "full_items_view")]
    pub(super) items_view: String,
    #[serde(default)]
    pub(super) error: Option<Value>,
}

fn full_items_view() -> String {
    "full".into()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemsPageResponse {
    pub(super) data: Vec<ItemEntry>,
    pub(super) next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemEntry {
    pub(super) turn_id: String,
    pub(super) item: Value,
}

impl From<CodexTurn> for Turn {
    fn from(turn: CodexTurn) -> Self {
        Self {
            id: turn.id,
            status: turn.status,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DynamicToolCall {
    pub(super) arguments: Value,
    pub(super) call_id: String,
    pub(super) thread_id: String,
    pub(super) tool: String,
    pub(super) turn_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountResponse {
    pub(super) account: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModelListResponse {
    pub(super) data: Vec<CodexModel>,
    pub(super) next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CodexModel {
    id: String,
    display_name: String,
    is_default: bool,
    default_reasoning_effort: String,
    supported_reasoning_efforts: Vec<CodexReasoningEffort>,
    #[serde(default)]
    service_tiers: Vec<CodexServiceTier>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexReasoningEffort {
    reasoning_effort: String,
}

#[derive(Deserialize)]
struct CodexServiceTier {
    id: String,
    name: String,
}

impl From<CodexModel> for Model {
    fn from(model: CodexModel) -> Self {
        Self {
            id: model.id,
            display_name: model.display_name,
            is_default: model.is_default,
            default_reasoning_effort: model.default_reasoning_effort,
            reasoning_efforts: model
                .supported_reasoning_efforts
                .into_iter()
                .map(|effort| ReasoningEffort {
                    id: effort.reasoning_effort,
                })
                .collect(),
            service_tiers: model
                .service_tiers
                .into_iter()
                .map(|tier| ServiceTier {
                    id: tier.id,
                    name: tier.name,
                })
                .collect(),
        }
    }
}
