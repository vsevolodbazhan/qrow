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
        window.find(row.clone()).label() == Some("Spark")
    });
    app.wait_status(cx, "Error · Connection lost");
    app.update(cx, |window, _| assert_eq!(cell(window, 0, 1), None));
    assert_eq!(app.credentials.reads(), 1);
}
