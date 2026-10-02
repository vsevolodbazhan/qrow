use super::assistant_view::{
    AppendedQuery, PendingQuery, PendingQueryKind, ToolActivity, ToolKind, ToolState,
    TranscriptEntry,
};
use super::*;
use crate::assistant::broker::ActionTarget;
use crate::assistant::{
    ToolCall, ToolResult,
    broker::{
        AppendRequest, CallIdentity, EditRequest, EditorDocument, MAX_SQL_BYTES,
        MAX_SQL_PAGE_BYTES, MAX_TOOL_OUTPUT_BYTES, RunRequest, TOOL_SCHEMA_VERSION, ToolBroker,
        bound_rows_after, bound_text, context_statement_ranges, preview_rows, sql_page,
    },
    catalog,
    service::Command as AssistantCommand,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::ops::Range;

fn remap_selection(
    selection: Range<usize>,
    edits: &[crate::assistant::broker::TextEdit],
) -> Option<Range<usize>> {
    let mut shift = 0_i64;
    for edit in edits {
        if edit.end <= selection.start {
            shift += edit.replacement.len() as i64 - (edit.end - edit.start) as i64;
        } else if edit.start < selection.end {
            return None;
        }
    }
    let start = (selection.start as i64).checked_add(shift)?;
    let end = (selection.end as i64).checked_add(shift)?;
    Some(usize::try_from(start).ok()?..usize::try_from(end).ok()?)
}

/// Maps an unknown tab ID to the conversation tab for read-only tools. A real
/// ID of another tab stays available for reads.
fn resolve_tab_id(
    requested: Uuid,
    conversation_tab: Option<Uuid>,
    known: impl IntoIterator<Item = Uuid>,
) -> Uuid {
    if known.into_iter().any(|id| id == requested) {
        requested
    } else {
        conversation_tab.unwrap_or(requested)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionInput {
    version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TabInput {
    version: u32,
    tab_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SqlPageInput {
    version: u32,
    tab_id: Uuid,
    /// The byte offset of the first byte to read. The default is 0.
    #[serde(default)]
    offset: usize,
    /// The largest number of bytes to read. The default and maximum is
    /// `MAX_SQL_PAGE_BYTES`.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetInput {
    version: u32,
    tab_id: Uuid,
    connection_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunInput {
    version: u32,
    tab_id: Uuid,
    connection_id: Uuid,
    editor_revision: Option<u64>,
    statement_range: Option<Range<usize>>,
}

fn run_request(
    input: RunInput,
    appended: Option<&AppendedQuery>,
    turn_id: &str,
    document: &EditorDocument<'_>,
) -> Result<RunRequest, ToolResult> {
    let editor_revision = match input.editor_revision {
        Some(revision) => revision,
        None if input.statement_range.is_none()
            && appended.is_some_and(|append| {
                append.turn_id == turn_id
                    && append.tab_id == document.tab_id
                    && append.connection_id == document.connection_id
                    && append.revision == document.revision
                    && document.selected_range.as_ref() == Some(&append.selected_range)
            }) =>
        {
            document.revision
        }
        None => {
            return Err(failure(
                "invalid_arguments",
                "Provide editor_revision to run SQL other than the statement just appended and selected in this turn.",
            ));
        }
    };
    Ok(RunRequest {
        version: input.version,
        tab_id: input.tab_id,
        connection_id: input.connection_id,
        editor_revision,
        statement_range: input.statement_range,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RowsInput {
    version: u32,
    tab_id: Uuid,
    offset: usize,
    count: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogsInput {
    version: u32,
    tab_id: Uuid,
    scope: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemasInput {
    version: u32,
    connection_id: Uuid,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationsInput {
    version: u32,
    connection_id: Uuid,
    schema: String,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeInput {
    version: u32,
    connection_id: Uuid,
    schema: String,
    relation: String,
}

/// What a catalog tool call reads.
enum CatalogRequest {
    Schemas(SchemasInput),
    Relations(RelationsInput),
    Relation(DescribeInput),
}

impl CatalogRequest {
    fn parse(call: &ToolCall) -> Result<Self, ToolResult> {
        let arguments = call.arguments.clone();
        let (request, requested) = match call.name.as_str() {
            "list_schemas" => {
                let input: SchemasInput = parse(arguments)?;
                (input.version, Self::Schemas(input))
            }
            "list_relations" => {
                let input: RelationsInput = parse(arguments)?;
                (input.version, Self::Relations(input))
            }
            _ => {
                let input: DescribeInput = parse(arguments)?;
                (input.version, Self::Relation(input))
            }
        };
        version(request)?;
        Ok(requested)
    }

    fn connection_id(&self) -> Uuid {
        match self {
            Self::Schemas(input) => input.connection_id,
            Self::Relations(input) => input.connection_id,
            Self::Relation(input) => input.connection_id,
        }
    }
}

/// The longest time that a catalog tool call waits for the cache or for a
/// refresh. A refresh continues after it.
const MAX_CATALOG_WAIT: Duration = Duration::from_secs(120);

/// A catalog tool call that waits for the cache to load or for a refresh.
pub(in crate::ui) struct PendingCatalogCall {
    call: ToolCall,
    deadline: Instant,
    /// The idle reports of the catalog worker when the call started a
    /// refresh. The call does not start another one, and it waits until the
    /// worker reports again.
    refreshed: Option<u64>,
    connection: Uuid,
}

/// The next step of a catalog tool call.
enum CatalogStep {
    Done(ToolResult),
    /// Wait for the cache or a refresh. `refreshed` is true when this step
    /// started the refresh.
    Wait {
        refreshed: bool,
    },
}

fn failure(code: &str, message: impl Into<String>) -> ToolResult {
    ToolResult {
        success: false,
        content: json!({"version": TOOL_SCHEMA_VERSION,
        "error": {"code": code, "message": message.into()}}),
    }
}

fn success(value: Value) -> ToolResult {
    if serde_json::to_vec(&value).is_ok_and(|bytes| bytes.len() <= MAX_TOOL_OUTPUT_BYTES) {
        ToolResult {
            success: true,
            content: value,
        }
    } else {
        failure("limit_reached", "The assistant tool output is too large.")
    }
}

/// Names the current step of a running query from the tab status, for example
/// `Executing` for `Executing…`. The spinner already shows that work continues.
fn running_step(status: &str) -> String {
    let step = status
        .split(" · ")
        .next()
        .unwrap_or(status)
        .trim_end_matches('…');
    if step.is_empty() {
        "Running".into()
    } else {
        step.into()
    }
}

/// Summarizes a finished query as its downloaded rows and duration.
fn query_outcome(rows: usize, more: bool, elapsed: Option<Duration>) -> String {
    let rows = match (rows, more) {
        (1, false) => "1 row".to_owned(),
        (rows, false) => format!("{rows} rows"),
        (rows, true) => format!("{rows}+ rows"),
    };
    match elapsed {
        Some(elapsed) => format!("{rows} · {:.2} s", elapsed.as_secs_f64()),
        None => rows,
    }
}

/// Returns the error message of a failed tool result as the first detail line.
fn error_line(result: &ToolResult) -> String {
    if result.success {
        return String::new();
    }
    result
        .content
        .pointer("/error/message")
        .and_then(Value::as_str)
        .map_or_else(String::new, |message| format!("Error: {message}\n"))
}

fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, ToolResult> {
    serde_json::from_value(value).map_err(|_| {
        failure(
            "invalid_arguments",
            "The assistant tool arguments are invalid.",
        )
    })
}

fn version(version: u32) -> Result<(), ToolResult> {
    if version == TOOL_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(failure(
            "capability_missing",
            "The assistant tool version is not supported.",
        ))
    }
}

fn tab_closed() -> ToolResult {
    failure(
        "tab_closed",
        "The query tab of this conversation is closed. Wait for a new user instruction.",
    )
}

impl Qrow {
    fn tool_tab_id(&self, call: &ToolCall, requested: Uuid) -> Uuid {
        resolve_tab_id(
            requested,
            self.thread_tab_index(&call.thread_id)
                .map(|index| self.tabs[index].saved.id),
            self.tabs.iter().map(|tab| tab.saved.id),
        )
    }

    /// The index of the query tab that a call can change.
    fn tool_tab_index(&self, call: &ToolCall) -> Result<usize, ToolResult> {
        self.thread_tab_index(&call.thread_id)
            .ok_or_else(tab_closed)
    }

    fn tool_target(&self, call: &ToolCall) -> Option<ActionTarget> {
        self.thread_run(&call.thread_id)
            .and_then(|run| run.target.clone())
    }

    pub(super) fn answer_assistant_call(
        &mut self,
        call: ToolCall,
        ok: bool,
        content: Value,
        cx: &mut Context<Self>,
    ) {
        self.assistant_command(
            AssistantCommand::Answer {
                call,
                result: ToolResult {
                    success: ok,
                    content,
                },
            },
            cx,
        );
    }

    pub(super) fn handle_assistant_tool(
        &mut self,
        call: ToolCall,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.assistant.conversation(&call.thread_id).is_none()
            || self
                .thread_run(&call.thread_id)
                .and_then(|run| run.active_turn.as_deref())
                != Some(call.turn_id.as_str())
        {
            self.answer_assistant_call(
                call,
                false,
                failure("stale_target", "The conversation or turn changed.").content,
                cx,
            );
            return;
        }
        if let Some(result) = self.dispatch_assistant_tool(&call, window, cx) {
            self.finish_assistant_tool(call, result, cx);
        }
        cx.notify();
    }

    /// Record a tool call in the conversation and answer it.
    fn finish_assistant_tool(
        &mut self,
        call: ToolCall,
        result: ToolResult,
        cx: &mut Context<Self>,
    ) {
        let name = call.name.clone();
        {
            let target = result
                .content
                .get("tab_id")
                .or_else(|| call.arguments.get("tab_id"))
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
                .and_then(|id| self.tabs.iter().find(|tab| tab.saved.id == id))
                .map(|tab| tab.saved.title.clone());
            let (detail, _) = bound_text(&format!(
                "{}Arguments:\n{}\nResult:\n{}",
                error_line(&result),
                call.arguments,
                result.content
            ));
            let tool = ToolActivity {
                kind: ToolKind::from_name(&name),
                target,
                state: if result.success {
                    ToolState::Done(None)
                } else {
                    ToolState::Failed
                },
            };
            self.assistant_state
                .transcripts
                .entry(call.thread_id.clone())
                .or_default()
                .push(TranscriptEntry::tool(tool, call.turn_id.clone()).with_detail(detail));
            self.answer_assistant_call(call, result.success, result.content, cx);
        }
    }

    fn dispatch_assistant_tool(
        &mut self,
        call: &ToolCall,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ToolResult> {
        let result = match call.name.as_str() {
            "get_workspace_context" => self.tool_workspace(call, cx),
            "read_tab_sql" => self.tool_read_sql(call, cx),
            "append_selected_tab_sql" => self.tool_append(call, window, cx),
            "edit_selected_tab_sql" => self.tool_edit(call, window, cx),
            "run_selected_tab_query" => return self.tool_run(call, window, cx),
            "cancel_selected_tab_query" => self.tool_cancel(call, cx),
            "get_query_status" => self.tool_status(call, cx),
            "read_results" => self.tool_results(call, cx),
            "fetch_more_results" => return self.tool_fetch(call, cx),
            "read_query_logs" => self.tool_logs(call),
            "list_schemas" | "list_relations" | "describe_relation" => {
                return self.tool_catalog(call, cx);
            }
            _ => Err(failure(
                "capability_missing",
                "This assistant tool is not available.",
            )),
        };
        Some(result.unwrap_or_else(|error| error))
    }

    fn tool_workspace(&mut self, call: &ToolCall, cx: &App) -> Result<ToolResult, ToolResult> {
        let args: VersionInput = parse(call.arguments.clone())?;
        version(args.version)?;
        let context = self.assistant_context(&call.thread_id, cx);
        // This read binds later actions to the current state of the tab.
        let target = self
            .assistant_target(&call.thread_id, cx)
            .map(|mut target| {
                target.turn_id = call.turn_id.clone();
                target
            });
        self.thread_run_mut(&call.thread_id).target = target;
        Ok(success(context))
    }

    fn tool_read_sql(&self, call: &ToolCall, cx: &App) -> Result<ToolResult, ToolResult> {
        let mut args: SqlPageInput = parse(call.arguments.clone())?;
        version(args.version)?;
        args.tab_id = self.tool_tab_id(call, args.tab_id);
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.saved.id == args.tab_id)
            .ok_or_else(|| {
                failure(
                    "invalid_arguments",
                    "The query tab was not found. Read the workspace and use the exact tab ID it returns.",
                )
            })?;
        let sql = tab.input.read(cx).value().to_string();
        if sql.len() > MAX_SQL_BYTES {
            return Err(failure("limit_reached", "The SQL text is too large."));
        }
        let mut limit = args
            .limit
            .unwrap_or(MAX_SQL_PAGE_BYTES)
            .clamp(1, MAX_SQL_PAGE_BYTES);
        loop {
            let page = sql_page(&sql, args.offset, limit).ok_or_else(|| {
                failure(
                    "invalid_arguments",
                    "The offset is past the end of the SQL or inside a character. Use 0 or next_offset.",
                )
            })?;
            let (statement_ranges, statement_ranges_truncated) =
                context_statement_ranges(&sql, &page);
            let next_offset = (page.end < sql.len()).then_some(page.end);
            let value = json!({"version": 1, "tab_id": args.tab_id,
                "connection_id": tab.saved.profile, "editor_revision": tab.revision,
                "sql": &sql[page.clone()], "sql_offset": page.start, "sql_bytes": sql.len(),
                "next_offset": next_offset, "statement_ranges": statement_ranges,
                "statement_ranges_truncated": statement_ranges_truncated});
            // Escaped characters make the JSON of a page larger than its SQL.
            let fits =
                serde_json::to_vec(&value).is_ok_and(|bytes| bytes.len() <= MAX_TOOL_OUTPUT_BYTES);
            if fits || sql[page.clone()].chars().nth(1).is_none() {
                return Ok(success(value));
            }
            limit = page.len() / 2;
        }
    }

    fn tool_edit(
        &mut self,
        call: &ToolCall,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<ToolResult, ToolResult> {
        let mut args: EditRequest = parse(call.arguments.clone())?;
        let index = self.tool_tab_index(call)?;
        let tab = &self.tabs[index];
        args.tab_id = tab.saved.id;
        args.connection_id = tab.saved.profile;
        let sql = tab.input.read(cx).value().to_string();
        let selected = tab.input.read(cx).selected_range();
        let document = EditorDocument {
            tab_id: tab.saved.id,
            connection_id: tab.saved.profile,
            revision: tab.revision,
            sql: &sql,
            selected_range: (!selected.is_empty()).then_some(selected.clone()),
            busy: tab.busy,
            sql_style: self.settings.sql_style(),
        };
        let plan = ToolBroker::new(self.tool_target(call))
            .plan_edit(
                CallIdentity {
                    conversation_id: &call.thread_id,
                    turn_id: &call.turn_id,
                },
                &args,
                &document,
            )
            .map_err(|error| ToolResult {
                success: false,
                content: json!({"version": 1, "error": error}),
            })?;
        let editor = tab.input.clone();
        let appended_range = plan
            .edits
            .iter()
            .any(|edit| {
                edit.start == sql.len()
                    && edit.end == sql.len()
                    && !edit.replacement.trim().is_empty()
            })
            .then(|| crate::sql::last_statement_range(&plan.sql))
            .flatten()
            .filter(|range| range.start >= sql.len());
        let mapped = appended_range
            .clone()
            .or_else(|| remap_selection(selected, &plan.edits));
        self.tabs[index].revision = self.tabs[index].revision.saturating_add(1);
        self.tabs[index].pending_assistant_edit = Some(plan.sql.clone());
        editor.update(cx, |editor, cx| {
            let scroll = editor.scroll_offset();
            editor.replace_all(plan.sql.clone(), window, cx);
            if let Some(range) = mapped {
                editor.set_selected_range(range, cx);
            }
            editor.set_scroll_offset(scroll, cx);
        });
        if let Some(range) = appended_range
            && let Some(target) = &mut self.thread_run_mut(&call.thread_id).target
        {
            target.selected_range = Some(range);
        }
        let revision = self.tabs[index].revision;
        let selected = editor.read(cx).selected_range();
        self.changed(cx);
        // A run without statement_range uses this selection, so the model does not read it back.
        Ok(success(
            json!({"version": 1, "tab_id": plan.tab_id, "editor_revision": revision,
                "sql_bytes": plan.sql.len(), "formatted": plan.formatted,
                "selected_range": (!selected.is_empty()).then_some(selected)}),
        ))
    }

    fn tool_append(
        &mut self,
        call: &ToolCall,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<ToolResult, ToolResult> {
        let mut args: AppendRequest = parse(call.arguments.clone())?;
        let index = self.tool_tab_index(call)?;
        let tab = &self.tabs[index];
        args.tab_id = tab.saved.id;
        args.connection_id = tab.saved.profile;
        let sql = tab.input.read(cx).value().to_string();
        let document = EditorDocument {
            tab_id: tab.saved.id,
            connection_id: tab.saved.profile,
            revision: tab.revision,
            sql: &sql,
            selected_range: None,
            busy: tab.busy,
            sql_style: self.settings.sql_style(),
        };
        let plan = ToolBroker::new(self.tool_target(call))
            .plan_append(
                CallIdentity {
                    conversation_id: &call.thread_id,
                    turn_id: &call.turn_id,
                },
                &args,
                &document,
            )
            .map_err(|error| ToolResult {
                success: false,
                content: json!({"version": 1, "error": error}),
            })?;
        let editor = tab.input.clone();
        self.tabs[index].revision = self.tabs[index].revision.saturating_add(1);
        self.tabs[index].pending_assistant_edit = Some(plan.sql.clone());
        editor.update(cx, |editor, cx| {
            editor.replace_all(plan.sql.clone(), window, cx);
            editor.set_selected_range(plan.appended_range.clone(), cx);
        });
        if let Some(target) = &mut self.thread_run_mut(&call.thread_id).target {
            target.selected_range = Some(plan.appended_range.clone());
        }
        let revision = self.tabs[index].revision;
        self.thread_run_mut(&call.thread_id).appended_query = Some(AppendedQuery {
            turn_id: call.turn_id.clone(),
            tab_id: plan.tab_id,
            connection_id: args.connection_id,
            revision,
            selected_range: plan.appended_range.clone(),
        });
        self.changed(cx);
        Ok(success(
            json!({"version": 1, "tab_id": plan.tab_id, "editor_revision": revision,
                "sql_bytes": plan.sql.len(), "statement_range": plan.appended_range,
                "formatted": plan.formatted}),
        ))
    }

    fn tool_run(
        &mut self,
        call: &ToolCall,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ToolResult> {
        let result = (|| {
            let input: RunInput = parse(call.arguments.clone())?;
            let index = self.tool_tab_index(call)?;
            let tab = &self.tabs[index];
            let sql = tab.input.read(cx).value().to_string();
            let selected = tab.input.read(cx).selected_range();
            let document = EditorDocument {
                tab_id: tab.saved.id,
                connection_id: tab.saved.profile,
                revision: tab.revision,
                sql: &sql,
                selected_range: (!selected.is_empty()).then_some(selected),
                busy: tab.busy,
                sql_style: self.settings.sql_style(),
            };
            let appended = self
                .thread_run(&call.thread_id)
                .and_then(|run| run.appended_query.as_ref());
            let mut args = run_request(input, appended, &call.turn_id, &document)?;
            args.tab_id = tab.saved.id;
            args.connection_id = tab.saved.profile.ok_or_else(|| {
                failure(
                    "no_action_target",
                    "Connect the query tab of this conversation before running SQL.",
                )
            })?;
            let plan = ToolBroker::new(self.tool_target(call))
                .plan_run(
                    CallIdentity {
                        conversation_id: &call.thread_id,
                        turn_id: &call.turn_id,
                    },
                    &args,
                    &document,
                )
                .map_err(|error| ToolResult {
                    success: false,
                    content: json!({"version": 1, "error": error}),
                })?;
            if self
                .thread_run(&call.thread_id)
                .is_some_and(|run| run.pending_query.is_some())
            {
                return Err(failure(
                    "tab_busy",
                    "Another assistant query request is pending.",
                ));
            }
            if let Some(range) = args.statement_range {
                tab.input.update(cx, |editor, cx| {
                    editor.set_selected_range(range.clone(), cx);
                });
                if let Some(target) = &mut self.thread_run_mut(&call.thread_id).target {
                    target.selected_range = Some(range);
                }
            }
            let mode = self
                .assistant
                .conversations
                .iter()
                .find(|conversation| conversation.thread_id == call.thread_id)
                .map(|conversation| conversation.execution_mode)
                .unwrap_or_default();
            self.thread_run_mut(&call.thread_id).pending_query = Some(PendingQuery {
                call: call.clone(),
                tab_id: plan.tab_id,
                revision: plan.expected_revision,
                sql: plan.sql,
                approved: mode == crate::model::AssistantExecutionMode::RunAutomatically,
                started: false,
                kind: PendingQueryKind::Run,
                activity_index: None,
                detached: false,
                first_row: 0,
            });
            if mode == crate::model::AssistantExecutionMode::RunAutomatically {
                self.begin_assistant_query(&call.thread_id, window, cx);
            }
            Ok(())
        })();
        result.err()
    }

    pub(super) fn approve_assistant_query(
        &mut self,
        thread_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pending) = &mut self.thread_run_mut(thread_id).pending_query {
            pending.approved = true;
        }
        self.begin_assistant_query(thread_id, window, cx);
    }

    pub(super) fn cancel_assistant_approval(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        if let Some(pending) = self.thread_run_mut(thread_id).pending_query.take() {
            self.record_assistant_query_cancelled(&pending, "You cancelled this query request.");
            self.answer_assistant_call(
                pending.call,
                false,
                failure(
                    "approval_cancelled",
                    "The user cancelled this query request.",
                )
                .content,
                cx,
            );
        }
        cx.notify();
    }

    fn begin_assistant_query(
        &mut self,
        thread_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self
            .thread_run(thread_id)
            .and_then(|run| run.pending_query.as_ref())
        else {
            return;
        };
        if !pending.approved || pending.started {
            return;
        }
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.saved.id == pending.tab_id)
            .filter(|index| self.thread_tab_index(thread_id) == Some(*index));
        let valid = index.is_some_and(|index| {
            let tab = &self.tabs[index];
            if tab.revision != pending.revision || tab.busy {
                return false;
            }
            let sql = tab.input.read(cx).value();
            let selected = tab.input.read(cx).selected_range();
            let text = if selected.is_empty() {
                sql.as_ref()
            } else {
                sql.get(selected).unwrap_or("")
            };
            text == pending.sql
        });
        let (Some(index), true) = (index, valid) else {
            let pending = self.thread_run_mut(thread_id).pending_query.take().unwrap();
            self.answer_assistant_call(
                pending.call,
                false,
                failure(
                    "stale_revision",
                    "The query target changed. Send a new instruction before running.",
                )
                .content,
                cx,
            );
            return;
        };
        let kind = pending.kind;
        if let Some(pending) = &mut self.thread_run_mut(thread_id).pending_query {
            pending.started = true;
        }
        let started = match kind {
            PendingQueryKind::Run => self.run_tab_query(index, window, cx),
            PendingQueryKind::Fetch => {
                self.next_tab_page(index, cx);
                self.tabs[index].busy
            }
        };
        if !started {
            let pending = self.thread_run_mut(thread_id).pending_query.take().unwrap();
            self.answer_assistant_call(
                pending.call,
                false,
                failure(
                    "query_not_started",
                    "Qrow could not start this query. Check the query tab and connection.",
                )
                .content,
                cx,
            );
            return;
        }
        self.record_assistant_query_started(thread_id, cx);
        self.complete_assistant_query(thread_id, cx);
    }

    /// Describes an assistant query request as a tool call card.
    pub(super) fn assistant_query_tool(
        &self,
        pending: &PendingQuery,
        state: ToolState,
    ) -> ToolActivity {
        ToolActivity {
            kind: match pending.kind {
                PendingQueryKind::Run => ToolKind::RunQuery,
                PendingQueryKind::Fetch => ToolKind::FetchRows,
            },
            target: self
                .tabs
                .iter()
                .find(|tab| tab.saved.id == pending.tab_id)
                .map(|tab| tab.saved.title.clone()),
            state,
        }
    }

    /// Records a query request that Qrow did not run, with the reason.
    pub(super) fn record_assistant_query_cancelled(
        &mut self,
        pending: &PendingQuery,
        reason: &str,
    ) {
        let entry = TranscriptEntry::tool(
            self.assistant_query_tool(pending, ToolState::Cancelled),
            pending.call.turn_id.clone(),
        )
        .with_detail(bound_text(&format!("{reason}\nQuery:\n{}", pending.sql)).0);
        self.assistant_state
            .transcripts
            .entry(pending.call.thread_id.clone())
            .or_default()
            .push(entry);
    }

    fn record_assistant_query_started(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        let Some(pending) = self
            .thread_run(thread_id)
            .and_then(|run| run.pending_query.as_ref())
        else {
            return;
        };
        let thread = pending.call.thread_id.clone();
        let turn = pending.call.turn_id.clone();
        let tool = self.assistant_query_tool(pending, ToolState::Running("Preparing".into()));
        let detail = bound_text(&format!("Query:\n{}", pending.sql)).0;
        let entries = self.assistant_state.transcripts.entry(thread).or_default();
        let index = entries.len();
        entries.push(TranscriptEntry::tool(tool, turn).with_detail(detail));
        if let Some(pending) = &mut self.thread_run_mut(thread_id).pending_query {
            pending.activity_index = Some(index);
        }
        cx.notify();
    }

    /// Updates the tool card of a running assistant query in each
    /// conversation, and answers the calls of queries that ended.
    pub(super) fn tick_assistant_queries(&mut self, cx: &mut Context<Self>) {
        let threads: Vec<_> = self
            .assistant_state
            .runs
            .iter()
            .filter(|(_, run)| run.pending_query.is_some())
            .map(|(thread, _)| thread.clone())
            .collect();
        for thread in threads {
            self.update_assistant_query_progress(&thread, cx);
            self.complete_assistant_query(&thread, cx);
        }
    }

    fn update_assistant_query_progress(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        let Some(pending) = self
            .thread_run(thread_id)
            .and_then(|run| run.pending_query.as_ref())
        else {
            return;
        };
        let Some(index) = pending.activity_index else {
            return;
        };
        let Some(tab) = self.tabs.iter().find(|tab| tab.saved.id == pending.tab_id) else {
            return;
        };
        let state = ToolState::Running(running_step(&tab.status));
        if let Some(entry) = self
            .assistant_state
            .transcripts
            .get_mut(thread_id)
            .and_then(|entries| entries.get_mut(index))
            && entry.tool.as_ref().is_some_and(|tool| tool.state != state)
        {
            entry.set_tool_state(state);
            cx.notify();
        }
    }

    fn complete_assistant_query(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        let Some(pending) = self
            .thread_run(thread_id)
            .and_then(|run| run.pending_query.as_ref())
        else {
            return;
        };
        if !pending.started {
            return;
        }
        let Some(tab) = self.tabs.iter().find(|tab| tab.saved.id == pending.tab_id) else {
            return;
        };
        if tab.busy {
            return;
        }
        let results = tab.table.read(cx);
        let results = results.delegate();
        let ok = !tab.status.starts_with("Error")
            && !tab.status.starts_with("Rejected")
            && !tab.status.starts_with("Cancelled");
        let mut content = json!({"version": 1, "tab_id": tab.saved.id, "status": tab.status,
            "columns": results.columns.iter().map(|column| column.name.as_str()).collect::<Vec<_>>(),
            "downloaded_rows": results.rows.len(), "more_rows_available": tab.more,
            "duration_seconds": tab.elapsed.map(|duration| duration.as_secs_f64())});
        if ok {
            // Returning the first rows saves a read_results call and a model turn.
            let used =
                serde_json::to_vec(&content).map_or(MAX_TOOL_OUTPUT_BYTES, |bytes| bytes.len());
            let preview = preview_rows(&results.rows, pending.first_row, used);
            content["rows"] = json!(preview.rows);
            content["next_offset"] = json!(preview.next_offset);
            content["more_downloaded_rows"] = json!(preview.next_offset < results.rows.len());
            if !preview.omitted_row_offsets.is_empty() {
                content["omitted_row_offsets"] = json!(preview.omitted_row_offsets);
            }
        }
        let state = if ok {
            ToolState::Done(Some(query_outcome(
                results.rows.len(),
                tab.more,
                tab.elapsed,
            )))
        } else if tab.status.starts_with("Cancelled") {
            ToolState::Cancelled
        } else {
            ToolState::Failed
        };
        let status = (!ok).then(|| format!("Status: {}\n", tab.status));
        let pending = self.thread_run_mut(thread_id).pending_query.take().unwrap();
        let entry = TranscriptEntry::tool(
            self.assistant_query_tool(&pending, state),
            pending.call.turn_id.clone(),
        )
        .with_detail(
            bound_text(&format!(
                "{}Query:\n{}\nResult:\n{}",
                status.unwrap_or_default(),
                pending.sql,
                content
            ))
            .0,
        );
        let entries = self
            .assistant_state
            .transcripts
            .entry(pending.call.thread_id.clone())
            .or_default();
        if let Some(existing) = pending
            .activity_index
            .and_then(|index| entries.get_mut(index))
        {
            *existing = entry;
        } else {
            entries.push(entry);
        }
        if !pending.detached {
            self.answer_assistant_call(pending.call, ok, content, cx);
        }
    }

    fn tool_cancel(
        &mut self,
        call: &ToolCall,
        cx: &mut Context<Self>,
    ) -> Result<ToolResult, ToolResult> {
        let mut args: TargetInput = parse(call.arguments.clone())?;
        version(args.version)?;
        let index = self.tool_tab_index(call)?;
        args.tab_id = self.tabs[index].saved.id;
        args.connection_id = self.tabs[index].saved.profile.ok_or_else(|| {
            failure(
                "no_action_target",
                "The query tab of this conversation has no connection.",
            )
        })?;
        let target = self
            .tool_target(call)
            .ok_or_else(|| failure("no_action_target", "Wait for a new user message."))?;
        if target.conversation_id != call.thread_id
            || target.turn_id != call.turn_id
            || target.tab_id != args.tab_id
            || target.connection_id != Some(args.connection_id)
            || self.tabs[index].saved.id != args.tab_id
        {
            return Err(failure(
                "stale_target",
                "The query tab or connection of this conversation changed. Read the workspace and retry.",
            ));
        }
        let was_running = self.tabs[index].busy;
        if was_running {
            self.cancel_tab(index, cx);
        }
        Ok(success(
            json!({"version": 1, "tab_id": args.tab_id, "cancellation_requested": was_running}),
        ))
    }

    fn tool_status(&self, call: &ToolCall, cx: &App) -> Result<ToolResult, ToolResult> {
        let mut args: TabInput = parse(call.arguments.clone())?;
        version(args.version)?;
        args.tab_id = self.tool_tab_id(call, args.tab_id);
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.saved.id == args.tab_id)
            .ok_or_else(|| failure("invalid_arguments", "The query tab was not found."))?;
        let data = tab.table.read(cx);
        let data = data.delegate();
        Ok(success(
            json!({"version": 1, "tab_id": args.tab_id, "status": tab.status,
            "running": tab.busy, "cancelling": tab.cancelling,
            "columns": data.columns.iter().map(|column| column.name.as_str()).collect::<Vec<_>>(),
            "downloaded_rows": data.rows.len(), "more_rows_available": tab.more,
            "latest_error": tab.output.latest_error().map(|entry| bound_text(&entry.text).0)}),
        ))
    }

    fn tool_results(&self, call: &ToolCall, cx: &App) -> Result<ToolResult, ToolResult> {
        let mut args: RowsInput = parse(call.arguments.clone())?;
        version(args.version)?;
        args.tab_id = self.tool_tab_id(call, args.tab_id);
        if self.tool_target(call).is_none() {
            return Err(failure("no_action_target", "Wait for a new user message."));
        }
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.saved.id == args.tab_id)
            .ok_or_else(|| failure("invalid_arguments", "The query tab was not found."))?;
        let data = tab.table.read(cx);
        let data = data.delegate();
        let mut content = json!({"version": 1, "tab_id": args.tab_id,
            "columns": data.columns.iter().map(|column| column.name.as_str()).collect::<Vec<_>>(),
            "downloaded_rows": data.rows.len(), "rows": [], "next_offset": args.offset,
            "more_downloaded_rows": false, "truncated": false, "omitted_row_offsets": []});
        let used = serde_json::to_vec(&content).map_or(MAX_TOOL_OUTPUT_BYTES, |bytes| bytes.len());
        let bounded =
            bound_rows_after(&data.rows, args.offset, args.count, used).map_err(|error| {
                ToolResult {
                    success: false,
                    content: json!({"version": 1, "error": error}),
                }
            })?;
        content["rows"] = json!(bounded.rows);
        content["next_offset"] = json!(bounded.next_offset);
        content["more_downloaded_rows"] = json!(bounded.next_offset < data.rows.len());
        content["truncated"] = json!(bounded.truncated);
        content["omitted_row_offsets"] = json!(bounded.omitted_row_offsets);
        Ok(success(content))
    }

    fn tool_fetch(&mut self, call: &ToolCall, cx: &mut Context<Self>) -> Option<ToolResult> {
        let result = (|| {
            let mut args: TargetInput = parse(call.arguments.clone())?;
            version(args.version)?;
            let index = self.tool_tab_index(call)?;
            args.tab_id = self.tabs[index].saved.id;
            args.connection_id = self.tabs[index].saved.profile.ok_or_else(|| {
                failure(
                    "no_action_target",
                    "The query tab of this conversation has no connection.",
                )
            })?;
            let target = self
                .tool_target(call)
                .ok_or_else(|| failure("no_action_target", "Wait for a new user message."))?;
            let tab = &self.tabs[index];
            if target.conversation_id != call.thread_id
                || target.turn_id != call.turn_id
                || target.tab_id != args.tab_id
                || target.connection_id != Some(args.connection_id)
                || tab.saved.id != args.tab_id
            {
                return Err(failure(
                    "stale_target",
                    "The query tab or connection of this conversation changed. Read the workspace and retry.",
                ));
            }
            if tab.busy || !tab.more {
                return Err(failure(
                    "result_unavailable",
                    "No more downloaded result batch is available.",
                ));
            }
            if self
                .thread_run(&call.thread_id)
                .is_some_and(|run| run.pending_query.is_some())
            {
                return Err(failure(
                    "tab_busy",
                    "Another assistant query request is pending.",
                ));
            }
            let worker = tab.worker.as_ref().ok_or_else(|| {
                failure(
                    "result_unavailable",
                    "The query worker is no longer available.",
                )
            })?;
            let (next_page, first_row) = {
                let data = tab.table.read(cx);
                let rows = data.delegate().rows.len();
                (data.delegate().pagination.pages(rows), rows)
            };
            worker.more();
            let tab = &mut self.tabs[index];
            tab.pending_page = Some(next_page);
            tab.busy = true;
            tab.cancelling = false;
            tab.started = Some(Instant::now());
            tab.status = "Fetching next batch…".into();
            let pending = PendingQuery {
                call: call.clone(),
                tab_id: tab.saved.id,
                revision: tab.revision,
                sql: tab.input.read(cx).value().to_string(),
                approved: true,
                started: true,
                kind: PendingQueryKind::Fetch,
                activity_index: None,
                detached: false,
                first_row,
            };
            self.thread_run_mut(&call.thread_id).pending_query = Some(pending);
            self.record_assistant_query_started(&call.thread_id, cx);
            Ok(())
        })();
        result.err()
    }

    fn tool_catalog(&mut self, call: &ToolCall, cx: &mut Context<Self>) -> Option<ToolResult> {
        match self.catalog_step(call, true, cx) {
            CatalogStep::Done(result) => Some(result),
            CatalogStep::Wait { refreshed } => {
                let connection = call
                    .arguments
                    .get("connection_id")
                    .and_then(Value::as_str)
                    .and_then(|id| Uuid::parse_str(id).ok())
                    .unwrap_or_default();
                let refreshed = refreshed.then(|| self.catalog.idle_reports(connection));
                self.thread_run_mut(&call.thread_id)
                    .catalog_calls
                    .push(PendingCatalogCall {
                        call: call.clone(),
                        deadline: Instant::now() + MAX_CATALOG_WAIT,
                        refreshed,
                        connection,
                    });
                None
            }
        }
    }

    /// Answer a catalog tool call from the cache. A call for data that the
    /// cache does not have waits while a refresh reads it. With
    /// `may_refresh`, a call starts that refresh when a tab of the
    /// connection is connected.
    fn catalog_step(
        &mut self,
        call: &ToolCall,
        may_refresh: bool,
        cx: &mut Context<Self>,
    ) -> CatalogStep {
        let request = match CatalogRequest::parse(call) {
            Ok(request) => request,
            Err(error) => return CatalogStep::Done(error),
        };
        let id = request.connection_id();
        let Some(profile) = self.profiles.iter().find(|profile| profile.id == id) else {
            return CatalogStep::Done(failure(
                "invalid_arguments",
                "The connection was not found. Use a connection ID from the workspace context.",
            ));
        };
        if !profile.catalog.browses() {
            return CatalogStep::Done(failure(
                "schema_browsing_off",
                "Schema browsing is off for this connection, so Qrow has no schema catalog for it. Ask the user to set Schema refresh in the connection settings, or run SHOW and DESCRIBE with run_selected_tab_query.",
            ));
        }
        let settings = crate::model::effective_catalog(profile, &self.shared_catalogs);
        self.ensure_catalog(id);
        // The worker loads the cache by itself, soon after it starts.
        let Some(catalog) = self.catalog.catalog(id) else {
            return CatalogStep::Wait { refreshed: false };
        };
        let now = crate::catalog::now();
        let result = match &request {
            CatalogRequest::Schemas(input) => catalog::schemas(
                catalog,
                &settings,
                now,
                input.offset,
                input.limit.unwrap_or(catalog::DEFAULT_PAGE),
            ),
            CatalogRequest::Relations(input) => catalog::relations(
                catalog,
                &settings,
                now,
                &input.schema,
                input.pattern.as_deref(),
                input.offset,
                input.limit.unwrap_or(catalog::DEFAULT_PAGE),
            ),
            CatalogRequest::Relation(input) => {
                catalog::relation(catalog, &settings, now, &input.schema, &input.relation)
            }
        };
        let missing = match result {
            Ok(mut value) => {
                value["version"] = json!(TOOL_SCHEMA_VERSION);
                value["connection_id"] = json!(id);
                return CatalogStep::Done(success(value));
            }
            Err(missing) => missing,
        };
        if let Some(scope) = missing.scope() {
            if self.catalog.reads(id, &scope) {
                return CatalogStep::Wait { refreshed: false };
            }
            // A refresh that ran and could not read the data left its error.
            let error = match &scope {
                crate::catalog::Scope::Schema(schema) => {
                    catalog.schema(schema).and_then(|node| node.error.clone())
                }
                crate::catalog::Scope::Relation(schema, relation) => catalog
                    .relation(schema, relation)
                    .and_then(|node| node.error.clone()),
                crate::catalog::Scope::Connection => catalog.error.clone(),
            };
            if let Some(error) = error.filter(|_| !may_refresh) {
                return CatalogStep::Done(failure(
                    "refresh_failed",
                    format!("Qrow could not read it from the database: {error}"),
                ));
            }
            if may_refresh && self.catalog_warm(id) {
                self.refresh_catalog(id, scope, cx);
                return CatalogStep::Wait { refreshed: true };
            }
        }
        let (code, message) = missing.error();
        CatalogStep::Done(failure(code, message))
    }

    /// Answer the catalog tool calls whose cache or refresh is ready, or whose
    /// wait ended. Calls of turns that ended go away. Returns whether calls
    /// still wait.
    pub(super) fn resume_catalog_calls(&mut self, cx: &mut Context<Self>) -> bool {
        let threads: Vec<String> = self
            .assistant_state
            .runs
            .iter()
            .filter(|(_, run)| !run.catalog_calls.is_empty())
            .map(|(thread, _)| thread.clone())
            .collect();
        let mut waiting = false;
        for thread in threads {
            let run = self.thread_run_mut(&thread);
            let active = run.active_turn.clone();
            let calls = std::mem::take(&mut run.catalog_calls);
            for mut pending in calls {
                if active.as_deref() != Some(pending.call.turn_id.as_str()) {
                    continue;
                }
                // The window can see the end of a refresh before it sees its
                // start, so a call waits until the worker reports idle again.
                let unfinished = pending
                    .refreshed
                    .is_some_and(|idle| self.catalog.idle_reports(pending.connection) == idle);
                // A call that waited only for the cache can still start a
                // refresh. A call whose refresh ended reports what it read.
                let step = if unfinished && Instant::now() < pending.deadline {
                    CatalogStep::Wait { refreshed: false }
                } else {
                    self.catalog_step(&pending.call, pending.refreshed.is_none(), cx)
                };
                let result = match step {
                    CatalogStep::Done(result) => result,
                    CatalogStep::Wait { refreshed } if Instant::now() < pending.deadline => {
                        if refreshed {
                            pending.refreshed = Some(self.catalog.idle_reports(pending.connection));
                        }
                        waiting = true;
                        self.thread_run_mut(&thread).catalog_calls.push(pending);
                        continue;
                    }
                    CatalogStep::Wait { .. } => failure(
                        "not_cached",
                        "Qrow is still reading the schema catalog. Try again later, or run SHOW or DESCRIBE with run_selected_tab_query.",
                    ),
                };
                self.finish_assistant_tool(pending.call, result, cx);
                cx.notify();
            }
        }
        waiting
    }

    fn tool_logs(&self, call: &ToolCall) -> Result<ToolResult, ToolResult> {
        let mut args: LogsInput = parse(call.arguments.clone())?;
        version(args.version)?;
        args.tab_id = self.tool_tab_id(call, args.tab_id);
        if self.tool_target(call).is_none() {
            return Err(failure("no_action_target", "Wait for a new user message."));
        }
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.saved.id == args.tab_id)
            .ok_or_else(|| failure("invalid_arguments", "The query tab was not found."))?;
        let text = match args.scope.as_str() {
            "latest_error" => tab.output.copy_error().unwrap_or_default(),
            "latest_execution" => tab
                .output
                .groups()
                .iter()
                .rev()
                .find(|group| group.execution_id.is_some())
                .map(|group| {
                    tab.output
                        .group_entries(group.id)
                        .map(|entry| entry.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
            _ => {
                return Err(failure(
                    "invalid_arguments",
                    "Choose latest_execution or latest_error.",
                ));
            }
        };
        let (text, truncated) = bound_text(&text);
        Ok(success(
            json!({"version": 1, "tab_id": args.tab_id, "scope": args.scope, "text": text, "truncated": truncated}),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{AppendedQuery, EditorDocument, RunInput, resolve_tab_id, run_request};
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn unknown_tab_id_uses_conversation_tab_but_existing_other_tab_remains_distinct() {
        let conversation = Uuid::new_v4();
        let other = Uuid::new_v4();
        let unknown = Uuid::new_v4();
        assert_eq!(
            resolve_tab_id(unknown, Some(conversation), [conversation, other]),
            conversation
        );
        assert_eq!(
            resolve_tab_id(other, Some(conversation), [conversation, other]),
            other
        );
        assert_eq!(resolve_tab_id(unknown, None, [conversation]), unknown);
    }

    #[test]
    fn run_without_revision_requires_the_unchanged_append_selection() {
        let tab_id = Uuid::new_v4();
        let connection_id = Uuid::new_v4();
        let sql = "SELECT 0;\n\nSELECT 1";
        let document = EditorDocument {
            tab_id,
            connection_id: Some(connection_id),
            revision: 8,
            sql,
            selected_range: Some(11..19),
            busy: false,
            sql_style: crate::sql::SqlStyle::default(),
        };
        let appended = AppendedQuery {
            turn_id: "turn".into(),
            tab_id,
            connection_id: Some(connection_id),
            revision: 8,
            selected_range: 11..19,
        };
        let input = || {
            serde_json::from_value::<RunInput>(json!({
                "version": 1,
                "tab_id": tab_id,
                "connection_id": connection_id,
            }))
            .unwrap()
        };
        let request = run_request(input(), Some(&appended), "turn", &document).unwrap();
        assert_eq!(request.editor_revision, 8);
        assert!(request.statement_range.is_none());
        assert!(run_request(input(), None, "turn", &document).is_err());
        assert!(run_request(input(), Some(&appended), "next-turn", &document).is_err());
        assert!(
            run_request(
                input(),
                Some(&appended),
                "turn",
                &EditorDocument {
                    revision: 9,
                    ..document.clone()
                }
            )
            .is_err()
        );
        assert!(
            run_request(
                input(),
                Some(&appended),
                "turn",
                &EditorDocument {
                    selected_range: Some(0..9),
                    ..document.clone()
                }
            )
            .is_err()
        );
        assert!(
            run_request(
                input(),
                Some(&appended),
                "turn",
                &EditorDocument {
                    tab_id: Uuid::new_v4(),
                    ..document.clone()
                }
            )
            .is_err()
        );

        let range_input = serde_json::from_value(json!({
            "version": 1,
            "tab_id": tab_id,
            "connection_id": connection_id,
            "statement_range": {"start": 0, "end": 8},
        }))
        .unwrap();
        assert!(run_request(range_input, Some(&appended), "turn", &document).is_err());
    }
}
