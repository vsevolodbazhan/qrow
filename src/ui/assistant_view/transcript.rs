//! The messages and tool cards of a conversation.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum Speaker {
    User,
    Assistant,
    Activity,
    Error,
}

/// A Qrow tool that the assistant can call. The name stays the same while the
/// call runs and after it ends. The card state shows the progress and outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum ToolKind {
    Workspace,
    ReadQuery,
    AppendQuery,
    EditQuery,
    RunQuery,
    CancelQuery,
    QueryStatus,
    ReadResults,
    FetchRows,
    ReadLogs,
    Other,
}

impl ToolKind {
    pub fn from_name(name: &str) -> Self {
        match name {
            "get_workspace_context" => Self::Workspace,
            "read_tab_sql" => Self::ReadQuery,
            "append_selected_tab_sql" => Self::AppendQuery,
            "edit_selected_tab_sql" => Self::EditQuery,
            "run_selected_tab_query" => Self::RunQuery,
            "cancel_selected_tab_query" => Self::CancelQuery,
            "get_query_status" => Self::QueryStatus,
            "read_results" => Self::ReadResults,
            "fetch_more_results" => Self::FetchRows,
            "read_query_logs" => Self::ReadLogs,
            _ => Self::Other,
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Workspace => "Read workspace",
            Self::ReadQuery => "Read query",
            Self::AppendQuery => "Append query",
            Self::EditQuery => "Edit query",
            Self::RunQuery => "Run query",
            Self::CancelQuery => "Cancel query",
            Self::QueryStatus => "Check query status",
            Self::ReadResults => "Read results",
            Self::FetchRows => "Fetch more rows",
            Self::ReadLogs => "Read logs",
            Self::Other => "Use tool",
        }
    }

    pub(super) fn icon(self) -> AssetIconName {
        match self {
            Self::Workspace => AssetIconName::LayoutDashboard,
            Self::ReadQuery => AssetIconName::FileText,
            Self::AppendQuery => AssetIconName::ListPlus,
            Self::EditQuery => AssetIconName::Pencil,
            Self::RunQuery => AssetIconName::Play,
            Self::CancelQuery => AssetIconName::CircleStop,
            Self::QueryStatus => AssetIconName::Activity,
            Self::ReadResults => AssetIconName::Table,
            Self::FetchRows => AssetIconName::ListPlus,
            Self::ReadLogs => AssetIconName::ScrollText,
            Self::Other => AssetIconName::SquareTerminal,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::ui) enum ToolState {
    /// The call is in progress. The text names the current step.
    Running(String),
    /// The call succeeded. The optional text is a short outcome.
    Done(Option<String>),
    Failed,
    /// Qrow did not run the call, for example because the user declined it.
    Cancelled,
}

impl ToolState {
    pub(super) fn label(&self) -> Option<&str> {
        match self {
            Self::Running(step) => Some(step),
            Self::Done(outcome) => outcome.as_deref(),
            Self::Failed => Some("Failed"),
            Self::Cancelled => Some("Cancelled"),
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::ui) struct ToolActivity {
    pub kind: ToolKind,
    /// The query tab the call used. The expanded card shows it.
    pub target: Option<String>,
    pub state: ToolState,
}

impl ToolActivity {
    /// Matches the collapsed card header: the tool name and its state.
    pub(super) fn label(&self) -> String {
        match self.state.label() {
            Some(state) => format!("{} · {state}", self.kind.title()),
            None => self.kind.title().to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::ui) struct TranscriptEntry {
    pub(super) id: Uuid,
    pub speaker: Speaker,
    /// Shared strings, so that each frame passes the text and the label
    /// without a copy. A change replaces them.
    text: SharedString,
    label: SharedString,
    pub turn_id: Option<String>,
    pub tool: Option<ToolActivity>,
    pub detail: Option<String>,
    pub expanded: bool,
}

impl TranscriptEntry {
    pub fn new(speaker: Speaker, text: impl Into<SharedString>, turn_id: Option<String>) -> Self {
        let mut entry = Self {
            id: Uuid::new_v4(),
            speaker,
            text: SharedString::default(),
            label: SharedString::default(),
            turn_id,
            tool: None,
            detail: None,
            expanded: false,
        };
        entry.set_text(text);
        entry
    }

    pub fn tool(tool: ToolActivity, turn_id: String) -> Self {
        let mut entry = Self::new(Speaker::Activity, tool.label(), Some(turn_id));
        entry.tool = Some(tool);
        entry.set_text(entry.text.clone());
        entry
    }

    pub fn text(&self) -> &SharedString {
        &self.text
    }

    /// The accessible label: the speaker and the text.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>) {
        self.text = text.into();
        let speaker = if self.tool.is_some() {
            "Tool call"
        } else {
            match self.speaker {
                Speaker::User => "You",
                Speaker::Assistant => "Assistant",
                Speaker::Activity => "Assistant activity",
                Speaker::Error => "Assistant error",
            }
        };
        self.label = format!("{speaker}: {}", self.text).into();
    }

    /// Appends a streamed part. This copies the text once for each part.
    pub fn push_text(&mut self, part: &str) {
        let mut text = String::with_capacity(self.text.len() + part.len());
        text.push_str(&self.text);
        text.push_str(part);
        self.set_text(text);
    }

    pub fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }

    /// Replaces the tool state and keeps the accessible text in step with it.
    pub fn set_tool_state(&mut self, state: ToolState) {
        if let Some(tool) = &mut self.tool {
            tool.state = state;
            let text = tool.label();
            self.set_text(text);
        }
    }
}

pub(super) fn merge_history(entries: &mut Vec<TranscriptEntry>, turns: Vec<HistoryTurn>) {
    let mut matched = BTreeSet::new();
    for turn in turns {
        for item in turn.items {
            let Some((speaker, text)) = history_item_text(&item) else {
                continue;
            };
            let speaker = if speaker == "user" {
                Speaker::User
            } else {
                Speaker::Assistant
            };
            if let Some(index) = entries
                .iter()
                .enumerate()
                .find(|(index, entry)| {
                    !matched.contains(index)
                        && entry.speaker == speaker
                        && entry.turn_id.as_deref() == Some(&turn.id)
                })
                .map(|(index, _)| index)
            {
                entries[index].set_text(text);
                matched.insert(index);
            } else {
                entries.push(TranscriptEntry::new(speaker, text, Some(turn.id.clone())));
                matched.insert(entries.len() - 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::WORKSPACE_CONTEXT_SEPARATOR;

    #[::core::prelude::v1::test]
    fn tool_cards_keep_the_tool_name_and_show_the_state_separately() {
        let tool = |kind, state| ToolActivity {
            kind,
            target: Some("Clicks".into()),
            state,
        };
        let mut entry = TranscriptEntry::tool(
            tool(ToolKind::RunQuery, ToolState::Running("Preparing".into())),
            "turn-1".into(),
        );
        assert!(!entry.expanded);
        assert_eq!(entry.text, "Run query · Preparing");
        entry.set_tool_state(ToolState::Done(Some("3 rows · 0.20 s".into())));
        assert_eq!(entry.text, "Run query · 3 rows · 0.20 s");
        entry.set_tool_state(ToolState::Failed);
        assert_eq!(entry.text, "Run query · Failed");
        let edit = TranscriptEntry::tool(
            tool(
                ToolKind::from_name("edit_selected_tab_sql"),
                ToolState::Done(None),
            ),
            "turn-1".into(),
        );
        assert_eq!(edit.text, "Edit query");
        assert_eq!(ToolKind::from_name("unknown_tool"), ToolKind::Other);
        assert!(
            TranscriptEntry::new(Speaker::Error, "Failed", None)
                .tool
                .is_none()
        );
    }

    #[::core::prelude::v1::test]
    fn history_keeps_distinct_messages_from_one_turn() {
        let mut entries = vec![
            TranscriptEntry::new(Speaker::User, "draft", Some("turn-1".into())),
            TranscriptEntry::new(Speaker::Assistant, "streamed", Some("turn-1".into())),
        ];
        merge_history(
            &mut entries,
            vec![HistoryTurn {
                id: "turn-1".into(),
                status: "completed".into(),
                items: vec![
                    json!({"type":"userMessage","content":[{"type":"text","text":"question"}]}),
                    json!({"type":"agentMessage","text":"first"}),
                    json!({"type":"agentMessage","text":"second"}),
                ],
            }],
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].text, "question");
        assert_eq!(entries[1].text, "first");
        assert_eq!(entries[2].text, "second");
    }

    #[::core::prelude::v1::test]
    fn history_merge_keeps_steered_message_visible_without_workspace_context() {
        let mut entries = vec![
            TranscriptEntry::new(Speaker::User, "First", Some("turn-1".into())),
            TranscriptEntry::new(Speaker::User, "Follow-up", Some("turn-1".into())),
        ];
        let steered = format!(
            "Follow-up{WORKSPACE_CONTEXT_SEPARATOR}{}",
            json!({"version":1,"connections":[],"tabs":[],"selected_tab":null})
        );
        merge_history(
            &mut entries,
            vec![HistoryTurn {
                id: "turn-1".into(),
                status: "completed".into(),
                items: vec![
                    json!({"type":"userMessage","content":[{"type":"text","text":"First"}]}),
                    json!({"type":"userMessage","content":[{"type":"text","text":steered}]}),
                ],
            }],
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].text, "Follow-up");
    }
}
