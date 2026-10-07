//! The messages and tool cards of a conversation.
use super::*;
use std::borrow::Cow;
use std::time::Duration;

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
    ListSchemas,
    ListRelations,
    DescribeRelation,
    SearchModels,
    DescribeModel,
    ReadLineage,
    ReadModelSql,
    Other,
}

impl ToolKind {
    pub fn from_name(name: &str) -> Self {
        let name = crate::assistant::tools::canonical_name(name);
        match name {
            "workspace-read-context" => Self::Workspace,
            "tab-read-sql" => Self::ReadQuery,
            "tab-append-sql" => Self::AppendQuery,
            "tab-edit-sql" => Self::EditQuery,
            "query-run" => Self::RunQuery,
            "query-cancel" => Self::CancelQuery,
            "query-read-status" => Self::QueryStatus,
            "query-read-results" => Self::ReadResults,
            "query-fetch-results" => Self::FetchRows,
            "query-read-logs" => Self::ReadLogs,
            "catalog-list-schemas" => Self::ListSchemas,
            "catalog-list-relations" => Self::ListRelations,
            "catalog-describe-relation" => Self::DescribeRelation,
            "dbt-search-models" => Self::SearchModels,
            "dbt-describe-model" => Self::DescribeModel,
            "dbt-read-lineage" => Self::ReadLineage,
            "dbt-read-sql" => Self::ReadModelSql,
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
            Self::ListSchemas => "List schemas",
            Self::ListRelations => "List tables",
            Self::DescribeRelation => "Describe table",
            Self::SearchModels => "Search dbt models",
            Self::DescribeModel => "Describe dbt model",
            Self::ReadLineage => "Read dbt lineage",
            Self::ReadModelSql => "Read dbt model SQL",
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
            Self::ListSchemas => AssetIconName::Database,
            Self::ListRelations => AssetIconName::Table2,
            Self::DescribeRelation => AssetIconName::Columns3,
            Self::SearchModels => AssetIconName::Search,
            Self::DescribeModel => AssetIconName::BookOpen,
            Self::ReadLineage => AssetIconName::Network,
            Self::ReadModelSql => AssetIconName::FileCode,
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
    /// A successful SQL call keeps its row count and duration separate.
    QueryResult {
        rows: usize,
        more: bool,
        elapsed: Option<Duration>,
    },
    Failed,
    /// Qrow did not run the call, for example because the user declined it.
    Cancelled,
}

impl ToolState {
    pub(super) fn label(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::Running(step) => Some(step.as_str().into()),
            Self::Done(outcome) => outcome.as_deref().map(Cow::Borrowed),
            Self::QueryResult { rows, more, .. } => Some(match (*rows, *more) {
                (1, false) => "1 row".into(),
                (rows, false) => format!("{rows} rows").into(),
                (rows, true) => format!("{rows}+ rows").into(),
            }),
            Self::Failed => Some("Failed".into()),
            Self::Cancelled => Some("Cancelled".into()),
        }
    }

    pub(super) fn elapsed(&self) -> Option<Duration> {
        match self {
            Self::QueryResult { elapsed, .. } => *elapsed,
            _ => None,
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
        match (self.state.label(), self.state.elapsed()) {
            (Some(state), Some(elapsed)) => format!(
                "{}: {state} in {:.2} s",
                self.kind.title(),
                elapsed.as_secs_f64()
            ),
            (Some(state), None) => format!("{}: {state}", self.kind.title()),
            (None, _) => self.kind.title().to_owned(),
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
    /// The part of `text` that shows. A streamed reply shows its text step
    /// by step. See [`Reveal`].
    shown: SharedString,
    reveal: Option<Reveal>,
    pub turn_id: Option<String>,
    pub tool: Option<ToolActivity>,
    pub detail: Option<String>,
    expanded: bool,
    /// Counts the changes that can change the height of the entry.
    revision: u64,
}

impl TranscriptEntry {
    pub fn new(speaker: Speaker, text: impl Into<SharedString>, turn_id: Option<String>) -> Self {
        let mut entry = Self {
            id: Uuid::new_v4(),
            speaker,
            text: SharedString::default(),
            label: SharedString::default(),
            shown: SharedString::default(),
            reveal: None,
            turn_id,
            tool: None,
            detail: None,
            expanded: false,
            revision: 0,
        };
        entry.set_text(text);
        entry
    }

    /// The first part of a streamed reply. Its text shows step by step.
    pub fn streamed(speaker: Speaker, text: &str, turn_id: Option<String>) -> Self {
        let mut entry = Self::new(speaker, "", turn_id);
        entry.reveal = Some(Reveal::default());
        entry.push_text(text);
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

    /// The text that the transcript shows.
    pub fn shown_text(&self) -> &SharedString {
        &self.shown
    }

    /// The accessible label: the speaker and the shown text.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn expanded(&self) -> bool {
        self.expanded
    }

    /// Opens or closes the detail of a tool card.
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
        self.revision += 1;
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Replaces the text with its final version. A streamed reply that the
    /// new text continues shows the rest step by step.
    pub fn set_text(&mut self, text: impl Into<SharedString>) {
        let text = text.into();
        match &mut self.reveal {
            Some(reveal) if text.starts_with(self.shown.as_str()) => reveal.complete(),
            _ => self.reveal = None,
        }
        self.replace_text(text);
    }

    fn replace_text(&mut self, text: SharedString) {
        self.text = text;
        if self.reveal.is_none() {
            self.shown = self.text.clone();
        }
        self.shown_changed();
    }

    fn shown_changed(&mut self) {
        self.revision += 1;
        let speaker = if self.tool.is_some() {
            "Tool Call"
        } else {
            match self.speaker {
                Speaker::User => "You",
                Speaker::Assistant => "Assistant",
                Speaker::Activity => "Assistant Activity",
                Speaker::Error => "Assistant Error",
            }
        };
        self.label = format!("{speaker}: {}", self.shown).into();
    }

    /// Appends a streamed part. This copies the text once for each part.
    pub fn push_text(&mut self, part: &str) {
        let mut text = String::with_capacity(self.text.len() + part.len());
        text.push_str(&self.text);
        text.push_str(part);
        if let Some(reveal) = &mut self.reveal {
            reveal.received();
        }
        self.replace_text(text.into());
    }

    /// Whether the entry has streamed text that does not show yet.
    pub fn revealing(&self) -> bool {
        self.reveal.is_some() && self.shown.len() < self.text.len()
    }

    /// Shows more of the streamed text. Returns whether the shown text changed.
    pub fn advance_reveal(&mut self, elapsed: std::time::Duration) -> bool {
        let Some(reveal) = &mut self.reveal else {
            return false;
        };
        if !reveal.advance(&self.text, elapsed) {
            return false;
        }
        self.shown = if reveal.shown() == self.text.len() {
            self.text.clone()
        } else {
            self.text[..reveal.shown()].to_owned().into()
        };
        self.shown_changed();
        true
    }

    /// Shows all streamed text at once. Returns whether the shown text changed.
    pub fn show_all(&mut self) -> bool {
        let Some(reveal) = &mut self.reveal else {
            return false;
        };
        reveal.show_all(&self.text);
        if self.shown == self.text {
            return false;
        }
        self.shown = self.text.clone();
        self.shown_changed();
        true
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

/// One row of the transcript list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum Row {
    /// The button that loads older messages.
    Older,
    Entry {
        id: Uuid,
        revision: u64,
    },
    /// The line that shows while Codex works on a reply.
    Working,
}

impl Row {
    /// Whether both rows show the same entry, maybe at another revision.
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Entry { id, .. }, Self::Entry { id: other, .. }) => id == other,
            _ => self == other,
        }
    }
}

/// The rows of a transcript, and the conversation or tab that they show.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::ui) struct TranscriptRows {
    pub source: Option<String>,
    pub rows: Vec<Row>,
}

/// The change of a transcript list from one set of rows to the next.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::ui) struct RowChange {
    /// The old rows that new rows replace.
    pub replaced: Range<usize>,
    /// The number of new rows in place of `replaced`.
    pub inserted: usize,
    /// The new indexes of the rows whose content changed.
    pub remeasure: Vec<usize>,
}

/// Compares the rows before and after a change of one transcript. The rows
/// before and after the changed part keep their place, so that the list keeps
/// its scroll position when older messages load or a reply grows.
pub(in crate::ui) fn row_change(old: &[Row], new: &[Row]) -> RowChange {
    let prefix = old
        .iter()
        .zip(new)
        .take_while(|(old, new)| old.same(new))
        .count();
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(old.len().min(new.len()) - prefix)
        .take_while(|(old, new)| old.same(new))
        .count();
    let changed = |(old_index, new_index): (usize, usize)| {
        (old[old_index] != new[new_index]).then_some(new_index)
    };
    let remeasure = (0..prefix)
        .map(|index| (index, index))
        .chain((0..suffix).map(|index| (old.len() - suffix + index, new.len() - suffix + index)))
        .filter_map(changed)
        .collect();
    RowChange {
        replaced: prefix..old.len() - suffix,
        inserted: new.len() - prefix - suffix,
        remeasure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::WORKSPACE_CONTEXT_SEPARATOR;

    #[::core::prelude::v1::test]
    fn the_icon_of_each_tool_kind_loads() {
        use gpui_kit::AssetSource as _;
        let kinds = [
            ToolKind::Workspace,
            ToolKind::ReadQuery,
            ToolKind::AppendQuery,
            ToolKind::EditQuery,
            ToolKind::RunQuery,
            ToolKind::CancelQuery,
            ToolKind::QueryStatus,
            ToolKind::ReadResults,
            ToolKind::FetchRows,
            ToolKind::ReadLogs,
            ToolKind::ListSchemas,
            ToolKind::ListRelations,
            ToolKind::DescribeRelation,
            ToolKind::SearchModels,
            ToolKind::DescribeModel,
            ToolKind::ReadLineage,
            ToolKind::ReadModelSql,
            ToolKind::Other,
        ];
        for kind in kinds {
            // A new kind does not compile until it is in the list above.
            match kind {
                ToolKind::Workspace
                | ToolKind::ReadQuery
                | ToolKind::AppendQuery
                | ToolKind::EditQuery
                | ToolKind::RunQuery
                | ToolKind::CancelQuery
                | ToolKind::QueryStatus
                | ToolKind::ReadResults
                | ToolKind::FetchRows
                | ToolKind::ReadLogs
                | ToolKind::ListSchemas
                | ToolKind::ListRelations
                | ToolKind::DescribeRelation
                | ToolKind::SearchModels
                | ToolKind::DescribeModel
                | ToolKind::ReadLineage
                | ToolKind::ReadModelSql
                | ToolKind::Other => {}
            }
            let path = kind.icon().path();
            assert!(
                crate::assets::Assets.load(&path).unwrap().is_some(),
                "{path} of {kind:?} is missing"
            );
        }
    }

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
        assert!(!entry.expanded());
        assert_eq!(entry.text, "Run query: Preparing");
        entry.set_tool_state(ToolState::QueryResult {
            rows: 3,
            more: false,
            elapsed: Some(Duration::from_millis(200)),
        });
        assert_eq!(entry.text, "Run query: 3 rows in 0.20 s");
        entry.set_tool_state(ToolState::Failed);
        assert_eq!(entry.text, "Run query: Failed");
        let edit = TranscriptEntry::tool(
            tool(ToolKind::from_name("tab-edit-sql"), ToolState::Done(None)),
            "turn-1".into(),
        );
        assert_eq!(edit.text, "Edit query");
        assert_eq!(ToolKind::from_name("unknown_tool"), ToolKind::Other);
        // An older conversation calls the earlier name.
        assert_eq!(
            ToolKind::from_name("edit_selected_tab_sql"),
            ToolKind::EditQuery
        );
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

    fn entry(id: u128, revision: u64) -> Row {
        Row::Entry {
            id: Uuid::from_u128(id),
            revision,
        }
    }

    #[::core::prelude::v1::test]
    fn transcript_rows_keep_their_place_when_rows_are_added_or_change() {
        let change = |replaced, inserted, remeasure: &[usize]| RowChange {
            replaced,
            inserted,
            remeasure: remeasure.to_vec(),
        };
        let rows = [Row::Older, entry(1, 0), entry(2, 0)];
        assert_eq!(row_change(&rows, &rows), change(3..3, 0, &[]));
        // A new reply and the working line after the last message.
        let working = [
            Row::Older,
            entry(1, 0),
            entry(2, 0),
            entry(3, 0),
            Row::Working,
        ];
        assert_eq!(row_change(&rows, &working), change(3..3, 2, &[]));
        // Streamed text changes the reply before the working line.
        let streamed = [
            Row::Older,
            entry(1, 0),
            entry(2, 0),
            entry(3, 4),
            Row::Working,
        ];
        assert_eq!(row_change(&working, &streamed), change(5..5, 0, &[3]));
        // Older messages go between the button and the first message.
        let older = [
            Row::Older,
            entry(8, 0),
            entry(9, 0),
            entry(1, 0),
            entry(2, 1),
        ];
        assert_eq!(row_change(&rows, &older), change(1..1, 2, &[4]));
        // A finished query replaces its running tool card.
        let replaced = [Row::Older, entry(1, 0), entry(7, 0)];
        assert_eq!(row_change(&rows, &replaced), change(2..3, 1, &[]));
        // The last page removes the button.
        assert_eq!(row_change(&rows, &rows[1..]), change(0..1, 0, &[]));
        assert_eq!(row_change(&[], &rows), change(0..0, 3, &[]));
    }

    #[::core::prelude::v1::test]
    fn entries_count_the_changes_of_their_height() {
        let mut entry = TranscriptEntry::new(Speaker::Assistant, "Part", None);
        let first = entry.revision();
        entry.push_text(" two");
        assert!(entry.revision() > first);
        let streamed = entry.revision();
        entry.toggle_expanded();
        assert!(entry.expanded() && entry.revision() > streamed);
    }

    #[::core::prelude::v1::test]
    fn final_text_after_a_shown_reply_also_shows() {
        let mut entry = TranscriptEntry::streamed(Speaker::Assistant, "One two ", None);
        while entry.advance_reveal(Duration::from_millis(100)) {}
        assert!(entry.shown_text().starts_with("One two"));
        assert!(entry.show_all() || entry.shown_text() == "One two ");
        // Codex can drop streamed parts. The final text has them.
        entry.set_text("One two three four");
        assert!(entry.revealing());
        while entry.advance_reveal(Duration::from_millis(100)) {}
        assert_eq!(entry.shown_text(), "One two three four");
        assert!(!entry.revealing());
        // Other final text shows at once.
        entry.set_text("Something else");
        assert_eq!(entry.shown_text(), "Something else");
    }
}
