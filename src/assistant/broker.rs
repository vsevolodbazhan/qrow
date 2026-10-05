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
/// Leading rows that a finished query returns, so a short result needs no `read_results` call.
pub const MAX_PREVIEW_ROWS: usize = 20;
pub const MAX_PREVIEW_BYTES: usize = 16 * 1024;
pub const MAX_CONTEXT_STATEMENTS: usize = 100;
/// The largest part of the tab SQL in the workspace context. `read_tab_sql`
/// reads the other parts.
pub const MAX_CONTEXT_SQL_BYTES: usize = 32 * 1024;
/// The largest part of the tab SQL that one `read_tab_sql` call returns.
pub const MAX_SQL_PAGE_BYTES: usize = 32 * 1024;
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
    /// The SQL of the tab, or the part of it around the selection when the
    /// SQL is longer than `MAX_CONTEXT_SQL_BYTES`.
    pub sql: String,
    /// The byte offset of `sql` in the SQL of the tab.
    pub sql_offset: usize,
    /// The length of the SQL of the tab in bytes.
    pub sql_bytes: usize,
    /// Whether `sql` holds only a part of the SQL of the tab.
    pub sql_truncated: bool,
    pub selected_range: Option<Range<usize>>,
    /// Byte ranges that `run_selected_tab_query` accepts as `statement_range`.
    /// When `sql_truncated` is true, only the statements in `sql`.
    pub statement_ranges: Vec<Range<usize>>,
    pub statement_ranges_truncated: bool,
    pub editor_revision: u64,
    pub results: ResultSummary,
    pub latest_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceContext {
    pub version: u32,
    /// The user's SQL layout. The assistant writes SQL in it, and Qrow uses it
    /// when it formats a long statement.
    pub sql_style: sql::SqlStyle,
    pub connections: Vec<ConnectionContext>,
    pub tabs: Vec<TabSummary>,
    pub selected_tab: Option<SelectedTabContext>,
    /// The schema catalog of the connection of `selected_tab`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<super::catalog::CatalogContext>,
    /// The assistant notes of the connection of `selected_tab`, when they
    /// are new to the conversation. An empty text removes earlier notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_notes: Option<String>,
    /// Whether the notes of an earlier message still apply.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub connection_notes_unchanged: bool,
}

/// Returns up to `MAX_CONTEXT_STATEMENTS` ranges of the statements that
/// overlap `part` of `sql`, and whether more statements exist.
pub fn context_statement_ranges(sql: &str, part: &Range<usize>) -> (Vec<Range<usize>>, bool) {
    let ranges = sql::statement_ranges(sql);
    let total = ranges.len();
    let ranges: Vec<_> = ranges
        .into_iter()
        .filter(|range| range.start < part.end && part.start < range.end)
        .take(MAX_CONTEXT_STATEMENTS)
        .collect();
    let truncated = ranges.len() < total;
    (ranges, truncated)
}

/// The byte range of at most `limit` bytes of `sql` around `focus`, for
/// example the selection. The range starts and ends on character boundaries.
/// It holds all of `sql` when `sql` is not longer than `limit`.
pub fn sql_window(sql: &str, focus: &Range<usize>, limit: usize) -> Range<usize> {
    if sql.len() <= limit {
        return 0..sql.len();
    }
    let focus_start = focus.start.min(sql.len());
    let focus_end = focus.end.clamp(focus_start, sql.len());
    let spare = limit.saturating_sub(focus_end - focus_start);
    let mut start = focus_start.saturating_sub(spare / 2);
    let mut end = start + limit;
    if end > sql.len() {
        end = sql.len();
        start = end - limit;
    }
    sql.ceil_char_boundary(start)..sql.floor_char_boundary(end)
}

/// The byte range of the part of `sql` that starts at `offset` and has at
/// most `limit` bytes. The part ends on a character boundary, and it has at
/// least one character when `offset` is before the end. Returns `None` when
/// `offset` is past the end or inside a character.
pub fn sql_page(sql: &str, offset: usize, limit: usize) -> Option<Range<usize>> {
    if offset > sql.len() || !sql.is_char_boundary(offset) {
        return None;
    }
    let mut end = sql.floor_char_boundary(offset.saturating_add(limit).min(sql.len()));
    if end == offset && offset < sql.len() {
        end = sql.ceil_char_boundary(offset + 1);
    }
    Some(offset..end)
}

impl WorkspaceContext {
    pub fn new(
        sql_style: sql::SqlStyle,
        connections: Vec<ConnectionContext>,
        tabs: Vec<TabSummary>,
        selected_tab: Option<SelectedTabContext>,
    ) -> Self {
        Self {
            version: TOOL_SCHEMA_VERSION,
            sql_style,
            connections,
            tabs,
            selected_tab,
            catalog: None,
            connection_notes: None,
            connection_notes_unchanged: false,
        }
    }

    pub fn with_catalog(mut self, catalog: Option<super::catalog::CatalogContext>) -> Self {
        self.catalog = catalog;
        self
    }

    pub fn with_notes(mut self, notes: super::notes::ContextNotes) -> Self {
        use super::notes::ContextNotes;
        (self.connection_notes, self.connection_notes_unchanged) = match notes {
            ContextNotes::None => (None, false),
            ContextNotes::Send(text) => (Some(text), false),
            ContextNotes::Unchanged => (None, true),
        };
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionTarget {
    pub conversation_id: String,
    pub turn_id: String,
    pub tab_id: Uuid,
    pub connection_id: Option<Uuid>,
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
    /// The layout for SQL that Qrow formats in this document.
    pub sql_style: sql::SqlStyle,
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
    pub connection_id: Option<Uuid>,
    pub editor_revision: u64,
    pub edits: Vec<TextEdit>,
    #[serde(default)]
    pub replace_existing: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppendRequest {
    pub version: u32,
    pub tab_id: Uuid,
    pub connection_id: Option<Uuid>,
    pub editor_revision: u64,
    pub sql: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RunRequest {
    pub version: u32,
    pub tab_id: Uuid,
    pub connection_id: Uuid,
    pub editor_revision: u64,
    pub statement_range: Option<Range<usize>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditPlan {
    pub tab_id: Uuid,
    pub expected_revision: u64,
    pub sql: String,
    /// The applied edits in document order, with formatted replacements.
    pub edits: Vec<TextEdit>,
    pub formatted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppendPlan {
    pub tab_id: Uuid,
    pub sql: String,
    pub appended_range: Range<usize>,
    pub formatted: bool,
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
        if !document.sql.is_empty()
            && !request.replace_existing
            && request.edits.len() == 1
            && request.edits[0].start == 0
            && request.edits[0].end == document.sql.len()
        {
            return Err(ToolError::new(
                ToolErrorCode::InvalidArguments,
                "A new query must be appended. Set replace_existing only when the user asks to replace all SQL.",
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
        let mut edits = request.edits.clone();
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
        let mut sql = apply_edits(document.sql, &edits);
        let formatted = format_replaced_statements(&sql, &mut edits, document.sql_style);
        if formatted {
            sql = apply_edits(document.sql, &edits);
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
            edits,
            formatted,
        })
    }

    pub fn plan_append(
        &self,
        call: CallIdentity<'_>,
        request: &AppendRequest,
        document: &EditorDocument<'_>,
    ) -> Result<AppendPlan, ToolError> {
        self.validate_request(
            request.version,
            call,
            request.tab_id,
            request.connection_id,
            request.editor_revision,
            document,
        )?;
        let new_query = request.sql.trim();
        if request.sql.len() > MAX_EDIT_BYTES {
            return Err(ToolError::new(
                ToolErrorCode::LimitReached,
                "Edit content is too large.",
            ));
        }
        sql::validate_single(new_query)
            .map_err(|error| ToolError::new(ToolErrorCode::InvalidStatement, error.to_string()))?;
        // A comment that describes the query stays as written above it and
        // outside the statement range, which is the text that runs.
        let (comment, statement) = new_query.split_at(
            sql::statement_ranges(new_query)
                .first()
                .map_or(0, |range| range.start),
        );
        let formatted = sql::format_statement(statement, document.sql_style);
        let statement = formatted.as_deref().unwrap_or(statement);

        let mut sql = document.sql.to_owned();
        let last_token = sql::tokens(document.sql)
            .into_iter()
            .rev()
            .find(|(range, kind)| {
                *kind != sql::Kind::Comment && !document.sql[range.clone()].trim().is_empty()
            });
        if let Some((range, kind)) = last_token
            && kind != sql::Kind::Separator
        {
            sql.insert(range.end, ';');
        }
        if !sql.is_empty() {
            if !sql.ends_with('\n') {
                sql.push('\n');
            }
            if !sql.ends_with("\n\n") {
                sql.push('\n');
            }
        }
        sql.push_str(comment);
        let start = sql.len();
        sql.push_str(statement);
        if sql.len() > MAX_SQL_BYTES {
            return Err(ToolError::new(
                ToolErrorCode::LimitReached,
                "The edited query is too large.",
            ));
        }
        Ok(AppendPlan {
            tab_id: document.tab_id,
            appended_range: start..sql.len(),
            sql,
            formatted: formatted.is_some(),
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
            Some(request.connection_id),
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
        let range = request
            .statement_range
            .as_ref()
            .or(target.selected_range.as_ref());
        let sql = match range {
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
                    "The statement range is invalid.",
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
        connection_id: Option<Uuid>,
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
        if call.conversation_id != target.conversation_id || call.turn_id != target.turn_id {
            return Err(ToolError::new(
                ToolErrorCode::StaleTarget,
                "This assistant turn is no longer active.",
            ));
        }
        if tab_id != target.tab_id
            || connection_id != target.connection_id
            || document.tab_id != target.tab_id
            || document.connection_id != target.connection_id
        {
            return Err(ToolError::new(
                ToolErrorCode::StaleTarget,
                "The query tab or connection of this conversation changed. Read the workspace and retry.",
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

/// Rows from `offset` when the result metadata already uses `used_bytes`.
pub fn bound_rows_after(
    rows: &[Vec<Option<String>>],
    offset: usize,
    requested: usize,
    used_bytes: usize,
) -> Result<BoundedRows, ToolError> {
    bound_rows_within(
        rows,
        offset,
        requested,
        MAX_TOOL_PAYLOAD_BYTES.saturating_sub(used_bytes),
        true,
    )
}

/// Rows from `offset` for a query result that already uses `used_bytes` of the tool output.
pub fn preview_rows(rows: &[Vec<Option<String>>], offset: usize, used_bytes: usize) -> BoundedRows {
    let budget = MAX_PREVIEW_BYTES.min(MAX_TOOL_PAYLOAD_BYTES.saturating_sub(used_bytes));
    bound_rows_within(rows, offset, MAX_PREVIEW_ROWS, budget, false)
        .expect("the preview row count is within the tool row limit")
}

fn bound_rows_within(
    rows: &[Vec<Option<String>>],
    offset: usize,
    requested: usize,
    budget: usize,
    omit_unfittable: bool,
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
        let remaining = budget.saturating_sub(bytes.saturating_add(separator));
        let Some(full_row_bytes) = encoded_len_with_limit(row, MAX_TOOL_PAYLOAD_BYTES) else {
            omitted_row_offsets.push(offset + index);
            consumed += 1;
            continue;
        };
        if full_row_bytes > remaining {
            if output.is_empty() && omit_unfittable {
                omitted_row_offsets.push(offset + index);
                consumed += 1;
                continue;
            }
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

fn apply_edits(sql: &str, sorted_edits: &[TextEdit]) -> String {
    let mut sql = sql.to_owned();
    for edit in sorted_edits.iter().rev() {
        sql.replace_range(edit.start..edit.end, &edit.replacement);
    }
    sql
}

/// Formats each long statement of `edited` that one replacement
/// supplies completely, apart from its separator. A statement that keeps text
/// from before the edit keeps its layout. Returns whether a replacement changed.
fn format_replaced_statements(
    edited: &str,
    sorted_edits: &mut [TextEdit],
    style: sql::SqlStyle,
) -> bool {
    let statements: Vec<_> = sql::statement_ranges(edited)
        .into_iter()
        .map(|range| {
            let body = edited[range.clone()].trim_end_matches(';').trim_end();
            range.start..range.start + body.len()
        })
        .collect();
    let mut formatted = false;
    let mut shift = 0_isize;
    for edit in sorted_edits {
        let start = edit.start.saturating_add_signed(shift);
        let end = start + edit.replacement.len();
        shift += edit.replacement.len() as isize - (edit.end - edit.start) as isize;
        for range in statements
            .iter()
            .rev()
            .filter(|range| start <= range.start && range.end <= end)
        {
            if let Some(text) = sql::format_statement(&edited[range.clone()], style) {
                edit.replacement
                    .replace_range(range.start - start..range.end - start, &text);
                formatted = true;
            }
        }
    }
    formatted
}

#[cfg(test)]
mod tests;
