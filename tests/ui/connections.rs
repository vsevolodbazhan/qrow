use crate::support::TestApp;
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::model::Workspace;
use std::time::Duration;

#[gpui_kit::test]
fn new_connection_saves_its_password_in_the_injected_store(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    app.update(cx, |window, cx| window.click("add-connection", cx));
    app.wait_until(
        cx,
        "the connection form",
        Duration::from_secs(5),
        |window, _| window.try_find("connection-name").is_some(),
    );
    app.fill(cx, "connection-name", "Synthetic Alpha");
    app.fill(cx, "connection-host", "127.0.0.1");
    app.fill(cx, "connection-port", "10009");
    app.fill(cx, "connection-username", "synthetic-user");
    app.fill(cx, "connection-password", "synthetic-password");
    app.update(cx, |window, cx| window.click("save-profile", cx));

    app.wait_until(
        cx,
        "the saved connection",
        Duration::from_secs(10),
        |_, _| {
            app.saved()
                .profiles
                .iter()
                .any(|profile| profile.name == "Synthetic Alpha")
        },
    );
    let saved = app.saved();
    let profile = saved
        .profiles
        .iter()
        .find(|profile| profile.name == "Synthetic Alpha")
        .unwrap();
    assert_eq!(profile.host, "127.0.0.1");
    assert_eq!(profile.port, 10009);
    assert_eq!(profile.username, "synthetic-user");
    assert_eq!(
        app.credentials.get(profile.id).as_deref(),
        Some("synthetic-password")
    );
    let json = std::fs::read_to_string(app.workspace_path()).unwrap();
    assert!(
        !json.contains("synthetic-password"),
        "The workspace file must not contain the password"
    );
}
