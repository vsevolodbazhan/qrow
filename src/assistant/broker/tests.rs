use super::*;

fn ids() -> (Uuid, Uuid) {
    (Uuid::new_v4(), Uuid::new_v4())
}

fn target(tab: Uuid, connection: Uuid) -> ActionTarget {
    ActionTarget {
        conversation_id: "conversation".into(),
        turn_id: "turn".into(),
        tab_id: tab,
        connection_id: Some(connection),
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
        database_type: crate::model::DatabaseType::Kyuubi,
        tab_id: tab,
        connection_id: Some(connection),
        revision: 7,
        sql,
        selected_range: None,
        busy: false,
        sql_style: sql::SqlStyle::default(),
    }
}

fn edit_request(tab: Uuid, connection: Uuid, edits: Vec<TextEdit>) -> EditRequest {
    EditRequest {
        version: TOOL_SCHEMA_VERSION,
        tab_id: tab,
        connection_id: Some(connection),
        editor_revision: 7,
        edits,
        replace_existing: false,
    }
}

fn append_request(tab: Uuid, connection: Uuid, sql: &str) -> AppendRequest {
    AppendRequest {
        version: TOOL_SCHEMA_VERSION,
        tab_id: tab,
        connection_id: Some(connection),
        editor_revision: 7,
        sql: sql.into(),
    }
}

#[test]
fn appends_new_statement_and_selects_only_it() {
    let (tab, connection) = ids();
    let broker = ToolBroker::new(Some(target(tab, connection)));
    let first = broker
        .plan_append(
            call(),
            &append_request(tab, connection, " SELECT 2 "),
            &document(tab, connection, "SELECT '日本語' -- keep this comment"),
        )
        .unwrap();
    assert_eq!(
        first.sql,
        "SELECT '日本語'; -- keep this comment\n\nSELECT 2"
    );
    assert_eq!(&first.sql[first.appended_range], "SELECT 2");

    let second = broker
        .plan_append(
            call(),
            &append_request(tab, connection, "SELECT 3;"),
            &document(tab, connection, "SELECT 1;\n\nSELECT 2;\n"),
        )
        .unwrap();
    assert_eq!(second.sql, "SELECT 1;\n\nSELECT 2;\n\nSELECT 3;");
    assert_eq!(&second.sql[second.appended_range], "SELECT 3;");
}

#[test]
fn append_formats_long_one_line_statement() {
    let (tab, connection) = ids();
    let broker = ToolBroker::new(Some(target(tab, connection)));
    let long = "SELECT state, COUNT(*) AS bookings, MAX(booked_at) AS last_booked_at FROM integrations.bookings GROUP BY state;";
    let plan = broker
        .plan_append(
            call(),
            &append_request(tab, connection, long),
            &document(tab, connection, "SELECT 1"),
        )
        .unwrap();
    assert!(plan.formatted);
    assert_eq!(
        &plan.sql[plan.appended_range],
        "SELECT\n  state,\n  COUNT(*) AS bookings,\n  MAX(booked_at) AS last_booked_at\nFROM integrations.bookings\nGROUP BY state;"
    );

    let short = broker
        .plan_append(
            call(),
            &append_request(tab, connection, "SELECT 2"),
            &document(tab, connection, ""),
        )
        .unwrap();
    assert!(!short.formatted);
    assert_eq!(short.sql, "SELECT 2");
}

#[test]
fn append_keeps_leading_comment_outside_formatting_and_selection() {
    let (tab, connection) = ids();
    let broker = ToolBroker::new(Some(target(tab, connection)));
    // The comment does not count toward the length that starts formatting.
    let short = "-- Count paid bookings for the last seven days in every market\nSELECT COUNT(*) FROM bookings";
    let plan = broker
        .plan_append(
            call(),
            &append_request(tab, connection, short),
            &document(tab, connection, "SELECT 1"),
        )
        .unwrap();
    assert!(!plan.formatted);
    assert_eq!(plan.sql, format!("SELECT 1;\n\n{short}"));
    assert_eq!(
        &plan.sql[plan.appended_range],
        "SELECT COUNT(*) FROM bookings"
    );

    let long = "-- bookings by state\nselect state, count(*) as bookings, max(booked_at) as last_booked_at from integrations.bookings group by state;";
    let plan = broker
        .plan_append(
            call(),
            &append_request(tab, connection, long),
            &document(tab, connection, ""),
        )
        .unwrap();
    assert!(plan.formatted);
    assert_eq!(
        plan.sql,
        "-- bookings by state\nSELECT\n  state,\n  COUNT(*) AS bookings,\n  MAX(booked_at) AS last_booked_at\nFROM integrations.bookings\nGROUP BY state;"
    );
    assert!(plan.sql[plan.appended_range].starts_with("SELECT\n"));
}

#[test]
fn append_rejects_multiple_statements_and_stale_revision() {
    let (tab, connection) = ids();
    let broker = ToolBroker::new(Some(target(tab, connection)));
    let document = document(tab, connection, "SELECT 1");
    assert_eq!(
        broker
            .plan_append(
                call(),
                &append_request(tab, connection, "SELECT 2; SELECT 3"),
                &document,
            )
            .unwrap_err()
            .code,
        ToolErrorCode::InvalidStatement
    );
    let mut stale = append_request(tab, connection, "SELECT 2");
    stale.editor_revision = 6;
    assert_eq!(
        broker
            .plan_append(call(), &stale, &document)
            .unwrap_err()
            .code,
        ToolErrorCode::StaleRevision
    );
}

#[test]
fn appended_selection_can_run_without_running_older_queries() {
    let (tab, connection) = ids();
    let broker = ToolBroker::new(Some(target(tab, connection)));
    let append = broker
        .plan_append(
            call(),
            &append_request(tab, connection, "SELECT 2"),
            &document(tab, connection, "SELECT 1"),
        )
        .unwrap();
    let mut target = target(tab, connection);
    target.selected_range = Some(append.appended_range.clone());
    let document = EditorDocument {
        database_type: crate::model::DatabaseType::Kyuubi,
        tab_id: tab,
        connection_id: Some(connection),
        revision: 8,
        sql: &append.sql,
        selected_range: Some(append.appended_range),
        busy: false,
        sql_style: sql::SqlStyle::default(),
    };
    let run = ToolBroker::new(Some(target))
        .plan_run(
            call(),
            &RunRequest {
                version: TOOL_SCHEMA_VERSION,
                tab_id: tab,
                connection_id: connection,
                editor_revision: 8,
                statement_range: None,
            },
            &document,
        )
        .unwrap();
    assert_eq!(run.sql, "SELECT 2");
}

#[test]
fn edit_formats_only_statements_that_a_replacement_supplies() {
    let (tab, connection) = ids();
    let broker = ToolBroker::new(Some(target(tab, connection)));
    let long = "SELECT state, COUNT(*) AS bookings, MAX(booked_at) AS last_booked_at FROM integrations.bookings GROUP BY state";
    let formatted = "SELECT\n  state,\n  COUNT(*) AS bookings,\n  MAX(booked_at) AS last_booked_at\nFROM integrations.bookings\nGROUP BY state";
    let edit = |start, end, replacement: &str| TextEdit {
        start,
        end,
        replacement: replacement.into(),
    };

    // A rewrite of the second statement and a new statement at the end.
    let before = "SELECT 1;\n\nSELECT 2;";
    let plan = broker
        .plan_edit(
            call(),
            &edit_request(
                tab,
                connection,
                vec![
                    edit(before.len(), before.len(), &format!("\n\n{long};")),
                    // The separator stays from before the edit.
                    edit(11, 19, long),
                ],
            ),
            &document(tab, connection, before),
        )
        .unwrap();
    assert!(plan.formatted);
    assert_eq!(
        plan.sql,
        format!("SELECT 1;\n\n{formatted};\n\n{formatted};")
    );
    assert_eq!(apply_edits(before, &plan.edits), plan.sql);
    assert_eq!(plan.edits[0].replacement, formatted);

    // A replacement with several statements formats each long statement.
    let mut request = edit_request(
        tab,
        connection,
        vec![edit(0, 8, &format!("SELECT 0; {long}"))],
    );
    request.replace_existing = true;
    let plan = broker
        .plan_edit(call(), &request, &document(tab, connection, "SELECT 1"))
        .unwrap();
    assert_eq!(plan.sql, format!("SELECT 0; {formatted}"));

    // A partial change keeps the layout of a statement that the user wrote.
    let before = format!("{long} LIMIT 10");
    let at = before.len() - 2;
    let plan = broker
        .plan_edit(
            call(),
            &edit_request(tab, connection, vec![edit(at, before.len(), "20")]),
            &document(tab, connection, &before),
        )
        .unwrap();
    assert!(!plan.formatted);
    assert_eq!(plan.sql, format!("{long} LIMIT 20"));
}

#[test]
fn selected_tab_can_be_edited_without_a_connection_but_not_run() {
    let tab = Uuid::new_v4();
    let broker = ToolBroker::new(Some(ActionTarget {
        conversation_id: "conversation".into(),
        turn_id: "turn".into(),
        tab_id: tab,
        connection_id: None,
        selected_range: None,
    }));
    let document = EditorDocument {
        database_type: crate::model::DatabaseType::Kyuubi,
        tab_id: tab,
        connection_id: None,
        revision: 7,
        sql: "",
        selected_range: None,
        busy: false,
        sql_style: sql::SqlStyle::default(),
    };
    let edit = EditRequest {
        version: TOOL_SCHEMA_VERSION,
        tab_id: tab,
        connection_id: None,
        editor_revision: 7,
        edits: vec![TextEdit {
            start: 0,
            end: 0,
            replacement: "SELECT 1".into(),
        }],
        replace_existing: false,
    };
    assert_eq!(
        broker.plan_edit(call(), &edit, &document).unwrap().sql,
        "SELECT 1"
    );
    assert_eq!(
        broker
            .plan_run(
                call(),
                &RunRequest {
                    version: TOOL_SCHEMA_VERSION,
                    tab_id: tab,
                    connection_id: Uuid::new_v4(),
                    editor_revision: 7,
                    statement_range: None,
                },
                &document
            )
            .unwrap_err()
            .code,
        ToolErrorCode::StaleTarget
    );
}

#[test]
fn context_serialization_contains_only_allow_list_fields() {
    let (tab, connection) = ids();
    let context = WorkspaceContext::new(
        sql::SqlStyle::default(),
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
            sql_offset: 0,
            sql_bytes: 8,
            sql_truncated: false,
            selected_range: None,
            statement_ranges: sql::statement_ranges("SELECT 1"),
            statement_ranges_truncated: false,
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
fn whole_document_edit_requires_explicit_replacement() {
    let (tab, connection) = ids();
    let broker = ToolBroker::new(Some(target(tab, connection)));
    let mut request = edit_request(
        tab,
        connection,
        vec![TextEdit {
            start: 0,
            end: "SELECT 1".len(),
            replacement: "SELECT 2".into(),
        }],
    );
    let document = document(tab, connection, "SELECT 1");
    assert_eq!(
        broker
            .plan_edit(call(), &request, &document)
            .unwrap_err()
            .code,
        ToolErrorCode::InvalidArguments
    );
    request.replace_existing = true;
    assert_eq!(
        broker.plan_edit(call(), &request, &document).unwrap().sql,
        "SELECT 2"
    );
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
        statement_range: None,
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
fn run_can_target_a_statement_by_utf8_byte_range() {
    let (tab, connection) = ids();
    let broker = ToolBroker::new(Some(target(tab, connection)));
    let document = document(tab, connection, "SELECT '日本語';\n\nSELECT 2;");
    let start = document.sql.find("SELECT 2").unwrap();
    let mut request = RunRequest {
        version: TOOL_SCHEMA_VERSION,
        tab_id: tab,
        connection_id: connection,
        editor_revision: 7,
        statement_range: Some(start..document.sql.len()),
    };
    assert_eq!(
        broker.plan_run(call(), &request, &document).unwrap().sql,
        "SELECT 2;"
    );
    request.statement_range = Some(0..document.sql.len());
    assert_eq!(
        broker
            .plan_run(call(), &request, &document)
            .unwrap_err()
            .code,
        ToolErrorCode::InvalidStatement
    );
    request.statement_range = Some(9..document.sql.len());
    assert_eq!(
        broker
            .plan_run(call(), &request, &document)
            .unwrap_err()
            .code,
        ToolErrorCode::InvalidArguments
    );
}

#[test]
fn row_and_text_outputs_respect_byte_limits() {
    let rows = vec![vec![Some("x".repeat(MAX_TOOL_OUTPUT_BYTES))], vec![None]];
    let bounded = bound_rows_after(&rows, 0, 2, 0).unwrap();
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
        database_type: crate::model::DatabaseType::Kyuubi,
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
        bound_rows_after(&[], 0, 0, 0).unwrap_err().code,
        ToolErrorCode::InvalidArguments
    );
    assert_eq!(
        bound_rows_after(&[], 0, MAX_TOOL_ROWS + 1, 0)
            .unwrap_err()
            .code,
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
        statement_range: None,
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

    let first = bound_rows_after(&rows, 0, 2, 0).unwrap();
    assert_eq!(first.rows, rows[..2]);
    assert_eq!(first.next_offset, 2);
    assert!(!first.truncated);
    assert!(first.omitted_row_offsets.is_empty());

    let second = bound_rows_after(&rows, first.next_offset, 1, 0).unwrap();
    assert_eq!(second.rows, rows[2..]);
    assert_eq!(second.next_offset, 3);
    assert!(!second.truncated);
}

#[test]
fn result_preview_is_bounded_by_rows_bytes_and_remaining_output() {
    let rows: Vec<_> = (0..30).map(|row| vec![Some(row.to_string())]).collect();
    let preview = preview_rows(&rows, 0, 0);
    assert_eq!(preview.rows, rows[..MAX_PREVIEW_ROWS]);
    assert_eq!(preview.next_offset, MAX_PREVIEW_ROWS);

    let fetched = preview_rows(&rows, 25, 0);
    assert_eq!(fetched.rows, rows[25..]);
    assert_eq!(fetched.next_offset, 30);

    let wide: Vec<_> = (0..3)
        .map(|_| vec![Some("x".repeat(MAX_PREVIEW_BYTES / 2))])
        .collect();
    let preview = preview_rows(&wide, 0, 0);
    assert_eq!(preview.rows.len(), 1);
    assert_eq!(preview.next_offset, 1);
    assert!(preview.truncated);

    let full = preview_rows(&rows, 0, MAX_TOOL_PAYLOAD_BYTES);
    assert!(full.rows.is_empty());
    assert_eq!(full.next_offset, 0);
    assert!(full.omitted_row_offsets.is_empty());

    let large = vec![vec![Some("x".repeat(MAX_PREVIEW_BYTES + 1))]];
    let preview = preview_rows(&large, 0, 0);
    assert!(preview.rows.is_empty());
    assert_eq!(preview.next_offset, 0);
    assert!(preview.omitted_row_offsets.is_empty());
    let read = bound_rows_after(&large, 0, 1, 300).unwrap();
    assert_eq!(read.rows, large);
    assert_eq!(read.next_offset, 1);
}

#[test]
fn wide_rows_do_not_stall_result_paging() {
    let mut rows: Vec<_> = (0..170)
        .map(|number| vec![Some(number.to_string()), Some("value".into())])
        .collect();
    rows[25][1] = Some("x".repeat(MAX_TOOL_PAYLOAD_BYTES - 100));
    let mut offset = 20;
    let mut omitted = Vec::new();
    while offset < rows.len() {
        let page = bound_rows_after(&rows, offset, 100, 300).unwrap();
        assert!(page.next_offset > offset, "row paging stopped at {offset}");
        omitted.extend(page.omitted_row_offsets);
        offset = page.next_offset;
    }
    assert_eq!(offset, 170);
    assert_eq!(omitted, [25]);
}

#[test]
fn context_lists_a_bounded_number_of_statement_ranges() {
    let sql = "SELECT 1;\nSELECT 2";
    let (ranges, truncated) = context_statement_ranges(sql, &(0..sql.len()));
    assert_eq!(ranges, [0..9, 10..18]);
    assert!(!truncated);
    let many = "SELECT 1;".repeat(MAX_CONTEXT_STATEMENTS + 1);
    let (ranges, truncated) = context_statement_ranges(&many, &(0..many.len()));
    assert_eq!(ranges.len(), MAX_CONTEXT_STATEMENTS);
    assert!(truncated);
    // A part lists only the statements that overlap it.
    let sql = "SELECT 1;\nSELECT 2;\nSELECT 3";
    let (ranges, truncated) = context_statement_ranges(sql, &(12..14));
    assert_eq!(ranges, vec![Range { start: 10, end: 19 }]);
    assert!(truncated);
    let (ranges, truncated) = context_statement_ranges(sql, &(5..12));
    assert_eq!(ranges, [0..9, 10..19]);
    assert!(truncated);
    let (ranges, truncated) = context_statement_ranges(sql, &(9..10));
    assert!(ranges.is_empty());
    assert!(truncated);
}

#[test]
fn sql_page_reads_whole_characters_from_a_boundary() {
    let sql = "abécd";
    assert_eq!(sql_page(sql, 0, 3), Some(0..2));
    assert_eq!(sql_page(sql, 2, 3), Some(2..5));
    // A limit that is smaller than one character still reads it.
    assert_eq!(sql_page(sql, 2, 1), Some(2..4));
    assert_eq!(sql_page(sql, 4, 100), Some(4..6));
    assert_eq!(sql_page(sql, 6, 10), Some(6..6));
    assert_eq!(sql_page(sql, 3, 10), None);
    assert_eq!(sql_page(sql, 7, 10), None);
}

#[test]
fn sql_window_keeps_the_focus_within_the_limit_on_character_boundaries() {
    assert_eq!(sql_window("SELECT 1", &(3..3), 64), 0..8);
    let sql = "é".repeat(100);
    // The window centers on the caret and does not split a character.
    let window = sql_window(&sql, &(100..100), 21);
    assert_eq!(window, 90..110);
    assert!(sql.is_char_boundary(window.start) && sql.is_char_boundary(window.end));
    // Near an end, the window uses the whole limit.
    assert_eq!(sql_window(&sql, &(0..0), 20), 0..20);
    assert_eq!(sql_window(&sql, &(200..200), 20), 180..200);
    // A selection longer than the limit shows its start.
    assert_eq!(sql_window(&sql, &(40..160), 20), 40..60);
    // A stale focus past the end shows the end.
    assert_eq!(sql_window(&sql, &(500..600), 20), 180..200);
}
