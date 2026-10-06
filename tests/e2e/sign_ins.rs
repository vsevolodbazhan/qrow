//! Browser sign-in in the real window, then SQL over TLS with the access
//! token, against the fixture provider and Kyuubi.
use crate::support::fixture::Kyuubi;
use crate::support::oidc::FixtureProvider;
use crate::support::{MemoryCredentials, SignIns, TestApp};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::model::{Authentication, SavedTab, Workspace};
use std::{sync::Arc, time::Duration};

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
    let profile_row = format!("sign-in-connection-{}", profile.id);
    let mut second = profile.clone();
    second.id = uuid::Uuid::new_v4();
    second.name = "Analytics M".into();
    let second_row = format!("sign-in-connection-{}", second.id);
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = "SELECT current_user() AS account".into();
    let workspace = Workspace {
        profiles: vec![profile, second],
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
    app.scroll_to(cx, "sign-in-connections");
    app.update(cx, |window, _| {
        assert_eq!(
            crate::support::label(window, "sign-in-connections-count").as_deref(),
            Some("2 connections")
        );
        assert_eq!(window.find(profile_row.clone()).label(), Some("Analytics"));
        assert!(window.find(profile_row.clone()).visible());
        assert_eq!(window.find(second_row.clone()).label(), Some("Analytics M"));
        let first = window.find(profile_row.clone()).bounds();
        let second = window.find(second_row.clone()).bounds();
        assert_eq!(first.top(), second.top());
        assert!(first.right() < second.left());
    });
    app.scroll_to(cx, "sign-in-account-sign-out");
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
    app.click(cx, "cancel-sign-in-editor");
    app.wait_gone(cx, "sign-in-name");
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.hover_labelled(cx, "Delete");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(800));
    app.wait_for(cx, "tooltip");
    app.update(cx, |window, _| {
        assert_eq!(
            crate::support::label(window, "popup-menu-tooltip-text").as_deref(),
            Some("Change authentication in the connections first.")
        );
    });
    app.choose(cx, "popup-menu", "Delete");
    app.settle(cx);
    assert_eq!(app.saved().sign_ins.len(), 1);
    app.press(cx, "escape");
    app.wait_gone(cx, "popup-menu");
    app.wait_gone(cx, "tooltip");
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_pasted_sign_in_runs_sql_in_a_connection_of_a_colleague(cx: &mut TestAppContext) {
    use crate::support::{connection_row, value};
    use qrow::ui::ShowConnections;
    let kyuubi = Kyuubi::get();
    let fixture = FixtureProvider::get();
    // A colleague copied this sign-in and sent it in a chat message.
    let shared = fixture.sign_in("Team").to_shared_text();
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT current_user() AS account", "not-used");
    let connection = workspace.profiles[0].id;
    workspace.profiles[0].name = "Analytics".into();
    let browser = fixture.browser("alice", &[]);
    let app = TestApp::launch_with_sign_ins(
        cx,
        workspace,
        credentials,
        SignIns::new(fixture.trust.clone(), Some(Arc::new(browser))),
    );
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(format!(
        "Use this sign-in:\n{shared}"
    )));
    app.click(cx, "show-sign-ins");
    app.click(cx, "paste-first-sign-in");
    app.wait_for(cx, "sign-in-pasted-note");
    app.click(cx, "save-sign-in-editor");
    app.wait_gone(cx, "sign-in-name");
    app.wait_until(cx, "the pasted sign-in", Duration::from_secs(20), |_, _| {
        app.saved().sign_ins.len() == 1
    });

    // The connection uses the pasted sign-in over TLS.
    app.dispatch(cx, ShowConnections);
    app.context_menu(cx, connection_row(connection));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    let port = fixture.tls_port.to_string();
    app.update(cx, |window, cx| {
        window.click("connection-port", cx);
        window.press("cmd-a", cx);
        window.input(&port, cx);
    });
    app.click(cx, "connection-tls");
    app.select(cx, "connection-authentication", "Sign-in (OpenID Connect)");
    app.wait_for(cx, "connection-sign-in");
    app.click(cx, "connection-sign-in");
    app.settle(cx);
    app.update(cx, |window, cx| window.input("Team", cx));
    app.settle(cx);
    app.press(cx, "enter");
    app.wait_until(
        cx,
        "the chosen sign-in",
        Duration::from_secs(20),
        |window, _| value(window, "connection-sign-in").as_deref() == Some("Team"),
    );
    if app.update(cx, |window, _| {
        window.try_find("connection-new-sign-in").is_some()
    }) {
        app.press(cx, "escape");
    }
    app.wait_gone(cx, "connection-new-sign-in");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    let sign_in = app.saved().sign_ins[0].id;
    app.wait_until(
        cx,
        "the saved connection",
        Duration::from_secs(20),
        |_, _| {
            let saved = app.saved();
            let profile = &saved.profiles[0];
            profile.tls && profile.authentication == Authentication::Oidc { sign_in }
        },
    );

    // Run opens the browser sign-in first, then runs the query.
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    app.wait_cell(cx, 0, 1, "qrow");
}
