//! The complete client-defined tool surface advertised to Codex.

use super::ToolDefinition;
use serde_json::json;

pub fn definitions() -> Vec<ToolDefinition> {
    [
        ("get_workspace_context", "Read current connection and query-tab metadata. Each user message already includes this context, so do not call this tool to start a turn. Call it after the user switches or renames a tab or after a tool reports a stale target. It refreshes the action target to the currently selected tab for this turn. Use the exact selected_tab IDs and revision it returns.", json!({
            "type": "object", "properties": {"version": {"const": 1}},
            "required": ["version"], "additionalProperties": false
        })),
        ("read_tab_sql", "Read the current SQL, editor revision, and statement byte ranges of one query tab. Tool results return the new revision and selection after a change, so do not call this tool only to read them.", json!({
            "type": "object", "properties": {"version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"}},
            "required": ["version", "tab_id"], "additionalProperties": false
        })),
        ("append_selected_tab_sql", "Append one new SQL statement to the selected query tab, preserving existing queries. Use this when writing a new query. The appended statement becomes selected. The result returns the new editor_revision and the statement_range of the appended statement. To run it, call run_selected_tab_query with that revision and without statement_range.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "connection_id": {"type": ["string", "null"], "format": "uuid"},
                "editor_revision": {"type": "integer", "minimum": 0},
                "sql": {"type": "string"}},
            "required": ["version", "tab_id", "connection_id", "editor_revision", "sql"],
            "additionalProperties": false
        })),
        ("edit_selected_tab_sql", "Modify existing SQL in the selected query tab at the specified revision. Use this only when the user asks to change existing SQL. Use append_selected_tab_sql for a new query. Set replace_existing to true only when the user asks to replace all SQL.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "connection_id": {"type": ["string", "null"], "format": "uuid"},
                "editor_revision": {"type": "integer", "minimum": 0},
                "replace_existing": {"type": "boolean"},
                "edits": {"type": "array", "minItems": 1, "maxItems": 64,
                    "items": {"type": "object", "properties": {
                        "start": {"type": "integer", "minimum": 0}, "end": {"type": "integer", "minimum": 0},
                        "replacement": {"type": "string"}},
                        "required": ["start", "end", "replacement"], "additionalProperties": false}}
            }, "required": ["version", "tab_id", "connection_id", "editor_revision", "edits"],
            "additionalProperties": false
        })),
        ("run_selected_tab_query", "Run one SQL statement from the selected tab through Qrow, subject to user approval mode. Without statement_range, it runs the selected text, or the whole tab when nothing is selected. To run another statement in a tab with several statements, pass one of the statement_ranges from the workspace context or read_tab_sql as statement_range. The range must match the current editor revision. A finished query returns its first downloaded rows in rows and the offset for read_results in next_offset.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "connection_id": {"type": "string", "format": "uuid"},
                "editor_revision": {"type": "integer", "minimum": 0},
                "statement_range": {"type": "object", "properties": {
                    "start": {"type": "integer", "minimum": 0},
                    "end": {"type": "integer", "minimum": 0}},
                    "required": ["start", "end"], "additionalProperties": false}},
            "required": ["version", "tab_id", "connection_id", "editor_revision"], "additionalProperties": false
        })),
        ("cancel_selected_tab_query", "Request best-effort cancellation of the selected tab's running query.", target_schema()),
        ("get_query_status", "Read the current query status and bounded result metadata for a tab.", tab_schema()),
        ("read_results", "Read downloaded result rows only; does not fetch from the database. A finished query already returns its first rows, so use this only for rows after that.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "offset": {"type": "integer", "minimum": 0},
                "count": {"type": "integer", "minimum": 1, "maximum": 100}},
            "required": ["version", "tab_id", "offset", "count"], "additionalProperties": false
        })),
        ("fetch_more_results", "Fetch one more bounded preview batch from the selected tab's existing cursor. The result returns the first fetched rows.", target_schema()),
        ("read_query_logs", "Read bounded Logs from a tab's latest execution or latest error.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "scope": {"type": "string", "enum": ["latest_execution", "latest_error"]}},
            "required": ["version", "tab_id", "scope"], "additionalProperties": false
        })),
    ]
    .into_iter()
    .map(|(name, description, input_schema)| ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema,
    })
    .collect()
}

fn tab_schema() -> serde_json::Value {
    json!({"type": "object", "properties": {
        "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"}},
        "required": ["version", "tab_id"], "additionalProperties": false})
}

fn target_schema() -> serde_json::Value {
    json!({"type": "object", "properties": {
        "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
        "connection_id": {"type": "string", "format": "uuid"}},
        "required": ["version", "tab_id", "connection_id"], "additionalProperties": false})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn tool_surface_has_unique_names_and_closed_input_schemas() {
        let tools = definitions();
        let names: BTreeSet<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(tools.len(), 10);
        assert_eq!(names.len(), tools.len());
        assert!(
            tools
                .iter()
                .all(|tool| tool.input_schema["additionalProperties"] == false)
        );
    }
}
