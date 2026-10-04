use crate::support::{
    MemoryCredentials, TestApp, assert_connection_dot, assert_tab_dot, bounds_of, connection_row,
    label, labelled,
};
use gpui_kit::component::ActiveTheme;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, px};
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
        });
    };
    hover(format!("query-status-{query}"), &app, cx);
    // Keep the failure unread by showing a different tab.
    app.click(cx, "new-tab");
    hover(format!("connection-status-{connection}"), &app, cx);
    drop(listener);
    app.wait_until(
        cx,
        "the failed session",
        Duration::from_secs(10),
        |window, _| label(window, "status-tooltip-status").as_deref() == Some("Unread Error"),
    );
    app.update(cx, |window, cx| {
        assert_tab_dot(window, query, Some(cx.theme().danger));
        assert_connection_dot(window, connection, cx.theme().danger);
    });
}

#[gpui_kit::test]
fn long_connection_tooltips_keep_status_and_details_inside_at_large_scale(cx: &mut TestAppContext) {
    let name = "W".repeat(60);
    let profile = crate::support::offline_profile(&name);
    let id = profile.id;
    let mut workspace = Workspace {
        tabs: vec![SavedTab::new(1, Some(id))],
        profiles: vec![profile],
        ..Workspace::default()
    };
    workspace.settings.ui_scale = 1.5;
    workspace.settings.theme = "Ayu Light".into();
    let app = TestApp::launch(cx, workspace);
    app.hover_labelled(cx, &name);
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, _| {
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
            "status-tooltip-detail",
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
    app.update(cx, |window, _| {
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
            Some("⇧⌘U")
        );
        let title = bounds_of(window, "status-tooltip-title");
        let shortcut = bounds_of(window, "status-tooltip-shortcut");
        let status = bounds_of(window, "status-tooltip-status");
        assert!(shortcut.left() > title.right());
        assert!(shortcut.top() < title.bottom());
        assert_eq!(title.left(), status.left());
        assert!(status.top() > title.bottom());
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
        |window, _| label(window, "status-tooltip-status").as_deref() == Some("1 Unread Error"),
    );
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "status-tooltip-status").as_deref(),
            Some("1 Unread Error")
        );
        assert_eq!(
            label(window, "status-tooltip-shortcut").as_deref(),
            Some("⇧⌘U")
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
    app.choose(cx, "popup-menu", "Show Activity");
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
    app.choose(cx, "popup-menu", "Show Activity");
    app.wait_for(cx, "activity");
    app.click_labelled(cx, "Show Tab");
    app.wait_gone(cx, "activity");
    app.wait_until(cx, "the first tab", Duration::from_secs(10), |_, _| {
        app.saved().active_tab == 0
    });

    // A closed tab keeps its entries without a link.
    app.press(cx, "cmd-w");
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Show Activity");
    app.wait_for(cx, "activity");
    assert!(app.copy_activity(cx).contains("Query 1: Submitted a query"));
    app.update(cx, |window, _| {
        assert!(labelled(window, "Show Tab").is_none());
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
