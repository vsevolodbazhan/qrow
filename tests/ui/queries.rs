use crate::support::{
    MemoryCredentials, TestApp, assert_connection_dot, assert_tab_dot,
    assert_tooltip_header_center, assert_tooltip_status_below, bounds_of, connection_row, label,
    labelled,
};
use gpui_kit::component::{ActiveTheme, input::EMPHASIS_OPACITY};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Bounds, ScaledPixels, TestAppContext, px};
use qrow::{
    activity::ActivityEntry,
    logs::Severity,
    model::{Profile, SavedTab, Workspace},
    storage::Credentials,
};
use std::{
    net::TcpListener,
    time::{Duration, SystemTime},
};

#[gpui_kit::test]
fn run_is_a_compact_text_button(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    app.update(cx, |window, _| {
        let button = bounds_of(window, "run");
        assert_eq!(label(window, "run").as_deref(), Some("Run"));
        assert!(
            button.size.width <= button.size.height * 2.,
            "Run should fit its text without space for a leading icon: {button:?}"
        );
    });
}

#[gpui_kit::test]
fn run_uses_the_cursor_statement_and_rejects_invalid_selections(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    app.type_sql(cx, "SELECT '日本語😀;';\nSELECT 2;\n-- trailing ;\n");
    app.press(cx, "secondary-a");
    app.press(cx, "left");
    app.press(cx, "secondary-enter");
    assert!(
        app.logs(cx)
            .contains("Choose a connection before running SQL.")
    );
    assert_eq!(app.credentials.reads(), 0);

    app.click(cx, "sql-editor");
    app.press(cx, "secondary-a");
    app.press(cx, "secondary-enter");
    assert!(app.logs(cx).contains("Run one statement at a time."));
    assert_eq!(app.credentials.reads(), 0);

    for (index, text) in ["", "\n-- only a comment;\n/* another ; */\n", "   "]
        .into_iter()
        .enumerate()
    {
        if text.is_empty() {
            app.click(cx, "sql-editor");
            app.press(cx, "secondary-a");
            app.press(cx, "backspace");
            app.wait_until(
                cx,
                "the empty editor to be saved",
                Duration::from_secs(10),
                |_, _| app.saved().tabs[0].sql.is_empty(),
            );
        } else {
            app.type_sql(cx, text);
        }
        app.press(cx, "secondary-enter");
        assert_eq!(
            app.logs(cx)
                .matches("Write or select a SQL statement first.")
                .count(),
            index + 1
        );
        assert_eq!(app.credentials.reads(), 0);
    }
}

#[gpui_kit::test]
fn run_uses_a_multiline_cursor_statement_and_rejects_gaps(cx: &mut TestAppContext) {
    let sql =
        "SELECT '最初😀;';\n\nSELECT\n  'middle😀;' AS value;\n\n-- between queries;\nSELECT 3;\n";
    let profile = crate::support::offline_profile("Cursor query");
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = sql.into();
    let app = TestApp::launch(
        cx,
        Workspace {
            profiles: vec![profile],
            tabs: vec![tab],
            ..Workspace::default()
        },
    );
    app.click(cx, "sql-editor");
    app.press(cx, "secondary-a");
    app.press(cx, "left");
    for _ in 0..3 {
        app.press(cx, "down");
    }
    app.press(cx, "secondary-enter");
    app.wait_status(cx, "Error");
    assert!(app.logs(cx).contains("SELECT\n  'middle😀;' AS value;"));
    assert!(!app.logs(cx).contains("SELECT 3;"));
    let reads = app.credentials.reads();

    // The blank line and standalone comment do not submit another query.
    for line in [4, 5, 7] {
        app.click(cx, "sql-editor");
        app.press(cx, "secondary-a");
        app.press(cx, "left");
        for _ in 0..line {
            app.press(cx, "down");
        }
        app.click(cx, "run");
        assert!(
            app.logs(cx)
                .contains("Move the cursor into a SQL statement or select SQL to run.")
        );
        assert_eq!(app.credentials.reads(), reads);
        assert!(!app.logs(cx).contains("SELECT 3;"));
    }
    assert_eq!(app.saved().tabs[0].sql, sql);
}

#[gpui_kit::test]
fn the_editor_tints_the_statement_that_run_uses(cx: &mut TestAppContext) {
    let sql =
        "SELECT 1; SELECT 2; -- note\n\nSELECT\n  '日本😀' AS v\nFROM t;\n-- between\nSELECT 4;";
    let mut tab = SavedTab::new(1, None);
    tab.sql = sql.into();
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![tab],
            ..Workspace::default()
        },
    );
    app.click(cx, "sql-editor");
    app.press(cx, "secondary-a");
    app.press(cx, "left");

    // Statements that share a line tint only their own text.
    let first = run_tint(&app, cx);
    assert_eq!(first.len(), 1, "{first:?}");
    for _ in 0.."SELECT 1; ".len() {
        app.press(cx, "right");
    }
    let second = run_tint(&app, cx);
    assert_eq!(second.len(), 1, "{second:?}");
    assert_eq!(second[0].top(), first[0].top());
    assert!(second[0].left() > first[0].right());

    // A statement on its own lines tints each line to the right edge.
    app.press(cx, "down");
    app.press(cx, "down");
    let rows = run_tint(&app, cx);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows[0].top() > second[0].bottom());
    for row in &rows {
        assert_eq!(row.left(), first[0].left());
        assert_eq!(row.right(), rows[0].right());
        assert!(row.right() > second[0].right());
    }
    for pair in rows.windows(2) {
        assert_eq!(pair[0].bottom(), pair[1].top());
    }

    // A comment between statements, a blank line, and a selection run nothing,
    // so they show no tint.
    for _ in 0..3 {
        app.press(cx, "down");
    }
    assert_eq!(run_tint(&app, cx), vec![]);
    app.press(cx, "secondary-a");
    app.press(cx, "left");
    app.press(cx, "down");
    assert_eq!(run_tint(&app, cx), vec![]);
    app.press(cx, "up");
    assert_eq!(run_tint(&app, cx).len(), 1);
    app.press(cx, "shift-right");
    assert_eq!(run_tint(&app, cx), vec![]);

    // The tint follows edits.
    app.type_sql(cx, "SELECT 9;\nSELECT 10;");
    let last = run_tint(&app, cx);
    assert_eq!(last.len(), 1, "{last:?}");
    assert_eq!(last[0].top(), first[0].top() + first[0].size.height);
}

#[gpui_kit::test]
fn the_tint_follows_large_sql_after_its_background_index(cx: &mut TestAppContext) {
    // More than 64 KB, so the statements index on a background thread.
    let sql = format!(
        "SELECT 1; SELECT 2;\n{}",
        "SELECT 'filler';\n".repeat(5_000)
    );
    let mut tab = SavedTab::new(1, None);
    tab.sql = sql.clone();
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![tab],
            ..Workspace::default()
        },
    );
    app.click(cx, "sql-editor");
    // Ctrl+Up does not move to the start on Linux.
    app.press(cx, "secondary-a");
    app.press(cx, "left");
    app.settle(cx);
    let first = run_tint(&app, cx);
    assert_eq!(first.len(), 1, "{first:?}");

    for _ in 0.."SELECT 1; ".len() {
        app.press(cx, "right");
    }
    app.settle(cx);
    let second = run_tint(&app, cx);
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(second[0].left() > first[0].right());

    // An edit indexes the new text, and the tint follows it to the
    // statement after the inserted one.
    app.update(cx, |window, cx| window.input("SELECT 9; ", cx));
    app.settle(cx);
    let edited = run_tint(&app, cx);
    assert_eq!(edited.len(), 1, "{edited:?}");
    assert_eq!(edited[0].top(), second[0].top());
    assert!(edited[0].left() > second[0].left(), "{edited:?} {second:?}");

    // Small SQL indexes at once. Then large SQL that indexed on a
    // background thread before comes back, and the tint follows it again.
    let paste = |app: &TestApp, cx: &mut TestAppContext, text: &str| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.to_owned()));
        app.press(cx, "secondary-a");
        app.press(cx, "secondary-v");
        app.settle(cx);
    };
    paste(&app, cx, &sql);
    paste(&app, cx, "SELECT 5;");
    paste(&app, cx, &sql);
    // Ctrl+Up does not move to the start on Linux.
    app.press(cx, "secondary-a");
    app.press(cx, "left");
    app.settle(cx);
    let restored = run_tint(&app, cx);
    assert_eq!(restored.len(), 1, "{restored:?}");
    assert_eq!(restored[0].top(), first[0].top());
}

/// The rows that the run tint paints in the SQL editor, top to bottom.
fn run_tint(app: &TestApp, cx: &mut TestAppContext) -> Vec<Bounds<ScaledPixels>> {
    app.update(cx, |window, cx| {
        let tint = gpui_kit::Background::from(cx.theme().selection.opacity(EMPHASIS_OPACITY));
        let editor = bounds_of(window, "sql-editor").scale(window.scale_factor());
        let mut rows: Vec<_> = window
            .painted_quads()
            .into_iter()
            .filter(|quad| quad.background == tint && editor.contains(&quad.bounds.center()))
            .map(|quad| quad.bounds)
            .collect();
        rows.sort_by(|a, b| a.top().0.total_cmp(&b.top().0));
        rows
    })
}

#[gpui_kit::test]
fn connecting_dots_and_tooltips_follow_a_failed_session_open(cx: &mut TestAppContext) {
    // A loopback socket holds sign-in until the test closes it.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let profile = Profile {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        ..crate::support::offline_profile("Connecting warehouse")
    };
    let connection = profile.id;
    let mut tab = SavedTab::new(1, Some(connection));
    tab.sql = "SELECT 1".into();
    let query = tab.id;
    let credentials = MemoryCredentials::default();
    credentials
        .set_password(connection, "synthetic-password")
        .unwrap();
    let app = TestApp::launch_with(
        cx,
        Workspace {
            profiles: vec![profile],
            tabs: vec![tab],
            ..Workspace::default()
        },
        credentials,
    );
    app.click(cx, "run");
    app.wait_until(
        cx,
        "the connecting session",
        Duration::from_secs(10),
        |window, _| {
            label(window, format!("query-status-{query}")).as_deref() == Some("Query 1, connecting")
        },
    );
    app.update(cx, |window, cx| {
        assert_tab_dot(window, query, Some(cx.theme().info.opacity(0.2)));
        assert_connection_dot(window, connection, cx.theme().info.opacity(0.2));
        assert_eq!(
            label(window, format!("connection-status-{connection}")).as_deref(),
            Some("Connecting warehouse, connecting")
        );
        assert_eq!(
            label(window, "toggle-activity").as_deref(),
            Some("Activity")
        );
    });
    let hover = |id: String, app: &TestApp, cx: &mut TestAppContext| {
        app.update(cx, |window, cx| window.hover(id, cx));
        cx.executor().advance_clock(Duration::from_millis(800));
        app.settle(cx);
        app.update(cx, |window, _| {
            assert_eq!(
                label(window, "status-tooltip-status").as_deref(),
                Some("Connecting")
            );
            assert_tooltip_status_below(window);
        });
    };
    hover(format!("query-status-{query}"), &app, cx);
    app.update(cx, |window, _| {
        assert!(window.try_find("status-tooltip-detail").is_none());
    });
    // Keep the failure unread by showing a different tab.
    app.click(cx, "new-tab");
    hover(format!("connection-status-{connection}"), &app, cx);
    drop(listener);
    app.wait_until(
        cx,
        "the failed session",
        Duration::from_secs(10),
        |window, _| label(window, "status-tooltip-status").as_deref() == Some("Unread error"),
    );
    app.update(cx, |window, cx| {
        assert_tab_dot(window, query, Some(cx.theme().danger));
        assert_connection_dot(window, connection, cx.theme().danger);
    });
}

#[gpui_kit::test]
fn long_connection_tooltips_align_status_and_fit_at_each_scale_and_theme(cx: &mut TestAppContext) {
    for (scale, theme) in [(0.75, "One Dark"), (1., "Ayu Light"), (1.5, "Ayu Light")] {
        let name = "W".repeat(60);
        let profile = crate::support::offline_profile(&name);
        let id = profile.id;
        let mut workspace = Workspace {
            tabs: vec![SavedTab::new(1, Some(id))],
            profiles: vec![profile],
            ..Workspace::default()
        };
        workspace.settings.ui_scale = scale;
        workspace.settings.theme = theme.into();
        let app = TestApp::launch(cx, workspace);
        app.hover_labelled(cx, &name);
        cx.executor().advance_clock(Duration::from_millis(800));
        app.settle(cx);
        app.update(cx, |window, _| {
            assert_tooltip_status_below(window);
            assert_eq!(
                label(window, "status-tooltip-title").as_deref(),
                Some(name.as_str())
            );
            assert_eq!(
                label(window, "status-tooltip-status").as_deref(),
                Some("Disconnected")
            );
            let popup = bounds_of(window, "status-tooltip");
            for id in [
                "status-tooltip-title",
                "status-tooltip-status",
                "status-tooltip-metadata",
            ] {
                let bounds = bounds_of(window, id);
                assert!(
                    bounds.right() <= popup.right(),
                    "{id} extends beyond the tooltip: {bounds:?} > {popup:?}"
                );
                assert!(bounds.bottom() <= popup.bottom());
            }
        });
    }
}

#[gpui_kit::test]
fn activity_tooltip_summarizes_connections_in_use_and_updates_live(cx: &mut TestAppContext) {
    use crate::support::assistant::{FakeCodex, REPLY_TIMEOUT};
    use crate::support::offline_profile;
    use qrow::model::SavedTab;

    let (directory, codex) = FakeCodex::new();
    let unused = offline_profile("Unused warehouse");
    let profile = offline_profile("Assistant warehouse");
    let connection = profile.id;
    let tab = SavedTab::new(1, Some(connection));
    let query = tab.id;
    let app = TestApp::launch_in(
        cx,
        directory,
        codex.workspace(Workspace {
            tabs: vec![tab],
            profiles: vec![profile, unused.clone()],
            ..Workspace::default()
        }),
        MemoryCredentials::default(),
    );
    app.open_assistant(cx);
    app.send(cx, "Title before first reply");
    app.wait_until(cx, "the held reply", REPLY_TIMEOUT, |_, _| {
        codex.marked("first-reply-pending")
    });
    app.wait_label(cx, "Activity, in use");
    app.hover_labelled(cx, "Activity, in use");
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, cx| {
        assert_eq!(
            label(window, "status-tooltip-status").as_deref(),
            Some("In use")
        );
        assert_connection_dot(window, connection, cx.theme().info);
        assert_tab_dot(window, query, None);
        assert_tooltip_header_center(window, cx, "status-tooltip-shortcut");
    });

    // An error on another connection keeps its count with the work summary.
    app.qrow
        .update(cx, |qrow, cx| {
            qrow.record_activity(
                unused.id,
                ActivityEntry::new(Severity::Error, "Synthetic refresh error"),
                cx,
            );
        })
        .unwrap();
    app.wait_until(
        cx,
        "the unread error and work",
        REPLY_TIMEOUT,
        |window, _| {
            label(window, "status-tooltip-status").as_deref() == Some("1 unread error\nIn use")
        },
    );

    // Keep the pointer over Activity as the working connection becomes idle.
    codex.mark("first-reply-release");
    app.wait_until(cx, "the finished work", REPLY_TIMEOUT, |window, _| {
        label(window, "status-tooltip-status").as_deref() == Some("1 unread error")
    });
}

#[gpui_kit::test]
fn activity_tooltips_keep_the_shortcut_above_unread_status(cx: &mut TestAppContext) {
    let profile = crate::support::offline_profile("Tooltip warehouse");
    let id = profile.id;
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![SavedTab::new(1, Some(id))],
            profiles: vec![profile],
            ..Workspace::default()
        },
    );
    let hover = |app: &TestApp, cx: &mut TestAppContext, title| {
        app.hover_labelled(cx, title);
        cx.executor().advance_clock(Duration::from_millis(800));
        app.settle(cx);
    };
    hover(&app, cx, "Activity");
    app.update(cx, |window, cx| {
        assert_eq!(
            label(window, "status-tooltip-title").as_deref(),
            Some("Activity")
        );
        assert_eq!(
            label(window, "status-tooltip-status").as_deref(),
            Some("Idle")
        );
        assert_eq!(
            label(window, "status-tooltip-shortcut").as_deref(),
            Some(if cfg!(target_os = "macos") {
                "⇧⌘U"
            } else {
                "Ctrl+Shift+U"
            })
        );
        let title = bounds_of(window, "status-tooltip-title");
        let shortcut = bounds_of(window, "status-tooltip-shortcut");
        let status = bounds_of(window, "status-tooltip-status");
        assert!(shortcut.left() > title.right());
        assert!(shortcut.top() < title.bottom());
        assert_eq!(title.left(), status.left());
        assert!(status.top() > title.bottom());
        assert_tooltip_header_center(window, cx, "status-tooltip-shortcut");
    });
    app.qrow
        .update(cx, |qrow, cx| {
            qrow.record_activity(
                id,
                ActivityEntry::new(Severity::Error, "Synthetic refresh error"),
                cx,
            );
        })
        .unwrap();
    app.wait_until(
        cx,
        "the unread error in the open tooltip",
        Duration::from_secs(10),
        |window, _| label(window, "status-tooltip-status").as_deref() == Some("1 unread error"),
    );
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "status-tooltip-status").as_deref(),
            Some("1 unread error")
        );
        assert_eq!(
            label(window, "status-tooltip-shortcut").as_deref(),
            Some(if cfg!(target_os = "macos") {
                "⇧⌘U"
            } else {
                "Ctrl+Shift+U"
            })
        );
    });
    app.click(cx, "toggle-activity");
    app.wait_for(cx, "activity");
    app.update(cx, |window, _| {
        assert!(window.try_find("status-tooltip").is_none())
    });
}

#[gpui_kit::test]
fn activity_copy_preserves_timestamps_and_error_details(cx: &mut TestAppContext) {
    let profile = Profile {
        name: "Activity timestamps".into(),
        ..Profile::default()
    };
    let id = profile.id;
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![SavedTab::new(1, Some(id))],
            profiles: vec![profile],
            ..Workspace::default()
        },
    );
    app.qrow
        .update(cx, |qrow, cx| {
            for (seconds, severity, text) in [
                (86_400, Severity::Info, "Schema refresh started"),
                (86_401, Severity::Error, "Schema refresh failed\n詳細 🐦"),
            ] {
                let mut entry = ActivityEntry::new(severity, text);
                entry.timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
                qrow.record_activity(id, entry, cx);
            }
        })
        .expect("The window is open");
    app.settle(cx);
    app.context_menu(cx, connection_row(id));
    app.choose(cx, "popup-menu", "Show activity");
    app.wait_for(cx, "activity");
    app.click(cx, "activity-errors");

    let expected = "[1970-01-02 00:00:01] Schema refresh failed\n詳細 🐦";
    assert_eq!(app.copy_activity(cx), expected);
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(String::new()));
    app.click(cx, ("activity-copy", 2u64));
    app.settle(cx);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some(expected.to_owned())
    );

    app.click(cx, "activity-all");
    assert_eq!(
        app.copy_activity(cx),
        concat!(
            "[1970-01-02 00:00:00] Schema refresh started\n",
            "[1970-01-02 00:00:01] Schema refresh failed\n詳細 🐦",
        )
    );
}

#[gpui_kit::test]
fn connection_failure_reaches_the_connection_list(cx: &mut TestAppContext) {
    // A port that was free a moment ago refuses the connection.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let profile = Profile {
        name: "Unreachable".into(),
        host: "127.0.0.1".into(),
        port,
        username: "synthetic-user".into(),
        database: "default".into(),
        ..Profile::default()
    };
    let credentials = MemoryCredentials::default();
    credentials
        .set_password(profile.id, "synthetic-password")
        .unwrap();
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = "SELECT 1".into();
    let row = gpui_kit::ElementId::Name(format!("profile-{}", profile.id).into());
    let profile_id = profile.id;
    let second = SavedTab::new(2, Some(profile.id));
    let app = TestApp::launch_with(
        cx,
        Workspace {
            profiles: vec![profile],
            tabs: vec![tab, second],
            ..Workspace::default()
        },
        credentials,
    );
    app.update(cx, |window, _| {
        assert_eq!(window.find(row.clone()).label(), Some("Unreachable"));
    });

    app.update(cx, |window, cx| {
        window.click("run", cx);
        let second = labelled(window, "Query 2").unwrap();
        crate::support::click_element(window, &second, cx);
    });
    app.wait_until(
        cx,
        "the connection error",
        Duration::from_secs(20),
        |window, _| window.find(row.clone()).label() == Some("Unreachable, unread error"),
    );
    // The dot shares the centerline of the New Connection button.
    app.update(cx, |window, _| {
        let warning = bounds_of(window, &format!("connection-status-{}", profile_id));
        let add = bounds_of(window, "add-connection");
        let (warning, add) = (warning.center().x, add.center().x);
        assert!((warning - add).abs() < px(0.5), "{warning:?} != {add:?}");
    });
    assert_eq!(
        app.credentials.reads(),
        1,
        "The worker must read the password from the injected store"
    );
}

#[gpui_kit::test]
fn activity_links_the_queries_of_a_connection_to_their_tabs(cx: &mut TestAppContext) {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let profile = Profile {
        name: "Unreachable".into(),
        host: "127.0.0.1".into(),
        port,
        username: "synthetic-user".into(),
        database: "default".into(),
        ..Profile::default()
    };
    let credentials = MemoryCredentials::default();
    credentials
        .set_password(profile.id, "synthetic-password")
        .unwrap();
    let mut first = SavedTab::new(1, Some(profile.id));
    first.sql = "SELECT 'hidden-text'".into();
    let second = SavedTab::new(2, Some(profile.id));
    let app = TestApp::launch_with(
        cx,
        Workspace {
            profiles: vec![profile.clone()],
            tabs: vec![first, second],
            ..Workspace::default()
        },
        credentials,
    );
    app.click_labelled(cx, "Query 1");
    app.update(cx, |window, cx| {
        window.click("run", cx);
        let second = labelled(window, "Query 2").unwrap();
        crate::support::click_element(window, &second, cx);
    });
    app.wait_until(
        cx,
        "the failed query",
        Duration::from_secs(20),
        |window, _| labelled(window, "Query 1, unread error").is_some(),
    );

    // A failed query marks its tab and Activity until its Logs show.
    let activity = app.activity(cx, profile.id);
    assert!(
        activity.contains("Query 1: Submitted a query"),
        "{activity}"
    );
    assert!(activity.contains("Query 1: Query failed"), "{activity}");
    assert!(!activity.contains("hidden-text"), "{activity}");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "toggle-activity").as_deref(),
            Some("Activity, 1 unseen error")
        );
    });

    // Show Tab closes Activity and shows the tab of the query.
    app.click_labelled(cx, "Query 2");
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Show activity");
    app.wait_for(cx, "activity");
    // Show tab shows when the pointer is over its row.
    app.hover_labelled(cx, "Show tab");
    app.click_labelled(cx, "Show tab");
    app.wait_gone(cx, "activity");
    app.wait_until(cx, "the first tab", Duration::from_secs(10), |_, _| {
        app.saved().active_tab == 0
    });

    // A closed tab keeps its entries without a link.
    app.press(cx, "secondary-w");
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Show activity");
    app.wait_for(cx, "activity");
    assert!(app.copy_activity(cx).contains("Query 1: Submitted a query"));
    app.update(cx, |window, _| {
        assert!(labelled(window, "Show tab").is_none());
    });
}

#[gpui_kit::test]
fn activity_does_not_read_a_hidden_tabs_local_error(cx: &mut TestAppContext) {
    let profile = crate::support::offline_profile("Synthetic");
    let mut first = SavedTab::new(1, Some(profile.id));
    first.sql = "SELECT 1; SELECT 2;".into();
    let app = TestApp::launch_with(
        cx,
        Workspace {
            profiles: vec![profile],
            tabs: vec![first],
            ..Workspace::default()
        },
        MemoryCredentials::default(),
    );
    let idle_button = app.update(cx, |window, _| bounds_of(window, "toggle-activity"));
    assert_eq!(idle_button.size.width, idle_button.size.height);
    app.click(cx, "sql-editor");
    app.press(cx, "secondary-a");
    app.click(cx, "toggle-activity");
    app.wait_for(cx, "activity");
    app.dispatch(cx, qrow::ui::RunQuery);
    app.wait_label(cx, "Activity, 1 unseen error");
    app.update(cx, |window, _| {
        assert_eq!(bounds_of(window, "toggle-activity"), idle_button);
    });
    app.dispatch(cx, qrow::ui::NewTab);
    app.press(cx, "escape");
    app.wait_gone(cx, "activity");
    app.wait_label(cx, "Query 1, unread error");
    app.click_labelled(cx, "Query 1, unread error");
    app.wait_label(cx, "Activity");
    assert_eq!(app.credentials.reads(), 0);
    assert!(app.logs(cx).contains("statement"));
}

#[gpui_kit::test]
fn a_tab_without_a_dot_shows_only_its_connection_state_in_its_tooltip(cx: &mut TestAppContext) {
    // No password is stored, so the query fails before a session opens.
    let mut profile = crate::support::offline_profile("Unreachable");
    profile.host = "127.0.0.1".into();
    profile.port = 1;
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = "SELECT 1".into();
    let app = TestApp::launch_with(
        cx,
        Workspace {
            profiles: vec![profile],
            tabs: vec![tab],
            ..Workspace::default()
        },
        MemoryCredentials::default(),
    );
    app.click(cx, "run");
    app.wait_until(cx, "the failed run", Duration::from_secs(10), |_, cx| {
        app.status(cx).starts_with("Error")
    });
    // The error is in sight, so the tab has no dot. The tab has the tooltip.
    app.hover_labelled(cx, "Query 1");
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "status-tooltip-status").as_deref(),
            Some("Not connected")
        );
        assert!(window.try_find("status-tooltip-detail").is_none());
    });
}

#[gpui_kit::test]
fn format_sql_selection_cursor_notices_and_undo(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    let sql = "select '日本語😀;' as value from t;\n\n-- between;\nselect 2 from t;";
    app.type_sql(cx, sql);
    app.press(cx, "secondary-a");
    app.press(cx, "alt-secondary-l");
    let formatted = "SELECT '日本語😀;' AS value\nFROM t;\n\n-- between;\nSELECT 2\nFROM t;";
    app.wait_until(
        cx,
        "formatted selection",
        Duration::from_secs(10),
        |_, _| app.saved().tabs[0].sql == formatted,
    );
    app.press(cx, "secondary-z");
    app.wait_until(cx, "format undo", Duration::from_secs(10), |_, _| {
        app.saved().tabs[0].sql == sql
    });
    app.press(cx, "left");
    app.press(cx, "alt-secondary-l");
    let first_only = "SELECT '日本語😀;' AS value\nFROM t;\n\n-- between;\nselect 2 from t;";
    app.wait_until(
        cx,
        "cursor statement only",
        Duration::from_secs(10),
        |_, _| app.saved().tabs[0].sql == first_only,
    );
    assert_eq!(app.credentials.reads(), 0);

    app.type_sql(cx, "SET x=1");
    app.press(cx, "secondary-a");
    app.press(cx, "alt-secondary-l");
    app.wait_label(
        cx,
        "Some SQL statements are not supported by the formatter.",
    );
    assert_eq!(app.saved().tabs[0].sql, "SET x=1");
    app.update(cx, |window, _| {
        assert_eq!(
            window.find("format-sql-notice").role(),
            Some(gpui_kit::Role::Alert)
        );
        // Every toast leads with a title that names the event.
        let notice = window.within("format-sql-notice");
        let title = notice.find("title");
        assert_eq!(title.label(), Some("Could not format all SQL"));
        assert!(notice.find("detail").bounds().top() >= title.bounds().bottom());
    });
    app.type_sql(cx, "select ${hivevar:dt}");
    app.press(cx, "secondary-a");
    app.press(cx, "alt-secondary-l");
    app.wait_label(cx, "Some SQL could not be formatted safely.");
    assert_eq!(app.saved().tabs[0].sql, "select ${hivevar:dt}");
}

#[gpui_kit::test]
fn format_sql_keeps_selections_inside_quoted_tokens(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    for sql in [
        "SELECT 'select 1' AS value",
        "SELECT `select 1` FROM t",
        "SELECT 1 /*select 1*/",
    ] {
        app.type_sql(cx, sql);
        app.press(cx, "secondary-a");
        app.press(cx, "left");
        for _ in 0..sql.find("select 1").unwrap() {
            app.press(cx, "right");
        }
        for _ in 0..8 {
            app.press(cx, "shift-right");
        }
        app.press(cx, "alt-secondary-l");
        app.wait_label(cx, "Some SQL could not be formatted safely.");
        assert_eq!(app.saved().tabs[0].sql, sql);
    }
}

#[gpui_kit::test]
fn format_sql_uses_editor_style(cx: &mut TestAppContext) {
    let mut workspace = Workspace::default();
    workspace.settings.assistant.sql_keyword_case = qrow::sql::KeywordCase::Lowercase;
    workspace.settings.editor_tab_size = 4;
    let app = TestApp::launch(cx, workspace);
    app.type_sql(cx, "SELECT a FROM t WHERE x=1 AND y=2");
    app.press(cx, "secondary-a");
    app.press(cx, "alt-secondary-l");
    app.wait_until(
        cx,
        "editor format style",
        Duration::from_secs(10),
        |_, _| {
            let text = &app.saved().tabs[0].sql;
            text.starts_with("select a\nfrom t\nwhere") && !text.contains("SELECT")
        },
    );
    app.type_sql(cx, "select 1;\n\nselect 2;");
    app.press(cx, "secondary-a");
    app.press(cx, "left");
    app.press(cx, "down");
    app.press(cx, "alt-secondary-l");
    app.wait_label(
        cx,
        "Move the cursor into a SQL statement or select SQL to format.",
    );
    assert_eq!(app.saved().tabs[0].sql, "select 1;\n\nselect 2;");
}

#[gpui_kit::test]
fn format_real_trino_queries_and_undo(cx: &mut TestAppContext) {
    let sql = include_str!("../fixtures/trino-format.sql");
    let profile = Profile {
        database_type: qrow::model::DatabaseType::Trino,
        ..Profile::default()
    };
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = sql.into();
    let app = TestApp::launch(
        cx,
        Workspace {
            profiles: vec![profile],
            tabs: vec![tab],
            ..Workspace::default()
        },
    );
    app.click(cx, "sql-editor");
    app.press(cx, "secondary-a");
    app.press(cx, "alt-secondary-l");
    app.wait_until(
        cx,
        "formatted Trino cost queries",
        Duration::from_secs(10),
        |_, _| app.saved().tabs[0].sql.matches("SELECT").count() == 3,
    );
    let formatted = app.saved().tabs[0].sql.clone();
    assert_eq!(formatted.matches("DATE'2026-10-").count(), 5);
    assert!(formatted.contains("SUM(unused_cost_usd) / NULLIF("));
    assert!(formatted.contains("\n\nSELECT"));
    app.update(cx, |window, _| {
        assert!(window.try_find("format-sql-notice").is_none())
    });
    app.press(cx, "secondary-z");
    app.wait_until(
        cx,
        "undo Trino cost formatting",
        Duration::from_secs(10),
        |_, _| app.saved().tabs[0].sql == sql,
    );
    assert_eq!(app.credentials.reads(), 0);
}

#[gpui_kit::test]
fn format_sql_uses_each_connections_quoting_rules(cx: &mut TestAppContext) {
    use qrow::model::DatabaseType;
    for (database_type, sql, expected) in [
        (
            DatabaseType::Postgres,
            r#"select $body$select; '日本語😀'$body$ as "Mixed Case";"#,
            r#"SELECT $body$select; '日本語😀'$body$ AS "Mixed Case";"#,
        ),
        (
            DatabaseType::Trino,
            r#"select '日本語😀;' as "Mixed Case";"#,
            r#"SELECT '日本語😀;' AS "Mixed Case";"#,
        ),
        (
            DatabaseType::Kyuubi,
            "select '日本語😀;' as `Mixed Case`;",
            "SELECT '日本語😀;' AS `Mixed Case`;",
        ),
    ] {
        let profile = Profile {
            database_type,
            ..Profile::default()
        };
        let mut tab = SavedTab::new(1, Some(profile.id));
        tab.sql = format!("{sql}\n\nselect 2;");
        let original = tab.sql.clone();
        let app = TestApp::launch(
            cx,
            Workspace {
                profiles: vec![profile],
                tabs: vec![tab],
                ..Workspace::default()
            },
        );
        app.click(cx, "sql-editor");
        app.press(cx, "secondary-a");
        app.press(cx, "left");
        app.press(cx, "alt-secondary-l");
        app.wait_until(
            cx,
            "dialect cursor formatting",
            Duration::from_secs(10),
            |_, _| app.saved().tabs[0].sql == format!("{expected}\n\nselect 2;"),
        );
        app.press(cx, "secondary-z");
        app.wait_until(
            cx,
            "dialect format undo",
            Duration::from_secs(10),
            |_, _| app.saved().tabs[0].sql == original,
        );
        app.press(cx, "secondary-a");
        app.press(cx, "alt-secondary-l");
        app.wait_until(
            cx,
            "dialect selection formatting",
            Duration::from_secs(10),
            |_, _| app.saved().tabs[0].sql == format!("{expected}\n\nSELECT 2;"),
        );
        assert_eq!(app.credentials.reads(), 0);
    }
}

#[gpui_kit::test]
fn a_long_log_draws_only_the_rows_near_the_view_and_selects_its_sql(cx: &mut TestAppContext) {
    use gpui_kit::{
        InputEvent as _, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        point,
    };

    // The demo runs the query of its first tab at launch; each run adds
    // the submitted query and its outcome.
    let app = TestApp::launch_demo(cx);
    app.click(cx, "output-panel-tab");
    app.wait_for(cx, "output-count");
    app.wait_status(cx, "Complete");
    for _ in 0..30 {
        let before = app.update(cx, |window, _| label(window, "output-count"));
        app.click(cx, "sql-editor");
        app.dispatch(cx, qrow::ui::RunQuery);
        app.wait_until(cx, "a new entry", Duration::from_secs(20), |window, _| {
            label(window, "output-count") != before
        });
        // A submission adds a log entry before its outcome. Wait for both,
        // so the final outcome cannot scroll the row during the drag.
        app.wait_status(cx, "Complete");
        app.click(cx, "output-panel-tab");
    }
    app.settle(cx);
    let sql_blocks = |window: &gpui_kit::Window| {
        crate::support::elements(window)
            .into_iter()
            .filter(|element| element.path().last() == Some(&"sql-text".into()))
            .collect::<Vec<_>>()
    };
    // A frame lays out and paints the rows on screen and a band around
    // them, not all 30 queries, so a drag stays fast in a long log.
    let (drawn, sql) = app.update(cx, |window, _| {
        let blocks = sql_blocks(window);
        // A visible snapshot can be partly clipped. Both drag endpoints
        // must stay inside the Logs viewport.
        let viewport = window.find("output-scroll").bounds();
        let visible = blocks
            .iter()
            .rfind(|block| {
                let bounds = block.bounds();
                block.visible()
                    && bounds.top() >= viewport.top()
                    && bounds.bottom() <= viewport.bottom()
            })
            .unwrap()
            .bounds();
        (blocks.len(), visible)
    });
    assert!(drawn < 30, "The Logs drew {drawn} of 30 queries");
    // Select within the text lane; the SQL block also contains buttons.
    let start = point(sql.left() + px(4.), sql.top() + px(4.));
    let end = point(sql.right() - px(4.), sql.bottom() - px(4.));
    app.update(cx, |window, cx| {
        window.activate_window();
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position: start,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(
            MouseMoveEvent {
                position: end,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position: end,
                modifiers: Modifiers::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        let selected = gpui_kit::base::TextSelection::selected_text(window, cx);
        assert!(
            selected.contains("COUNT(*) AS departures"),
            "The drag must select the logged SQL: {selected:?}"
        );
    });
}

#[gpui_kit::test]
fn activity_shows_a_cut_tab_name_in_full_on_hover(cx: &mut TestAppContext) {
    use gpui_kit::{InputEvent as _, MouseMoveEvent};

    const LONG: &str = "Модели Ривенделла по стоимости";
    // The demo runs the query of "Route overview" at launch. A rename and
    // another run add entries under the long name.
    let app = TestApp::launch_demo(cx);
    app.wait_for(cx, "toggle-activity");
    app.context_menu_labelled(cx, "Route overview");
    app.choose(cx, "popup-menu", "Rename…");
    app.fill(cx, "rename-tab-name", LONG);
    app.click(cx, "rename-tab");
    app.wait_gone(cx, "rename-tab-name");
    app.click(cx, "sql-editor");
    app.dispatch(cx, qrow::ui::RunQuery);
    app.click(cx, "toggle-activity");
    app.wait_for(cx, "activity");
    let source = |window: &gpui_kit::Window, name: &str| {
        crate::support::elements(window)
            .into_iter()
            .filter(|element| {
                element.path().last() == Some(&"source".into()) && element.label() == Some(name)
            })
            .find(|element| element.visible())
            .map(|element| element.bounds())
    };
    app.wait_until(
        cx,
        "an entry of the long name",
        Duration::from_secs(20),
        |window, _| source(window, LONG).is_some(),
    );
    let hover = |app: &TestApp, cx: &mut TestAppContext, name: &str| {
        app.update(cx, |window, cx| {
            let bounds = source(window, name).expect(name);
            window.dispatch_event(
                MouseMoveEvent {
                    position: bounds.center(),
                    pressed_button: None,
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
        cx.executor().advance_clock(Duration::from_millis(800));
        app.settle(cx);
        app.update(cx, |window, _| window.try_find("tooltip").is_some())
    };
    assert!(
        hover(&app, cx, LONG),
        "A cut tab name must show its tooltip"
    );
    // Leaving the name hides the tooltip; a name that fits shows none.
    assert!(
        !hover(&app, cx, "Route overview"),
        "A name that fits must not show a tooltip"
    );
}

#[gpui_kit::test]
fn a_long_logged_query_shows_five_lines_until_it_expands(cx: &mut TestAppContext) {
    use gpui_kit::ClipboardItem;

    // The demo runs its 11-line query at launch.
    let app = TestApp::launch_demo(cx);
    app.click(cx, "output-panel-tab");
    let found = |window: &gpui_kit::Window, id: &'static str| {
        crate::support::elements(window)
            .into_iter()
            .find(|element| element.path().last() == Some(&id.into()))
    };
    app.wait_until(
        cx,
        "a logged query",
        Duration::from_secs(10),
        |window, _| found(window, "sql").is_some(),
    );
    let (collapsed, line) = app.update(cx, |window, _| {
        assert!(
            found(window, "sql-fade").is_some(),
            "A long query must fade out"
        );
        (
            found(window, "sql").unwrap().bounds().size.height,
            window.line_height(),
        )
    });
    app.click_labelled(cx, "Expand query");
    app.wait_until(
        cx,
        "the full query",
        Duration::from_secs(10),
        |window, _| labelled(window, "Collapse query").is_some(),
    );
    app.update(cx, |window, _| {
        assert!(
            found(window, "sql-fade").is_none(),
            "A full query has no fade"
        );
        let expanded = found(window, "sql").unwrap().bounds().size.height;
        assert!(
            expanded > collapsed + line * 4.,
            "The full query {expanded:?} must show more lines than {collapsed:?}"
        );
    });
    app.click_labelled(cx, "Collapse query");
    app.wait_until(
        cx,
        "the short query",
        Duration::from_secs(10),
        |window, _| found(window, "sql-fade").is_some(),
    );
    // Copy query copies all of the SQL, also the lines that a collapsed block hides.
    cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
    app.click_labelled(cx, "Copy query");
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(
        copied.starts_with("-- A quick look at route performance")
            && copied.trim_end().ends_with("ORDER BY departures DESC;"),
        "Copy query must copy the full SQL: {copied:?}"
    );
}

#[gpui_kit::test]
fn logged_query_buttons_stay_inside_code_and_clear_of_long_sql(cx: &mut TestAppContext) {
    use qrow::logs::{LogEvent, LogKind};

    let app = TestApp::launch(cx, Workspace::default());
    let sql = format!(
        "SELECT '{}' AS value\n-- two\n-- three\n-- four\n-- five\n-- six",
        "長い列名".repeat(80)
    );
    app.qrow
        .update(cx, |qrow, cx| {
            qrow.record_tab_logs(
                0,
                vec![LogEvent::new(
                    None,
                    Severity::Info,
                    LogKind::Submitted,
                    format!("Submitted query:\n{sql}"),
                )],
                cx,
            );
        })
        .unwrap();
    app.click(cx, "output-panel-tab");
    let check = |app: &TestApp, cx: &mut TestAppContext| {
        app.update(cx, |window, _| {
            let block = bounds_of(window, "sql");
            let text = bounds_of(window, "sql-text");
            let copy = crate::support::elements(window)
                .into_iter()
                .find(|element| element.path().last() == Some(&("output-copy-query", 0_u64).into()))
                .unwrap()
                .bounds();
            let expand = crate::support::elements(window)
                .into_iter()
                .find(|element| element.path().last() == Some(&("output-expand", 0_u64).into()))
                .unwrap()
                .bounds();
            assert!(copy.left() >= block.left() && expand.right() <= block.right());
            assert!(copy.top() >= block.top() && copy.bottom() <= block.bottom());
            assert!(expand.top() >= block.top() && expand.bottom() <= block.bottom());
            assert!(
                text.right() <= copy.left(),
                "SQL must leave a separate button lane"
            );
            assert!(block.right() <= window.viewport_size().width);
        });
    };
    for width in [1280., 900.] {
        cx.simulate_window_resize(app.window, gpui_kit::size(px(width), px(820.)));
        app.settle(cx);
        check(&app, cx);
        app.click(cx, ("output-copy-query", 0_u64));
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(sql.clone())
        );
        app.click(cx, ("output-expand", 0_u64));
        app.settle(cx);
        check(&app, cx);
        app.click(cx, ("output-expand", 0_u64));
        app.settle(cx);
    }
}

#[gpui_kit::test]
fn toggle_comment_selection_cursor_undo_and_redo(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    let sql = "  SELECT '日本語😀';\n\n\tSELECT 2;";
    app.type_sql(cx, sql);
    app.press(cx, "secondary-a");
    app.press(cx, "secondary-/");
    let commented = "  -- SELECT '日本語😀';\n\n\t-- SELECT 2;";
    app.wait_until(cx, "commented lines", Duration::from_secs(10), |_, _| {
        app.saved().tabs[0].sql == commented
    });
    app.press(cx, "secondary-/");
    app.wait_until(cx, "uncommented lines", Duration::from_secs(10), |_, _| {
        app.saved().tabs[0].sql == sql
    });
    app.press(cx, "secondary-z");
    app.wait_until(cx, "undo uncomment", Duration::from_secs(10), |_, _| {
        app.saved().tabs[0].sql == commented
    });
    #[cfg(target_os = "macos")]
    app.press(cx, "secondary-shift-z");
    #[cfg(not(target_os = "macos"))]
    app.press(cx, "ctrl-y");
    app.wait_until(cx, "redo uncomment", Duration::from_secs(10), |_, _| {
        app.saved().tabs[0].sql == sql
    });
    app.press(cx, "secondary-a");
    app.press(cx, "left");
    app.press(cx, "secondary-/");
    app.wait_until(cx, "current line only", Duration::from_secs(10), |_, _| {
        app.saved().tabs[0].sql == "  -- SELECT '日本語😀';\n\n\tSELECT 2;"
    });
    assert_eq!(app.credentials.reads(), 0);
}
