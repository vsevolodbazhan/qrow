//! Validation boundary between an assistant harness and Qrow state.

use crate::sql;
use serde::{Deserialize, Serialize};
use std::{io, ops::Range};
use uuid::Uuid;

pub const TOOL_SCHEMA_VERSION: u32 = 1;
pub const MAX_TEXT_EDITS: usize = 64;
pub const MAX_EDIT_BYTES: usize = 1024 * 1024;
pub const MAX_SQL_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TOOL_ROWS: usize = 100;
pub const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_HARNESS_ID_BYTES: usize = 256;
const TOOL_OUTPUT_ENVELOPE_BYTES: usize = 1024;
const MAX_TOOL_PAYLOAD_BYTES: usize = MAX_TOOL_OUTPUT_BYTES - TOOL_OUTPUT_ENVELOPE_BYTES;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connected,
    Busy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryState {
    Idle,
    Running,
    Cancelling,
    Finished,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConnectionContext {
    pub id: Uuid,
    pub name: String,
    pub connector: &'static str,
    pub initial_database: String,
    pub state: ConnectionState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TabSummary {
    pub id: Uuid,
    pub title: String,
    pub connection_id: Option<Uuid>,
    pub state: QueryState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResultSummary {
    pub columns: Vec<String>,
    pub downloaded_rows: usize,
    pub more_rows_available: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SelectedTabContext {
    #[serde(flatten)]
    pub tab: TabSummary,
    pub sql: String,
    pub selected_range: Option<Range<usize>>,
    pub editor_revision: u64,
    pub results: ResultSummary,
    pub latest_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceContext {
    pub version: u32,
    pub connections: Vec<ConnectionContext>,
    pub tabs: Vec<TabSummary>,
    pub selected_tab: Option<SelectedTabContext>,
}

impl WorkspaceContext {
    pub fn new(
        connections: Vec<ConnectionContext>,
        tabs: Vec<TabSummary>,
        selected_tab: Option<SelectedTabContext>,
    ) -> Self {
        Self {
            version: TOOL_SCHEMA_VERSION,
            connections,
            tabs,
            selected_tab,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionTarget {
    pub conversation_id: String,
    pub turn_id: String,
    pub tab_id: Uuid,
    pub connection_id: Uuid,
    pub selected_range: Option<Range<usize>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallIdentity<'a> {
    pub conversation_id: &'a str,
    pub turn_id: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorDocument<'a> {
    pub tab_id: Uuid,
    pub connection_id: Option<Uuid>,
    pub revision: u64,
    pub sql: &'a str,
    pub selected_range: Option<Range<usize>>,
    pub busy: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TextEdit {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    pub version: u32,
    pub tab_id: Uuid,
    pub connection_id: Uuid,
    pub editor_revision: u64,
    pub edits: Vec<TextEdit>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RunRequest {
    pub version: u32,
    pub tab_id: Uuid,
    pub connection_id: Uuid,
    pub editor_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditPlan {
    pub tab_id: Uuid,
    pub expected_revision: u64,
    pub sql: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunPlan {
    pub tab_id: Uuid,
    pub expected_revision: u64,
    pub sql: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorCode {
    NoActionTarget,
    StaleTarget,
    StaleRevision,
    InvalidStatement,
    TabBusy,
    InvalidArguments,
    LimitReached,
    CapabilityMissing,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolError {
    pub code: ToolErrorCode,
    pub message: String,
}

impl ToolError {
    fn new(code: ToolErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub struct ToolBroker {
    target: Option<ActionTarget>,
}

impl ToolBroker {
    pub fn new(target: Option<ActionTarget>) -> Self {
        Self { target }
    }

    pub fn plan_edit(
        &self,
        call: CallIdentity<'_>,
        request: &EditRequest,
        document: &EditorDocument<'_>,
    ) -> Result<EditPlan, ToolError> {
        self.validate_request(
            request.version,
            call,
            request.tab_id,
            request.connection_id,
            request.editor_revision,
            document,
        )?;
        if request.edits.is_empty() || request.edits.len() > MAX_TEXT_EDITS {
            return Err(ToolError::new(
                ToolErrorCode::LimitReached,
                format!("Provide between 1 and {MAX_TEXT_EDITS} edits."),
            ));
        }
        let replacement_bytes = request
            .edits
            .iter()
            .try_fold(0_usize, |total, edit| {
                total.checked_add(edit.replacement.len())
            })
            .ok_or_else(|| {
                ToolError::new(ToolErrorCode::LimitReached, "Edit content is too large.")
            })?;
        if replacement_bytes > MAX_EDIT_BYTES {
            return Err(ToolError::new(
                ToolErrorCode::LimitReached,
                "Edit content is too large.",
            ));
        }
        let mut edits: Vec<_> = request.edits.iter().collect();
        edits.sort_by_key(|edit| (edit.start, edit.end));
        let mut previous_end = 0;
        let mut previous_empty = None;
        for (index, edit) in edits.iter().enumerate() {
            if edit.start > edit.end
                || edit.end > document.sql.len()
                || !document.sql.is_char_boundary(edit.start)
                || !document.sql.is_char_boundary(edit.end)
                || (index > 0 && edit.start < previous_end)
                || (edit.start == edit.end && previous_empty == Some(edit.start))
            {
                return Err(ToolError::new(
                    ToolErrorCode::InvalidArguments,
                    "Edits must use valid, non-overlapping UTF-8 byte ranges.",
                ));
            }
            previous_end = edit.end;
            previous_empty = (edit.start == edit.end).then_some(edit.start);
        }
        let mut sql = document.sql.to_owned();
        for edit in edits.into_iter().rev() {
            sql.replace_range(edit.start..edit.end, &edit.replacement);
        }
        if sql.len() > MAX_SQL_BYTES {
            return Err(ToolError::new(
                ToolErrorCode::LimitReached,
                "The edited query is too large.",
            ));
        }
        Ok(EditPlan {
            tab_id: document.tab_id,
            expected_revision: document.revision,
            sql,
        })
    }

    pub fn plan_run(
        &self,
        call: CallIdentity<'_>,
        request: &RunRequest,
        document: &EditorDocument<'_>,
    ) -> Result<RunPlan, ToolError> {
        self.validate_request(
            request.version,
            call,
            request.tab_id,
            request.connection_id,
            request.editor_revision,
            document,
        )?;
        if document.busy {
            return Err(ToolError::new(
                ToolErrorCode::TabBusy,
                "The query tab is already running a query.",
            ));
        }
        let target = self.target.as_ref().expect("validated action target");
        if document.selected_range != target.selected_range {
            return Err(ToolError::new(
                ToolErrorCode::StaleTarget,
                "The editor selection changed. Send another message to set a new target.",
            ));
        }
        let sql = match &target.selected_range {
            Some(range)
                if range.start <= range.end
                    && range.end <= document.sql.len()
                    && document.sql.is_char_boundary(range.start)
                    && document.sql.is_char_boundary(range.end) =>
            {
                &document.sql[range.clone()]
            }
            Some(_) => {
                return Err(ToolError::new(
                    ToolErrorCode::InvalidArguments,
                    "The editor selection is invalid.",
                ));
            }
            None => document.sql,
        };
        if sql.len() > MAX_SQL_BYTES {
            return Err(ToolError::new(
                ToolErrorCode::LimitReached,
                "The query is too large.",
            ));
        }
        sql::validate_single(sql)
            .map_err(|error| ToolError::new(ToolErrorCode::InvalidStatement, error.to_string()))?;
        Ok(RunPlan {
            tab_id: document.tab_id,
            expected_revision: document.revision,
            sql: sql.to_owned(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_request(
        &self,
        version: u32,
        call: CallIdentity<'_>,
        tab_id: Uuid,
        connection_id: Uuid,
        editor_revision: u64,
        document: &EditorDocument<'_>,
    ) -> Result<(), ToolError> {
        if version != TOOL_SCHEMA_VERSION {
            return Err(ToolError::new(
                ToolErrorCode::CapabilityMissing,
                "The assistant tool schema version is not supported.",
            ));
        }
        if call.conversation_id.len() > MAX_HARNESS_ID_BYTES
            || call.turn_id.len() > MAX_HARNESS_ID_BYTES
        {
            return Err(ToolError::new(
                ToolErrorCode::InvalidArguments,
                "The assistant call identifier is too large.",
            ));
        }
        let target = self.target.as_ref().ok_or_else(|| {
            ToolError::new(
                ToolErrorCode::NoActionTarget,
                "Select a query tab and send another message.",
            )
        })?;
        if call.conversation_id != target.conversation_id
            || call.turn_id != target.turn_id
            || tab_id != target.tab_id
            || connection_id != target.connection_id
            || document.tab_id != target.tab_id
            || document.connection_id != Some(target.connection_id)
        {
            return Err(ToolError::new(
                ToolErrorCode::StaleTarget,
                "The selected query tab changed. Send another message to set a new target.",
            ));
        }
        if editor_revision != document.revision {
            return Err(ToolError::new(
                ToolErrorCode::StaleRevision,
                "The query changed. Review it and wait for a new user instruction.",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BoundedRows {
    pub rows: Vec<Vec<Option<String>>>,
    pub omitted_row_offsets: Vec<usize>,
    pub next_offset: usize,
    pub truncated: bool,
}

pub fn bound_rows(
    rows: &[Vec<Option<String>>],
    offset: usize,
    requested: usize,
) -> Result<BoundedRows, ToolError> {
    if requested == 0 || requested > MAX_TOOL_ROWS {
        return Err(ToolError::new(
            ToolErrorCode::InvalidArguments,
            format!("Row count must be between 1 and {MAX_TOOL_ROWS}."),
        ));
    }
    let mut output = Vec::new();
    let mut omitted_row_offsets = Vec::new();
    let mut bytes = 0_usize;
    let mut consumed = 0_usize;
    let end = offset.saturating_add(requested).min(rows.len());
    for (index, row) in rows.get(offset..end).unwrap_or_default().iter().enumerate() {
        let separator = usize::from(!output.is_empty());
        let remaining = MAX_TOOL_PAYLOAD_BYTES.saturating_sub(bytes.saturating_add(separator));
        let Some(full_row_bytes) = encoded_len_with_limit(row, MAX_TOOL_PAYLOAD_BYTES) else {
            omitted_row_offsets.push(offset + index);
            consumed += 1;
            continue;
        };
        if full_row_bytes > remaining {
            break;
        }
        bytes = bytes.saturating_add(full_row_bytes + separator);
        output.push(row.clone());
        consumed += 1;
    }
    let next_offset = offset.saturating_add(consumed).min(rows.len());
    Ok(BoundedRows {
        truncated: !omitted_row_offsets.is_empty() || next_offset < end,
        rows: output,
        omitted_row_offsets,
        next_offset,
    })
}

pub fn bound_text(text: &str) -> (String, bool) {
    if text.len() <= MAX_TOOL_PAYLOAD_BYTES {
        return (text.to_owned(), false);
    }
    let mut end = MAX_TOOL_PAYLOAD_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

fn encoded_len_with_limit(value: &impl Serialize, limit: usize) -> Option<usize> {
    struct Counter {
        length: usize,
        limit: usize,
    }
    impl io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.length.saturating_add(bytes.len()) > self.limit {
                return Err(io::Error::other("encoded value exceeds limit"));
            }
            self.length += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { length: 0, limit };
    serde_json::to_writer(&mut counter, value)
        .ok()
        .map(|()| counter.length)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (Uuid, Uuid) {
        (Uuid::new_v4(), Uuid::new_v4())
    }

    fn target(tab: Uuid, connection: Uuid) -> ActionTarget {
        ActionTarget {
            conversation_id: "conversation".into(),
            turn_id: "turn".into(),
            tab_id: tab,
            connection_id: connection,
            selected_range: None,
        }
    }

    fn call() -> CallIdentity<'static> {
        CallIdentity {
            conversation_id: "conversation",
            turn_id: "turn",
        }
    }

    fn document<'a>(tab: Uuid, connection: Uuid, sql: &'a str) -> EditorDocument<'a> {
        EditorDocument {
            tab_id: tab,
            connection_id: Some(connection),
            revision: 7,
            sql,
            selected_range: None,
            busy: false,
        }
    }

    fn edit_request(tab: Uuid, connection: Uuid, edits: Vec<TextEdit>) -> EditRequest {
        EditRequest {
            version: TOOL_SCHEMA_VERSION,
            tab_id: tab,
            connection_id: connection,
            editor_revision: 7,
            edits,
        }
    }

    #[test]
    fn context_serialization_contains_only_allow_list_fields() {
        let (tab, connection) = ids();
        let context = WorkspaceContext::new(
            vec![ConnectionContext {
                id: connection,
                name: "Analytics".into(),
                connector: "spark_kyuubi",
                initial_database: "warehouse".into(),
                state: ConnectionState::Connected,
            }],
            vec![],
            Some(SelectedTabContext {
                tab: TabSummary {
                    id: tab,
                    title: "Query 1".into(),
                    connection_id: Some(connection),
                    state: QueryState::Idle,
                },
                sql: "SELECT 1".into(),
                selected_range: None,
                editor_revision: 2,
                results: ResultSummary {
                    columns: vec!["value".into()],
                    downloaded_rows: 1,
                    more_rows_available: false,
                },
                latest_error: None,
            }),
        );
        let json = serde_json::to_string(&context).unwrap();

        assert!(json.contains("Analytics"));
        for forbidden in ["host", "port", "username", "password", "parameters"] {
            assert!(!json.contains(forbidden));
        }
    }

    #[test]
    fn edits_apply_atomically_in_any_input_order() {
        let (tab, connection) = ids();
        let broker = ToolBroker::new(Some(target(tab, connection)));
        let request = edit_request(
            tab,
            connection,
            vec![
                TextEdit {
                    start: 7,
                    end: 8,
                    replacement: "second".into(),
                },
                TextEdit {
                    start: 0,
                    end: 6,
                    replacement: "VALUES".into(),
                },
            ],
        );

        let plan = broker
            .plan_edit(call(), &request, &document(tab, connection, "SELECT 1"))
            .unwrap();

        assert_eq!(plan.sql, "VALUES second");
        assert_eq!(plan.expected_revision, 7);
    }

    #[test]
    fn edits_reject_stale_targets_revisions_and_invalid_ranges() {
        let (tab, connection) = ids();
        let other_tab = Uuid::new_v4();
        let broker = ToolBroker::new(Some(target(tab, connection)));
        let mut request = edit_request(
            tab,
            connection,
            vec![TextEdit {
                start: 1,
                end: 0,
                replacement: String::new(),
            }],
        );
        assert_eq!(
            broker
                .plan_edit(call(), &request, &document(tab, connection, "é"))
                .unwrap_err()
                .code,
            ToolErrorCode::InvalidArguments
        );
        request.editor_revision = 6;
        assert_eq!(
            broker
                .plan_edit(call(), &request, &document(tab, connection, "SELECT 1"))
                .unwrap_err()
                .code,
            ToolErrorCode::StaleRevision
        );
        request.editor_revision = 7;
        request.tab_id = other_tab;
        assert_eq!(
            broker
                .plan_edit(call(), &request, &document(tab, connection, "SELECT 1"))
                .unwrap_err()
                .code,
            ToolErrorCode::StaleTarget
        );

        let duplicate_insertions = edit_request(
            tab,
            connection,
            vec![
                TextEdit {
                    start: 0,
                    end: 0,
                    replacement: "a".into(),
                },
                TextEdit {
                    start: 0,
                    end: 0,
                    replacement: "b".into(),
                },
            ],
        );
        assert_eq!(
            broker
                .plan_edit(
                    call(),
                    &duplicate_insertions,
                    &document(tab, connection, "SELECT 1"),
                )
                .unwrap_err()
                .code,
            ToolErrorCode::InvalidArguments
        );
    }

    #[test]
    fn run_uses_selection_and_existing_single_statement_validation() {
        let (tab, connection) = ids();
        let broker = ToolBroker::new(Some(target(tab, connection)));
        let request = RunRequest {
            version: TOOL_SCHEMA_VERSION,
            tab_id: tab,
            connection_id: connection,
            editor_revision: 7,
        };
        let mut document = document(tab, connection, "SELECT 1; SELECT 2");
        assert_eq!(
            broker
                .plan_run(call(), &request, &document)
                .unwrap_err()
                .code,
            ToolErrorCode::InvalidStatement
        );
        document.selected_range = Some(10..18);
        let mut selected_target = target(tab, connection);
        selected_target.selected_range = Some(10..18);
        let broker = ToolBroker::new(Some(selected_target));
        assert_eq!(
            broker.plan_run(call(), &request, &document).unwrap().sql,
            "SELECT 2"
        );
        document.busy = true;
        assert_eq!(
            broker
                .plan_run(call(), &request, &document)
                .unwrap_err()
                .code,
            ToolErrorCode::TabBusy
        );
    }

    #[test]
    fn row_and_text_outputs_respect_byte_limits() {
        let rows = vec![vec![Some("x".repeat(MAX_TOOL_OUTPUT_BYTES))], vec![None]];
        let bounded = bound_rows(&rows, 0, 2).unwrap();
        assert_eq!(bounded.rows, [vec![None]]);
        assert!(bounded.truncated);
        assert_eq!(bounded.omitted_row_offsets, [0]);
        assert_eq!(bounded.next_offset, 2);
        let text = format!("{}é", "x".repeat(MAX_TOOL_PAYLOAD_BYTES));
        let (bounded, truncated) = bound_text(&text);
        assert_eq!(bounded.len(), MAX_TOOL_PAYLOAD_BYTES);
        assert!(truncated);
    }

    #[test]
    fn tool_inputs_reject_unknown_fields() {
        let (tab, connection) = ids();
        let input = serde_json::json!({
            "version": 1,
            "tab_id": tab,
            "connection_id": connection,
            "editor_revision": 7,
            "edits": [],
            "shell": "rm -rf /"
        });

        assert!(serde_json::from_value::<EditRequest>(input).is_err());
    }

    #[test]
    fn broker_rejects_missing_target_wrong_version_and_oversized_ids() {
        let (tab, connection) = ids();
        let request = edit_request(
            tab,
            connection,
            vec![TextEdit {
                start: 0,
                end: 0,
                replacement: "SELECT 1".into(),
            }],
        );
        let document = document(tab, connection, "");
        assert_eq!(
            ToolBroker::new(None)
                .plan_edit(call(), &request, &document)
                .unwrap_err()
                .code,
            ToolErrorCode::NoActionTarget
        );
        let broker = ToolBroker::new(Some(target(tab, connection)));
        let mut wrong_version = request.clone();
        wrong_version.version += 1;
        assert_eq!(
            broker
                .plan_edit(call(), &wrong_version, &document)
                .unwrap_err()
                .code,
            ToolErrorCode::CapabilityMissing
        );
        let long_id = "x".repeat(MAX_HARNESS_ID_BYTES + 1);
        assert_eq!(
            broker
                .plan_edit(
                    CallIdentity {
                        conversation_id: &long_id,
                        turn_id: "turn",
                    },
                    &request,
                    &document,
                )
                .unwrap_err()
                .code,
            ToolErrorCode::InvalidArguments
        );
        assert_eq!(
            broker
                .plan_edit(
                    CallIdentity {
                        conversation_id: "another-conversation",
                        turn_id: "turn",
                    },
                    &request,
                    &document,
                )
                .unwrap_err()
                .code,
            ToolErrorCode::StaleTarget
        );
        let reassigned = EditorDocument {
            connection_id: Some(Uuid::new_v4()),
            ..document
        };
        assert_eq!(
            broker
                .plan_edit(call(), &request, &reassigned)
                .unwrap_err()
                .code,
            ToolErrorCode::StaleTarget
        );
    }

    #[test]
    fn edit_and_read_limits_fail_closed() {
        let (tab, connection) = ids();
        let broker = ToolBroker::new(Some(target(tab, connection)));
        let edits = (0..=MAX_TEXT_EDITS)
            .map(|_| TextEdit {
                start: 0,
                end: 0,
                replacement: String::new(),
            })
            .collect();
        assert_eq!(
            broker
                .plan_edit(
                    call(),
                    &edit_request(tab, connection, edits),
                    &document(tab, connection, ""),
                )
                .unwrap_err()
                .code,
            ToolErrorCode::LimitReached
        );
        let large = edit_request(
            tab,
            connection,
            vec![TextEdit {
                start: 0,
                end: 0,
                replacement: "x".repeat(MAX_EDIT_BYTES + 1),
            }],
        );
        assert_eq!(
            broker
                .plan_edit(call(), &large, &document(tab, connection, ""))
                .unwrap_err()
                .code,
            ToolErrorCode::LimitReached
        );
        assert_eq!(
            bound_rows(&[], 0, 0).unwrap_err().code,
            ToolErrorCode::InvalidArguments
        );
        assert_eq!(
            bound_rows(&[], 0, MAX_TOOL_ROWS + 1).unwrap_err().code,
            ToolErrorCode::InvalidArguments
        );
    }

    #[test]
    fn run_rejects_selection_changes_and_invalid_ranges() {
        let (tab, connection) = ids();
        let request = RunRequest {
            version: TOOL_SCHEMA_VERSION,
            tab_id: tab,
            connection_id: connection,
            editor_revision: 7,
        };
        let mut selected_target = target(tab, connection);
        selected_target.selected_range = Some(0..8);
        let broker = ToolBroker::new(Some(selected_target));
        let mut document = document(tab, connection, "SELECT 1");
        assert_eq!(
            broker
                .plan_run(call(), &request, &document)
                .unwrap_err()
                .code,
            ToolErrorCode::StaleTarget
        );
        document.selected_range = Some(0..9);
        let mut invalid_target = target(tab, connection);
        invalid_target.selected_range = document.selected_range.clone();
        assert_eq!(
            ToolBroker::new(Some(invalid_target))
                .plan_run(call(), &request, &document)
                .unwrap_err()
                .code,
            ToolErrorCode::InvalidArguments
        );
    }

    #[test]
    fn ordinary_row_pages_advance_by_the_requested_window() {
        let rows = vec![vec![Some("a".into())], vec![None], vec![Some("c".into())]];

        let first = bound_rows(&rows, 0, 2).unwrap();
        assert_eq!(first.rows, rows[..2]);
        assert_eq!(first.next_offset, 2);
        assert!(!first.truncated);
        assert!(first.omitted_row_offsets.is_empty());

        let second = bound_rows(&rows, first.next_offset, 1).unwrap();
        assert_eq!(second.rows, rows[2..]);
        assert_eq!(second.next_offset, 3);
        assert!(!second.truncated);
    }
}
