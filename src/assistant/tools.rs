//! The complete client-defined tool surface advertised to Codex.

use super::ToolDefinition;
use serde_json::json;

pub fn definitions() -> Vec<ToolDefinition> {
    [
        ("get_workspace_context", "Read allowed connection and query-tab metadata.", json!({
            "type": "object", "properties": {"version": {"const": 1}},
            "required": ["version"], "additionalProperties": false
        })),
        ("read_tab_sql", "Read the current SQL and editor revision of one query tab.", json!({
            "type": "object", "properties": {"version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"}},
            "required": ["version", "tab_id"], "additionalProperties": false
        })),
        ("append_selected_tab_sql", "Append one new SQL statement to the selected query tab, preserving existing queries. Use this when writing a new query. The appended statement becomes selected so it can run alone.", json!({
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
        ("run_selected_tab_query", "Run one SQL statement from the selected tab through Qrow, subject to user approval mode.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "connection_id": {"type": "string", "format": "uuid"},
                "editor_revision": {"type": "integer", "minimum": 0}},
            "required": ["version", "tab_id", "connection_id", "editor_revision"], "additionalProperties": false
        })),
        ("cancel_selected_tab_query", "Request best-effort cancellation of the selected tab's running query.", target_schema()),
        ("get_query_status", "Read the current query status and bounded result metadata for a tab.", tab_schema()),
        ("read_results", "Read downloaded result rows only; does not fetch from the database.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "offset": {"type": "integer", "minimum": 0},
                "count": {"type": "integer", "minimum": 1, "maximum": 100}},
            "required": ["version", "tab_id", "offset", "count"], "additionalProperties": false
        })),
        ("fetch_more_results", "Fetch one more bounded preview batch from the selected tab's existing cursor.", target_schema()),
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
