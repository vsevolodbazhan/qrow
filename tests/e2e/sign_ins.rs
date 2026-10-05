//! Browser sign-in in the real window, then SQL over TLS with the access
//! token, against the fixture provider and Kyuubi.
use crate::support::fixture::Kyuubi;
use crate::support::oidc::FixtureProvider;
use crate::support::{MemoryCredentials, SignIns, TestApp};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::model::{Authentication, SavedTab, Workspace};
use std::sync::Arc;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_browser_sign_in_runs_sql_as_the_connection_user_over_tls(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let fixture = FixtureProvider::get();
    let sign_in = fixture.sign_in("Fixture");
    let mut profile = kyuubi.profile("Analytics");
    profile.port = fixture.tls_port;
    profile.tls = true;
    profile.authentication = Authentication::Oidc {
        sign_in: sign_in.id,
    };
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = "SELECT current_user() AS account".into();
    let workspace = Workspace {
        profiles: vec![profile],
        tabs: vec![tab],
        sign_ins: vec![sign_in.clone()],
        ..Workspace::default()
    };
    let browser = fixture.browser("alice", &[]);
    let app = TestApp::launch_with_sign_ins(
        cx,
        workspace,
        MemoryCredentials::default(),
        SignIns::new(fixture.trust.clone(), Some(Arc::new(browser))),
    );

    // Run opens the browser sign-in first, then runs the query.
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "qrow");
    assert_eq!(app.credentials.reads(), 0, "no password was read");
    let logs = app.logs(cx);
    assert!(
        !logs.contains("access_token") && !logs.contains("eyJ"),
        "{logs}"
    );

    app.click(cx, "show-sign-ins");
    app.click(cx, format!("sign-in-{}", sign_in.id));
    app.wait_for(cx, "sign-in-account-sign-out");
    app.update(cx, |window, _| {
        let status = window.find("sign-in-account-status");
        assert_eq!(status.label(), Some("Signed in as alice@qrow.test."));
        let status = status.bounds();
        let sign_out = window.find("sign-in-account-sign-out").bounds();
        assert!(status.right() < sign_out.left());
        assert!(f32::from(status.center().y - sign_out.center().y).abs() < 1.);
    });
    app.click(cx, "sign-in-account-sign-out");
    app.wait_until(
        cx,
        "the signed-out account",
        std::time::Duration::from_secs(20),
        |window, _| {
            crate::support::label(window, "sign-in-account-status").as_deref()
                == Some("Not signed in.")
        },
    );
}
