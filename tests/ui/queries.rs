use crate::support::{MemoryCredentials, TestApp, bounds_of, connection_row, label, labelled};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, px};
use qrow::{
    model::{Profile, SavedTab, Workspace},
    storage::Credentials,
};
use std::{net::TcpListener, time::Duration};

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
    let app = TestApp::launch_with(
        cx,
        Workspace {
            profiles: vec![profile],
            tabs: vec![tab],
            ..Workspace::default()
        },
        credentials,
    );
    app.update(cx, |window, _| {
        assert_eq!(window.find(row.clone()).label(), Some("Unreachable"));
    });

    app.update(cx, |window, cx| window.click("run", cx));
    app.wait_until(
        cx,
        "the connection error",
        Duration::from_secs(20),
        |window, _| window.find(row.clone()).label() == Some("Unreachable, unread error"),
    );
    // The warning shares the centerline of the New Connection button.
    app.update(cx, |window, _| {
        let warning = bounds_of(window, &format!("connection-error-{}", profile_id));
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
    app.click(cx, "run");
    app.wait_until(
        cx,
        "the failed query",
        Duration::from_secs(20),
        |window, _| labelled(window, "Query 1, unread error").is_some(),
    );

    // A failed query marks its tab. It does not count in the status bar.
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
            Some("Activity")
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
