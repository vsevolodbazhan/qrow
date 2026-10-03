use crate::support::fixture::{Kyuubi, QUERY_TIMEOUT};
use crate::support::{TestApp, cell};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn ldap_rejects_a_wrong_password(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (workspace, credentials) = kyuubi.workspace("SELECT 1", "not-the-password");
    let row = gpui_kit::ElementId::Name(format!("profile-{}", workspace.profiles[0].id).into());
    let app = TestApp::launch_with(cx, workspace, credentials);

    app.update(cx, |window, cx| window.click("run", cx));
    app.wait_until(cx, "the rejected sign-in", QUERY_TIMEOUT, |window, _| {
        window.find(row.clone()).label() == Some("Spark, unread error")
    });
    app.update(cx, |window, _| assert_eq!(cell(window, 0, 1), None));
    assert_eq!(app.credentials.reads(), 1);
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_missing_initial_database_is_named_in_the_status_and_logs(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT 1", crate::support::fixture::PASSWORD);
    workspace.profiles[0].database = "qrow_missing_database".into();
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.update(cx, |window, cx| window.click("run", cx));
    app.wait_status(cx, "Error · Connection failed");
    let logs = app.logs(cx);
    assert!(
        logs.contains("Could not select the initial database \"qrow_missing_database\"."),
        "{logs}"
    );
    assert!(logs.contains("SCHEMA_NOT_FOUND"), "{logs}");
}
