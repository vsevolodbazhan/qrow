use super::assistant_view::{
    PendingQuery, PendingQueryKind, ToolActivity, ToolKind, ToolState, TranscriptEntry,
};
use super::*;
use qrow::assistant::{
    ToolCall, ToolResult,
    broker::{
        AppendRequest, CallIdentity, EditRequest, EditorDocument, MAX_SQL_BYTES,
        MAX_TOOL_OUTPUT_BYTES, RunRequest, TOOL_SCHEMA_VERSION, ToolBroker, bound_rows, bound_text,
        context_statement_ranges, preview_rows,
    },
    service::Command as AssistantCommand,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::ops::Range;

fn remap_selection(
    selection: Range<usize>,
    edits: &[qrow::assistant::broker::TextEdit],
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

fn resolve_selected_tab_id(
    requested: Uuid,
    selected: Option<Uuid>,
    known: impl IntoIterator<Item = Uuid>,
) -> Uuid {
    // Keep a real other-tab ID so selected-tab actions still reject it.
    if known.into_iter().any(|id| id == requested) {
        requested
    } else {
        selected.unwrap_or(requested)
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
struct TargetInput {
    version: u32,
    tab_id: Uuid,
    connection_id: Uuid,
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

impl Qrow {
    fn selected_tab_id_for_tool(&self, requested: Uuid) -> Uuid {
        resolve_selected_tab_id(
            requested,
            self.tabs.get(self.active).map(|tab| tab.saved.id),
            self.tabs.iter().map(|tab| tab.saved.id),
        )
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
        let selected = self.assistant.selected_thread.as_deref();
        if selected != Some(call.thread_id.as_str())
            || self.assistant_panel.active_turn.as_deref() != Some(call.turn_id.as_str())
        {
            self.answer_assistant_call(
                call,
                false,
                failure("stale_target", "The conversation or turn changed.").content,
                cx,
            );
            return;
        }
        let name = call.name.clone();
        let result = self.dispatch_assistant_tool(&call, window, cx);
        if let Some(result) = result {
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
            self.assistant_panel
                .transcripts
                .entry(call.thread_id.clone())
                .or_default()
                .push(TranscriptEntry::tool(tool, call.turn_id.clone()).with_detail(detail));
            self.answer_assistant_call(call, result.success, result.content, cx);
        }
        cx.notify();
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
        let context = self.assistant_context(cx);
        // This read makes a tab change visible and binds later actions to that tab.
        self.assistant_panel.target =
            self.assistant_target(&call.thread_id, cx)
                .map(|mut target| {
                    target.turn_id = call.turn_id.clone();
                    target
                });
        Ok(success(context))
    }

    fn tool_read_sql(&self, call: &ToolCall, cx: &App) -> Result<ToolResult, ToolResult> {
        let mut args: TabInput = parse(call.arguments.clone())?;
        version(args.version)?;
        args.tab_id = self.selected_tab_id_for_tool(args.tab_id);
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
        let (statement_ranges, statement_ranges_truncated) = context_statement_ranges(&sql);
        Ok(success(
            json!({"version": 1, "tab_id": args.tab_id, "connection_id": tab.saved.profile,
            "editor_revision": tab.revision, "sql": sql, "statement_ranges": statement_ranges,
            "statement_ranges_truncated": statement_ranges_truncated}),
        ))
    }

    fn tool_edit(
        &mut self,
        call: &ToolCall,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<ToolResult, ToolResult> {
        let mut args: EditRequest = parse(call.arguments.clone())?;
        args.tab_id = self.selected_tab_id_for_tool(args.tab_id);
        let tab = &self.tabs[self.active];
        let sql = tab.input.read(cx).value().to_string();
        let selected = tab.input.read(cx).selected_range();
        let document = EditorDocument {
            tab_id: tab.saved.id,
            connection_id: tab.saved.profile,
            revision: tab.revision,
            sql: &sql,
            selected_range: (!selected.is_empty()).then_some(selected.clone()),
            busy: tab.busy,
        };
        let plan = ToolBroker::new(self.assistant_panel.target.clone())
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
        let appended_range = args
            .edits
            .iter()
            .any(|edit| {
                edit.start == sql.len()
                    && edit.end == sql.len()
                    && !edit.replacement.trim().is_empty()
            })
            .then(|| qrow::sql::last_statement_range(&plan.sql))
            .flatten()
            .filter(|range| range.start >= sql.len());
        let mapped = appended_range
            .clone()
            .or_else(|| remap_selection(selected, &args.edits));
        self.tabs[self.active].revision = self.tabs[self.active].revision.saturating_add(1);
        self.tabs[self.active].pending_assistant_edit = Some(plan.sql.clone());
        editor.update(cx, |editor, cx| {
            let scroll = editor.scroll_offset();
            editor.replace_all(plan.sql.clone(), window, cx);
            if let Some(range) = mapped {
                editor.set_selected_range(range, cx);
            }
            editor.set_scroll_offset(scroll, cx);
        });
        if let Some(range) = appended_range
            && let Some(target) = &mut self.assistant_panel.target
        {
            target.selected_range = Some(range);
        }
        let revision = self.tabs[self.active].revision;
        let selected = editor.read(cx).selected_range();
        self.changed(cx);
        // A run without statement_range uses this selection, so the model does not read it back.
        Ok(success(
            json!({"version": 1, "tab_id": plan.tab_id, "editor_revision": revision,
                "sql_bytes": plan.sql.len(),
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
        args.tab_id = self.selected_tab_id_for_tool(args.tab_id);
        let tab = &self.tabs[self.active];
        let sql = tab.input.read(cx).value().to_string();
        let document = EditorDocument {
            tab_id: tab.saved.id,
            connection_id: tab.saved.profile,
            revision: tab.revision,
            sql: &sql,
            selected_range: None,
            busy: tab.busy,
        };
        let plan = ToolBroker::new(self.assistant_panel.target.clone())
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
        self.tabs[self.active].revision = self.tabs[self.active].revision.saturating_add(1);
        self.tabs[self.active].pending_assistant_edit = Some(plan.sql.clone());
        editor.update(cx, |editor, cx| {
            editor.replace_all(plan.sql.clone(), window, cx);
            editor.set_selected_range(plan.appended_range.clone(), cx);
        });
        if let Some(target) = &mut self.assistant_panel.target {
            target.selected_range = Some(plan.appended_range.clone());
        }
        let revision = self.tabs[self.active].revision;
        self.changed(cx);
        Ok(success(
            json!({"version": 1, "tab_id": plan.tab_id, "editor_revision": revision,
                "sql_bytes": plan.sql.len(), "statement_range": plan.appended_range}),
        ))
    }

    fn tool_run(
        &mut self,
        call: &ToolCall,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ToolResult> {
        let result = (|| {
            let mut args: RunRequest = parse(call.arguments.clone())?;
            args.tab_id = self.selected_tab_id_for_tool(args.tab_id);
            let tab = &self.tabs[self.active];
            let sql = tab.input.read(cx).value().to_string();
            let selected = tab.input.read(cx).selected_range();
            let document = EditorDocument {
                tab_id: tab.saved.id,
                connection_id: tab.saved.profile,
                revision: tab.revision,
                sql: &sql,
                selected_range: (!selected.is_empty()).then_some(selected),
                busy: tab.busy,
            };
            let plan = ToolBroker::new(self.assistant_panel.target.clone())
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
            if self.assistant_panel.pending_query.is_some() {
                return Err(failure(
                    "tab_busy",
                    "Another assistant query request is pending.",
                ));
            }
            if let Some(range) = args.statement_range {
                tab.input.update(cx, |editor, cx| {
                    editor.set_selected_range(range.clone(), cx);
                });
                if let Some(target) = &mut self.assistant_panel.target {
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
            self.assistant_panel.pending_query = Some(PendingQuery {
                call: call.clone(),
                tab_id: plan.tab_id,
                revision: plan.expected_revision,
                sql: plan.sql,
                approved: mode == qrow::model::AssistantExecutionMode::RunAutomatically,
                started: false,
                kind: PendingQueryKind::Run,
                activity_index: None,
                detached: false,
                first_row: 0,
            });
            if mode == qrow::model::AssistantExecutionMode::RunAutomatically {
                self.begin_assistant_query(window, cx);
            }
            Ok(())
        })();
        result.err()
    }

    pub(super) fn approve_assistant_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pending) = &mut self.assistant_panel.pending_query {
            pending.approved = true;
        }
        self.begin_assistant_query(window, cx);
    }

    pub(super) fn cancel_assistant_approval(&mut self, cx: &mut Context<Self>) {
        if let Some(pending) = self.assistant_panel.pending_query.take() {
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

    fn begin_assistant_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = &self.assistant_panel.pending_query else {
            return;
        };
        if !pending.approved || pending.started {
            return;
        }
        let valid = self.tabs.get(self.active).is_some_and(|tab| {
            if tab.saved.id != pending.tab_id || tab.revision != pending.revision || tab.busy {
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
        if !valid {
            let pending = self.assistant_panel.pending_query.take().unwrap();
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
        }
        let kind = pending.kind;
        if let Some(pending) = &mut self.assistant_panel.pending_query {
            pending.started = true;
        }
        let started = match kind {
            PendingQueryKind::Run => self.run_selected_query(window, cx),
            PendingQueryKind::Fetch => {
                self.next_page(cx);
                self.tabs[self.active].busy
            }
        };
        if !started {
            let pending = self.assistant_panel.pending_query.take().unwrap();
            self.answer_assistant_call(
                pending.call,
                false,
                failure(
                    "query_not_started",
                    "Qrow could not start this query. Check the selected tab and connection.",
                )
                .content,
                cx,
            );
            return;
        }
        self.record_assistant_query_started(cx);
        self.complete_assistant_query(cx);
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
        self.assistant_panel
            .transcripts
            .entry(pending.call.thread_id.clone())
            .or_default()
            .push(entry);
    }

    fn record_assistant_query_started(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.assistant_panel.pending_query.as_ref() else {
            return;
        };
        let thread = pending.call.thread_id.clone();
        let turn = pending.call.turn_id.clone();
        let tool = self.assistant_query_tool(pending, ToolState::Running("Preparing".into()));
        let detail = bound_text(&format!("Query:\n{}", pending.sql)).0;
        let entries = self.assistant_panel.transcripts.entry(thread).or_default();
        let index = entries.len();
        entries.push(TranscriptEntry::tool(tool, turn).with_detail(detail));
        if let Some(pending) = &mut self.assistant_panel.pending_query {
            pending.activity_index = Some(index);
        }
        cx.notify();
    }

    pub(super) fn update_assistant_query_progress(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.assistant_panel.pending_query.as_ref() else {
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
            .assistant_panel
            .transcripts
            .get_mut(&pending.call.thread_id)
            .and_then(|entries| entries.get_mut(index))
            && entry.tool.as_ref().is_some_and(|tool| tool.state != state)
        {
            entry.set_tool_state(state);
            cx.notify();
        }
    }

    pub(super) fn complete_assistant_query(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = &self.assistant_panel.pending_query else {
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
        let pending = self.assistant_panel.pending_query.take().unwrap();
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
            .assistant_panel
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
        args.tab_id = self.selected_tab_id_for_tool(args.tab_id);
        let target =
            self.assistant_panel.target.as_ref().ok_or_else(|| {
                failure("no_action_target", "Select a tab and send a new message.")
            })?;
        if target.conversation_id != call.thread_id
            || target.turn_id != call.turn_id
            || target.tab_id != args.tab_id
            || target.connection_id != Some(args.connection_id)
            || self.tabs[self.active].saved.id != args.tab_id
        {
            return Err(failure(
                "stale_target",
                "The selected tab or connection changed.",
            ));
        }
        let was_running = self.tabs[self.active].busy;
        if was_running {
            self.cancel(cx);
        }
        Ok(success(
            json!({"version": 1, "tab_id": args.tab_id, "cancellation_requested": was_running}),
        ))
    }

    fn tool_status(&self, call: &ToolCall, cx: &App) -> Result<ToolResult, ToolResult> {
        let mut args: TabInput = parse(call.arguments.clone())?;
        version(args.version)?;
        args.tab_id = self.selected_tab_id_for_tool(args.tab_id);
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
        args.tab_id = self.selected_tab_id_for_tool(args.tab_id);
        if self.assistant_panel.target.is_none() {
            return Err(failure(
                "no_action_target",
                "Select a tab and send a new message.",
            ));
        }
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.saved.id == args.tab_id)
            .ok_or_else(|| failure("invalid_arguments", "The query tab was not found."))?;
        let data = tab.table.read(cx);
        let data = data.delegate();
        let bounded =
            bound_rows(&data.rows, args.offset, args.count).map_err(|error| ToolResult {
                success: false,
                content: json!({"version": 1, "error": error}),
            })?;
        Ok(success(
            json!({"version": 1, "tab_id": args.tab_id, "columns": data.columns.iter().map(|column| column.name.as_str()).collect::<Vec<_>>(),
            "downloaded_rows": data.rows.len(), "rows": bounded.rows, "next_offset": bounded.next_offset,
            "truncated": bounded.truncated, "omitted_row_offsets": bounded.omitted_row_offsets}),
        ))
    }

    fn tool_fetch(&mut self, call: &ToolCall, cx: &mut Context<Self>) -> Option<ToolResult> {
        let result = (|| {
            let mut args: TargetInput = parse(call.arguments.clone())?;
            version(args.version)?;
            args.tab_id = self.selected_tab_id_for_tool(args.tab_id);
            let target = self.assistant_panel.target.as_ref().ok_or_else(|| {
                failure("no_action_target", "Select a tab and send a new message.")
            })?;
            let tab = &self.tabs[self.active];
            if target.conversation_id != call.thread_id
                || target.turn_id != call.turn_id
                || target.tab_id != args.tab_id
                || target.connection_id != Some(args.connection_id)
                || tab.saved.id != args.tab_id
            {
                return Err(failure(
                    "stale_target",
                    "The selected tab or connection changed.",
                ));
            }
            if tab.busy || !tab.more {
                return Err(failure(
                    "result_unavailable",
                    "No more downloaded result batch is available.",
                ));
            }
            if self.assistant_panel.pending_query.is_some() {
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
            let tab = &mut self.tabs[self.active];
            tab.pending_page = Some(next_page);
            tab.busy = true;
            tab.cancelling = false;
            tab.started = Some(Instant::now());
            tab.status = "Fetching next batch…".into();
            self.assistant_panel.pending_query = Some(PendingQuery {
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
            });
            self.record_assistant_query_started(cx);
            Ok(())
        })();
        result.err()
    }

    fn tool_logs(&self, call: &ToolCall) -> Result<ToolResult, ToolResult> {
        let mut args: LogsInput = parse(call.arguments.clone())?;
        version(args.version)?;
        args.tab_id = self.selected_tab_id_for_tool(args.tab_id);
        if self.assistant_panel.target.is_none() {
            return Err(failure(
                "no_action_target",
                "Select a tab and send a new message.",
            ));
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
                    group
                        .entries
                        .iter()
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
    use super::resolve_selected_tab_id;
    use uuid::Uuid;

    #[test]
    fn unknown_tab_id_uses_selected_tab_but_existing_other_tab_remains_distinct() {
        let selected = Uuid::new_v4();
        let other = Uuid::new_v4();
        let unknown = Uuid::new_v4();
        assert_eq!(
            resolve_selected_tab_id(unknown, Some(selected), [selected, other]),
            selected
        );
        assert_eq!(
            resolve_selected_tab_id(other, Some(selected), [selected, other]),
            other
        );
        assert_eq!(resolve_selected_tab_id(unknown, None, [selected]), unknown);
    }
}
