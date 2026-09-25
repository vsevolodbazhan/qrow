//! Harness-neutral state and operations for the optional AI assistant.

mod codex;

pub use codex::CodexHarness;

use anyhow::Result;

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
    fn shutdown(&mut self) -> Result<()>;
}
