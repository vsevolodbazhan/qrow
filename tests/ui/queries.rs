use crate::support::{MemoryCredentials, TestApp};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
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
    assert_eq!(
        app.credentials.reads(),
        1,
        "The worker must read the password from the injected store"
    );
}
