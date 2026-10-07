//! The complete client-defined tool surface advertised to Codex.

use super::{ToolDefinition, catalog};
use serde_json::json;

/// The names of the tools before they had a group prefix. Codex keeps the
/// tools of a conversation from its start, so older conversations call
/// these names.
const LEGACY_NAMES: [(&str, &str); 13] = [
    ("get_workspace_context", "workspace-read-context"),
    ("read_tab_sql", "tab-read-sql"),
    ("append_selected_tab_sql", "tab-append-sql"),
    ("edit_selected_tab_sql", "tab-edit-sql"),
    ("run_selected_tab_query", "query-run"),
    ("cancel_selected_tab_query", "query-cancel"),
    ("get_query_status", "query-read-status"),
    ("read_results", "query-read-results"),
    ("fetch_more_results", "query-fetch-results"),
    ("read_query_logs", "query-read-logs"),
    ("list_schemas", "catalog-list-schemas"),
    ("list_relations", "catalog-list-relations"),
    ("describe_relation", "catalog-describe-relation"),
];

/// The current name of a tool that an older conversation calls by its
/// earlier name. Other names stay.
pub fn canonical_name(name: &str) -> &str {
    LEGACY_NAMES
        .iter()
        .find(|(legacy, _)| *legacy == name)
        .map_or(name, |(_, current)| current)
}

pub fn definitions() -> Vec<ToolDefinition> {
    [
        ("workspace-read-context", "Read current connection and query-tab metadata. Each user message already includes this context, so do not call this tool to start a turn. Call it after the user renames a tab or after a tool reports a stale target. It refreshes the action target for this turn. selected_tab is the query tab of this conversation. Use the exact selected_tab IDs and revision it returns.", json!({
            "type": "object", "properties": {"version": {"const": 1}},
            "required": ["version"], "additionalProperties": false
        })),
        ("tab-read-sql", "Read the current SQL, editor revision, and statement byte ranges of one query tab. It returns at most 32768 bytes of SQL from offset, with sql_offset, the total sql_bytes, and next_offset for the next part, or null at the end. statement_ranges lists the statements in the returned part, with byte offsets in the complete SQL. Tool results return the new revision and selection after a change, so do not call this tool only to read them.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "offset": {"type": "integer", "minimum": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": 32768}},
            "required": ["version", "tab_id"], "additionalProperties": false
        })),
        ("tab-append-sql", "Append one new SQL statement to the query tab of this conversation (selected_tab in the workspace context), preserving existing queries. Use this when writing a new query. Start the SQL with one -- comment line of a few words that describes the query. The appended statement becomes selected, without its comment. The result returns the new editor_revision and the statement_range of the appended statement. To run it, call query-run with that revision and without statement_range. Write the SQL in sql_style from the workspace context: keywords, built-in function names, and type names in keyword_case, one clause per line, and indent_spaces spaces for each indent level. Qrow formats a statement longer than 80 characters in this style; formatted is then true, and statement_range refers to the formatted text.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "connection_id": {"type": ["string", "null"], "format": "uuid"},
                "editor_revision": {"type": "integer", "minimum": 0},
                "sql": {"type": "string"}},
            "required": ["version", "tab_id", "connection_id", "editor_revision", "sql"],
            "additionalProperties": false
        })),
        ("tab-edit-sql", "Modify existing SQL in the query tab of this conversation at the specified revision. Use this only when the user asks to change existing SQL. Use tab-append-sql for a new query. Set replace_existing to true only when the user asks to replace all SQL. Write replacement SQL in sql_style from the workspace context. Qrow formats a statement longer than 80 characters that a replacement supplies completely; formatted is then true, and later edits must use the returned editor_revision and the new SQL.", json!({
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
        ("query-run", "Run one SQL statement from the query tab of this conversation through Qrow, subject to user approval mode. Without statement_range, it runs the selected text, or the whole tab when nothing is selected. You may omit editor_revision only when running the query just appended in this turn; Qrow then uses that append's revision. For other runs, provide the current editor_revision. To run another statement in a tab with several statements, pass one of the statement_ranges from the workspace context or tab-read-sql as statement_range. The range must match the current editor revision. A finished query returns its first downloaded rows in rows and the offset for query-read-results in next_offset. Read again only while more_downloaded_rows is true.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "connection_id": {"type": "string", "format": "uuid"},
                "editor_revision": {"type": "integer", "minimum": 0},
                "statement_range": {"type": "object", "properties": {
                    "start": {"type": "integer", "minimum": 0},
                    "end": {"type": "integer", "minimum": 0}},
                    "required": ["start", "end"], "additionalProperties": false}},
            "required": ["version", "tab_id", "connection_id"], "additionalProperties": false
        })),
        ("query-cancel", "Request best-effort cancellation of the running query in the query tab of this conversation.", target_schema()),
        ("query-read-status", "Read the current query status and bounded result metadata for a tab.", tab_schema()),
        ("query-read-results", "Read downloaded result rows only; does not fetch from the database. A finished query already returns its first rows. Use next_offset for the next read. Stop when more_downloaded_rows is false. omitted_row_offsets lists rows too large for this tool; do not retry them.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "offset": {"type": "integer", "minimum": 0},
                "count": {"type": "integer", "minimum": 1, "maximum": 100}},
            "required": ["version", "tab_id", "offset", "count"], "additionalProperties": false
        })),
        ("query-fetch-results", "Fetch one more bounded preview batch from the existing cursor of the query tab of this conversation. The result returns the first fetched rows.", target_schema()),
        ("query-read-logs", "Read bounded Logs from a tab's latest execution or latest error.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "tab_id": {"type": "string", "format": "uuid"},
                "scope": {"type": "string", "enum": ["latest_execution", "latest_error"]}},
            "required": ["version", "tab_id", "scope"], "additionalProperties": false
        })),
        ("catalog-list-schemas", "List the schemas of a connection from the schema catalog that Qrow keeps on this computer. It does not query the database. Each schema has its relation_count, or null when Qrow has not read its relations. fetched_at is a Unix time in seconds, and stale is true when the data is older than the refresh period of the connection. Read the next page from next_offset while it is not null.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "connection_id": {"type": "string", "format": "uuid"},
                "offset": {"type": "integer", "minimum": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": catalog::MAX_PAGE}},
            "required": ["version", "connection_id"], "additionalProperties": false
        })),
        ("catalog-list-relations", "List the tables and views of one schema from the schema catalog of a connection. pattern filters the names: * matches any text and ? matches one character, without letter case. columns_loaded tells whether catalog-describe-relation can return the columns from the catalog. When Qrow has not read the relations and a tab of the connection is connected, Qrow reads them first. Read the next page from next_offset while it is not null.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "connection_id": {"type": "string", "format": "uuid"},
                "schema": {"type": "string"},
                "pattern": {"type": "string"},
                "offset": {"type": "integer", "minimum": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": catalog::MAX_PAGE}},
            "required": ["version", "connection_id", "schema"], "additionalProperties": false
        })),
        ("catalog-describe-relation", "Read the kind, comment, and columns with their types and comments of one table or view from the schema catalog of a connection. When Qrow has not read the columns and a tab of the connection is connected, Qrow reads them first. Otherwise the result is not_cached; then ask the user to refresh it, or run DESCRIBE with query-run. When a dbt model, seed, snapshot, or source builds the table, dbt_model gives its unique ID for dbt-describe-model.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "connection_id": {"type": "string", "format": "uuid"},
                "schema": {"type": "string"}, "relation": {"type": "string"}},
            "required": ["version", "connection_id", "schema", "relation"], "additionalProperties": false
        })),
        ("dbt-search-models", "Find the models, seeds, snapshots, and sources of the dbt project of a connection, from its manifest on this computer. It does not query the database and needs no schema catalog. patterns are globs that match the name, the table like core.orders, or the unique ID: * matches any text and ? one character, without regard to letter case. text matches words in the description. Each result gives the unique ID, the table, and the first line of the description. model_count is the total; use next_offset for the next page.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "connection_id": {"type": "string", "format": "uuid"},
                "patterns": {"type": "array", "items": {"type": "string"}, "maxItems": 20},
                "text": {"type": "string"}, "tag": {"type": "string"},
                "resource_type": {"enum": ["model", "seed", "snapshot", "source"]},
                "offset": {"type": "integer", "minimum": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": crate::assistant::dbt::MAX_PAGE}},
            "required": ["version", "connection_id"], "additionalProperties": false
        })),
        ("dbt-describe-model", "Read the meaning of one dbt model, seed, snapshot, or source: its description, materialization, tags, the tests of the model, and its direct parents and children with their counts. model is a unique ID, a table like core.orders, or a name. Without columns, it lists only the names of the columns that the project documents. For column descriptions, data types, and tests, pass glob patterns in columns, for example [\"*_id\", \"amount\"] or [\"*\"]; matched_column_count is the total, and next_column_offset gives the next page as column_offset. Use unique and not_null tests for keys, and relationships tests for join keys.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "connection_id": {"type": "string", "format": "uuid"},
                "model": {"type": "string"},
                "columns": {"type": "array", "items": {"type": "string"}, "maxItems": 50},
                "column_offset": {"type": "integer", "minimum": 0}},
            "required": ["version", "connection_id", "model"], "additionalProperties": false
        })),
        ("dbt-read-sql", "Read the SQL of a dbt model, seed, or snapshot from the manifest. model is a unique ID, a table like core.orders, or a name. code is compiled (the default), with refs resolved to tables, or raw, the Jinja SQL of the project. When the manifest has no compiled SQL, the result has the raw SQL and compiled_missing is true; the manifest then comes from dbt parse. It returns at most 32768 bytes from offset, with sql_offset, the total sql_bytes, and next_offset for the next part, or null at the end.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "connection_id": {"type": "string", "format": "uuid"},
                "model": {"type": "string"},
                "code": {"enum": ["compiled", "raw"]},
                "offset": {"type": "integer", "minimum": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": 32768}},
            "required": ["version", "connection_id", "model"], "additionalProperties": false
        })),
        ("dbt-read-lineage", "Read the resources upstream (parents) or downstream (children) of a dbt resource, to depth steps, from the dbt manifest. model is a unique ID, a table like core.orders, or a name. Each resource gives its unique ID, table, direction, and depth, and with descriptions the first line of its description. resource_count is the total; use next_offset for the next page.", json!({
            "type": "object", "properties": {
                "version": {"const": 1}, "connection_id": {"type": "string", "format": "uuid"},
                "model": {"type": "string"},
                "direction": {"enum": ["upstream", "downstream", "both"]},
                "depth": {"type": "integer", "minimum": 1, "maximum": crate::assistant::dbt::MAX_DEPTH},
                "descriptions": {"type": "boolean"},
                "offset": {"type": "integer", "minimum": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": crate::assistant::dbt::MAX_PAGE}},
            "required": ["version", "connection_id", "model"], "additionalProperties": false
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
    fn tool_names_have_a_group_and_earlier_names_still_work() {
        let groups = ["workspace-", "tab-", "query-", "catalog-", "dbt-"];
        let names: BTreeSet<String> = definitions().into_iter().map(|tool| tool.name).collect();
        for name in &names {
            assert!(groups.iter().any(|group| name.starts_with(group)), "{name}");
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{name}"
            );
        }
        for (legacy, current) in LEGACY_NAMES {
            assert_eq!(canonical_name(legacy), current);
            assert!(names.contains(current), "{current}");
        }
        assert_eq!(canonical_name("tab-read-sql"), "tab-read-sql");
        assert_eq!(canonical_name("unknown"), "unknown");
    }

    #[test]
    fn tool_surface_has_unique_names_and_closed_input_schemas() {
        let tools = definitions();
        let names: BTreeSet<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(tools.len(), 17);
        assert_eq!(names.len(), tools.len());
        assert!(
            tools
                .iter()
                .all(|tool| tool.input_schema["additionalProperties"] == false)
        );
    }

    #[test]
    fn read_tab_sql_limit_matches_the_page_limit() {
        let read = definitions()
            .into_iter()
            .find(|tool| tool.name == "tab-read-sql")
            .unwrap();
        assert_eq!(
            read.input_schema["properties"]["limit"]["maximum"],
            crate::assistant::broker::MAX_SQL_PAGE_BYTES
        );
    }

    #[test]
    fn run_schema_allows_the_revision_from_an_append_to_be_implicit() {
        let run = definitions()
            .into_iter()
            .find(|tool| tool.name == "query-run")
            .unwrap();
        let required = run.input_schema["required"].as_array().unwrap();
        assert!(!required.iter().any(|field| field == "editor_revision"));
        assert_eq!(
            run.input_schema["properties"]["editor_revision"]["type"],
            "integer"
        );
    }
}
