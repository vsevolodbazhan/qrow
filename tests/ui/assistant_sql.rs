//! The assistant edits, formats, and requests SQL in its tab.
use crate::support::assistant::{FakeCodex, approval};
use crate::support::{
    MemoryCredentials, TestApp, assert_tab_dot, assert_tooltip_header_center, bounds_of, label,
    offline_profile, present,
};
use gpui_kit::{ElementId, TestAppContext};
use qrow::model::{AssistantTitleSource::Codex, SavedTab, Workspace};
use qrow::sql::KeywordCase;
use qrow::ui::OpenSettings;

fn launch(
    cx: &mut TestAppContext,
    sql: &str,
    configure: impl FnOnce(&mut Workspace),
) -> (TestApp, FakeCodex) {
    let (directory, codex) = FakeCodex::new();
    let profile = offline_profile("Synthetic");
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = sql.into();
    let mut workspace = codex.workspace(Workspace {
        profiles: vec![profile],
        tabs: vec![tab],
        ..Workspace::default()
    });
    configure(&mut workspace);
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    (app, codex)
}

const LONG_QUERY: &str = "SELECT 1;\n\nSELECT 2;\n\n-- Bookings by state\nSELECT\n  state,\n  COUNT(*) AS bookings,\n  MAX(booked_at) AS last_booked_at\nFROM integrations.bookings\nGROUP BY state;";

#[gpui_kit::test]
fn appended_sql_keeps_earlier_queries_and_titles_the_conversation(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, "", |_| {});
    app.open_assistant(cx);
    app.hover_labelled(cx, "Send");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, cx| {
        assert_eq!(
            label(window, "status-tooltip-title").as_deref(),
            Some("Send message")
        );
        assert_eq!(
            label(window, "status-tooltip-shortcut").as_deref(),
            Some("⌘⏎")
        );
        assert_tooltip_header_center(window, cx, "status-tooltip-shortcut");
        assert_eq!(
            label(window, "assistant-send-mode").as_deref(),
            Some("SQL Mode: Ask first")
        );
        assert!(
            bounds_of(window, "assistant-send-mode").right()
                < bounds_of(window, "assistant-send").left()
        );
    });
    app.send(cx, "Write SELECT 1 into this tab");
    app.wait_editor(cx, "SELECT 1");
    app.wait_idle(cx);
    // The first message starts title generation alongside the reply.
    app.wait_conversations(cx, &[("Title: Write SELECT 1", Codex)]);
    app.send(cx, "Write SELECT 2 into this tab");
    app.wait_editor(cx, "SELECT 1;\n\nSELECT 2");
    app.wait_idle(cx);
    // A generated title stays after later replies.
    app.settle(cx);
    assert_eq!(
        app.conversations(),
        [("Title: Write SELECT 1".to_owned(), Codex)]
    );
    // Qrow formats a long query when the assistant appends it. The comment
    // above the query stays as written.
    app.send(cx, "Write a long query into this tab");
    app.wait_reply(cx, "I formatted the SQL.");
    app.wait_editor(cx, LONG_QUERY);
}

#[gpui_kit::test]
fn run_mode_needs_confirmation_and_reconnect_replaces_send(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, "", |_| {});
    app.open_assistant(cx);
    app.choose_send_mode(cx, "Run automatically");
    app.click(cx, "cancel-conversation-auto-run");
    app.wait_gone(cx, "cancel-conversation-auto-run");
    app.update(cx, |window, _| {
        assert_eq!(label(window, "assistant-send").as_deref(), Some("Send"))
    });
    app.choose_send_mode(cx, "Run automatically");
    app.click(cx, "confirm-conversation-auto-run");
    app.wait_until(
        cx,
        "SQL Mode: Auto run",
        std::time::Duration::from_secs(10),
        |window, _| label(window, "assistant-send-mode").as_deref() == Some("SQL Mode: Auto run"),
    );

    // Reconnect replaces Send and Cancel while Codex is disconnected.
    app.send(cx, "Disconnect Codex");
    app.wait_for(cx, "assistant-reconnect");
    app.update(cx, |window, _| {
        for id in ["assistant-send", "assistant-stop"] {
            assert!(
                !present(window, &ElementId::Name(id.into())),
                "{id} stayed visible"
            );
        }
    });
    app.click(cx, "assistant-reconnect");
    app.wait_gone(cx, "assistant-reconnect");
    app.wait_for(cx, "assistant-send");
    app.send_when_ready(cx, "Reply after reconnect");
    app.wait_reply(cx, "I can help with this query");
    app.wait_idle(cx);
}

#[gpui_kit::test]
fn requests_target_one_statement_and_edits_use_the_sql_style(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, "SELECT 0;", |workspace| {
        workspace.settings.editor_tab_size = 4;
        workspace.settings.assistant.sql_keyword_case = KeywordCase::Lowercase;
    });
    app.wait_editor(cx, "SELECT 0;");
    app.open_assistant(cx);
    app.send(cx, "Append two SQL statements with edit tool");
    app.wait_editor(cx, "SELECT 0;\n\nSELECT 1;\n\nSELECT 2");
    app.wait_idle(cx);

    // The newest statement, and an earlier one selected by its byte range.
    for (message, sql) in [
        ("Run selected SQL without range", "SELECT 2"),
        ("Run first SQL by range", "SELECT 0;"),
    ] {
        app.send(cx, message);
        app.wait_approval(cx, &format!("Run in Query 1 on Synthetic? {sql}"));
        app.click(cx, "assistant-cancel-query");
        app.wait_gone(cx, "assistant-query-approval");
        app.wait_idle(cx);
    }

    // An edit that supplies a long statement uses the saved style.
    app.send(cx, "Rewrite the last statement with edit tool");
    app.wait_reply(cx, "I formatted the SQL.");
    app.wait_editor(
        cx,
        "SELECT 0;\n\nSELECT 1;\n\nselect\n    state,\n    count(*) as bookings,\n    max(booked_at) as last_booked_at\nfrom integrations.bookings\ngroup by state",
    );
    app.wait_idle(cx);

    // A change in Settings applies to the next message.
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "tab size");
    app.wait_for(cx, "setting-editor-tab-size");
    assert_eq!(app.stepper(cx, "setting-editor-tab-size"), "4");
    app.step(cx, "setting-editor-tab-size", "increment", "5");
    app.fill_labelled(cx, "Search...", "keyword");
    app.wait_for(cx, "setting-sql-keyword-case");
    app.select(cx, "setting-sql-keyword-case", "Uppercase");
    app.click(cx, "save-settings");
    app.wait_gone(cx, "save-settings");
    app.send(cx, "Report the SQL style");
    app.wait_reply(cx, "SQL style: uppercase, 5 spaces");

    // A later selection cannot reuse the revision of the appended query.
    app.send(cx, "Append then retarget and run without revision");
    app.wait_approval(cx, "Run in Query 1 on Synthetic? SELECT 0;");
    app.click(cx, "assistant-cancel-query");
    app.wait_reply(cx, "Implicit run rejected: invalid_arguments");
    app.update(cx, |window, _| {
        assert!(
            approval(window).is_none(),
            "An implicit run targeted another statement"
        );
    });
}

#[gpui_kit::test]
fn a_long_tab_sends_a_bounded_part_and_the_assistant_reads_the_rest(cx: &mut TestAppContext) {
    // 4,000 statements of 14 bytes: longer than the 32 KB part in the context.
    let sql: String = (0..4000).map(|n| format!("SELECT {n:05};\n")).collect();
    let (app, _codex) = launch(cx, &sql, |_| {});
    app.open_assistant(cx);
    app.send(cx, "Report the tab SQL window");
    app.wait_reply(cx, ", 32768 of 56000 bytes, truncated True");
    app.wait_idle(cx);
    // read_tab_sql returns the SQL in parts of at most 32 KB.
    app.send(cx, "Read the tab SQL in pages");
    app.wait_reply(cx, "Read 56000 of 56000 bytes in 2 pages.");
}

#[gpui_kit::test]
fn the_logs_tool_reads_only_the_latest_execution(cx: &mut TestAppContext) {
    let (app, _codex) = launch(cx, "SELECT 1; SELECT 2", |_| {});
    // Rejected SQL makes a Logs group without an execution.
    app.click(cx, "run");
    app.type_sql(cx, "SELECT 'logged-query'");
    app.click(cx, "run");

    app.open_assistant(cx);
    app.send(cx, "Read the latest execution logs");
    app.wait_reply(cx, "Logs: query True, rejected SQL False");
}

#[gpui_kit::test]
fn the_catalog_tools_read_the_cached_schemas_without_a_session(cx: &mut TestAppContext) {
    use qrow::catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind};
    use qrow::model::CatalogSettings;
    use std::collections::BTreeMap;

    let (directory, codex) = FakeCodex::new();
    let profile = offline_profile("Synthetic");
    let mut catalog = Catalog::new(&profile);
    catalog.apply_schemas(
        vec!["avia".into(), "finance".into()],
        &CatalogSettings::default(),
        1,
    );
    let relation = |name: &str, kind| RelationEntry {
        name: name.into(),
        kind,
        comment: None,
    };
    catalog.apply_relations(
        "avia",
        None,
        vec![
            relation("bookings", RelationKind::Table),
            relation("daily", RelationKind::View),
        ],
        1,
    );
    let column = |name: &str, data_type: &str| CatalogColumn {
        name: name.into(),
        data_type: data_type.into(),
        comment: None,
    };
    catalog.apply_columns(
        "avia",
        Some("bookings"),
        BTreeMap::from([(
            "bookings".into(),
            vec![column("booking_id", "BIGINT"), column("gate", "STRING")],
        )]),
        1,
    );
    let cache = qrow::storage::catalog_path(&directory.path().join("workspace.json"), profile.id);
    qrow::storage::save_catalog(&cache, &catalog).unwrap();
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = "SELECT gate FROM avia.bookings".into();
    let tab_id = tab.id;
    let workspace = codex.workspace(Workspace {
        profiles: vec![profile],
        tabs: vec![tab],
        ..Workspace::default()
    });
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);

    // The tools wait for the cache, and the connection has no session, so
    // missing columns are not read.
    app.send(cx, "Read the catalog");
    app.update(cx, |window, _| assert_tab_dot(window, tab_id, None));
    app.wait_reply(
        cx,
        "schemas avia,finance; relations bookings; columns booking_id BIGINT,gate STRING; daily not_cached",
    );
    app.wait_idle(cx);
    app.update(cx, |window, _| assert_tab_dot(window, tab_id, None));
    // The next message has the cached columns of the relation in the tab SQL.
    app.send(cx, "Read the catalog again");
    app.wait_reply(cx, "Catalog: loaded True, referenced bookings;");
    assert_eq!(app.credentials.reads(), 0, "The tools must not connect");
}
