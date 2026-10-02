//! Settings > Sign-ins and sign-in authentication in Connection Settings,
//! against a mock OpenID Connect provider in the test process.
use crate::support::{
    MemoryCredentials, SignIns, TestApp, label, offline_profile,
    oidc::{Provider, subject, trust},
    shows,
};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::{
    model::{Authentication, SavedTab, SignIn, Workspace},
    storage::TokenStore,
    ui::OpenSignIns,
};
use std::{sync::Arc, time::Duration};

const WAIT: Duration = Duration::from_secs(20);

fn launch(cx: &mut TestAppContext, provider: &Provider, workspace: Workspace) -> TestApp {
    let browser = provider.browser("alice");
    TestApp::launch_with_sign_ins(
        cx,
        workspace,
        MemoryCredentials::default(),
        SignIns::new(trust(), Some(Arc::new(browser))),
    )
}

fn open_sign_ins(app: &TestApp, cx: &mut TestAppContext) {
    app.dispatch(cx, OpenSignIns);
    app.wait_for(cx, "save-settings");
}

fn close_settings(app: &TestApp, cx: &mut TestAppContext) {
    app.click(cx, "save-settings");
    app.wait_gone(cx, "save-settings");
}

fn wait_text(app: &TestApp, cx: &mut TestAppContext, text: &str) {
    app.wait_until(cx, text, WAIT, |window, _| shows(window, text));
}

/// Types `values` into the fields of the sign-in form from the focused
/// field on, and moves between fields with Tab like a keyboard user.
fn type_fields(app: &TestApp, cx: &mut TestAppContext, values: &[&str]) {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            app.press(cx, "tab");
        }
        app.update(cx, |window, cx| {
            window.press("cmd-a", cx);
            if value.is_empty() {
                window.press("backspace", cx);
            } else {
                window.input(value, cx);
            }
        });
        app.settle(cx);
    }
}

/// Waits until the account row of `sign_in` shows `status`.
fn wait_status(app: &TestApp, cx: &mut TestAppContext, sign_in: &SignIn, status: &str) {
    let id = format!("sign-in-{}-account-description", sign_in.id);
    app.wait_until(cx, status, WAIT, |window, _| {
        label(window, id.clone()).as_deref() == Some(status)
    });
}

/// A workspace with one sign-in for the provider and one offline connection.
fn workspace(provider: &Provider, uses_sign_in: bool) -> (Workspace, SignIn) {
    let sign_in = provider.sign_in("Company");
    let mut profile = offline_profile("Analytics");
    if uses_sign_in {
        profile.tls = true;
        profile.authentication = Authentication::Oidc {
            sign_in: sign_in.id,
        };
    }
    let tab = SavedTab::new(1, Some(profile.id));
    (
        Workspace {
            profiles: vec![profile],
            tabs: vec![tab],
            sign_ins: vec![sign_in.clone()],
            ..Workspace::default()
        },
        sign_in,
    )
}

#[gpui_kit::test]
fn a_new_sign_in_signs_in_with_the_browser_and_signs_out(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let app = launch(cx, &provider, Workspace::default());
    open_sign_ins(&app, cx);
    app.click(cx, "add-sign-in");
    app.wait_for(cx, "setting-sign-in-name");

    // Name, issuer, client ID, scopes, resource, and database hosts.
    type_fields(
        &app,
        cx,
        &[
            "Company",
            "http://insecure.example.test",
            "qrow-desktop",
            "kyuubi",
            "",
            "127.0.0.1",
        ],
    );
    // Enter in a field saves the form. The form keeps the input and reports
    // the field that is wrong.
    app.press(cx, "enter");
    app.wait_until(cx, "the issuer error", WAIT, |window, _| {
        label(window, "sign-in-form-error").is_some_and(|error| error.contains("HTTPS URL"))
    });
    for _ in 0..4 {
        app.press(cx, "shift-tab");
    }
    type_fields(&app, cx, &[&provider.issuer]);
    app.press(cx, "enter");
    app.wait_gone(cx, "setting-sign-in-name");
    app.wait_until(cx, "the saved sign-in", WAIT, |_, _| {
        app.saved().sign_ins.len() == 1
    });
    let sign_in = app.saved().sign_ins[0].clone();
    wait_status(&app, cx, &sign_in, "Not signed in");
    assert_eq!(sign_in.allowed_hosts, vec!["127.0.0.1"]);
    assert_eq!(sign_in.scopes, vec!["kyuubi"]);

    app.click(cx, format!("sign-in-{}-sign-in", sign_in.id));
    wait_status(&app, cx, &sign_in, "Signed in as alice@qrow.test");
    close_settings(&app, cx);
    app.wait_until(cx, "the saved identity", WAIT, |_, _| {
        app.saved().sign_ins[0]
            .identity
            .as_ref()
            .is_some_and(|identity| identity.subject == subject("alice"))
    });
    let tokens = app.sign_ins.as_ref().unwrap().tokens.clone();
    let record = tokens.load_tokens(sign_in.id).unwrap().unwrap();
    assert!(record.contains(&subject("alice")));
    // Tokens stay out of the workspace file.
    let file = std::fs::read_to_string(app.workspace_path()).unwrap();
    assert!(!file.contains("access-alice"));
    assert!(!file.contains("refresh-alice"));

    open_sign_ins(&app, cx);
    app.click(cx, format!("sign-in-{}-sign-out", sign_in.id));
    wait_status(&app, cx, &sign_in, "Not signed in");
    assert!(tokens.load_tokens(sign_in.id).unwrap().is_none());
    close_settings(&app, cx);
    app.wait_until(cx, "the cleared identity", WAIT, |_, _| {
        app.saved().sign_ins[0].identity.is_none()
    });
}

#[gpui_kit::test]
fn a_failed_sign_in_shows_the_reason_and_can_be_retried(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, false);
    let app = TestApp::launch_with_sign_ins(
        cx,
        workspace,
        MemoryCredentials::default(),
        SignIns::new(trust(), Some(Arc::new(provider.denying_browser()))),
    );
    open_sign_ins(&app, cx);
    app.click(cx, format!("sign-in-{}-sign-in", sign_in.id));
    wait_text(&app, cx, "The provider denied the sign-in");
    wait_text(&app, cx, "Not signed in");
    app.update(cx, |window, _| {
        assert!(
            window
                .find(format!("sign-in-{}-sign-in", sign_in.id))
                .label()
                .is_some()
        );
    });
}

#[gpui_kit::test]
fn a_sign_in_that_connections_use_cannot_be_removed(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, true);
    let app = launch(cx, &provider, workspace);
    open_sign_ins(&app, cx);
    let connections = format!("sign-in-{}-connections-description", sign_in.id);
    app.wait_until(cx, "the connections that use it", WAIT, |window, _| {
        label(window, connections.clone())
            .is_some_and(|text| text.starts_with("Used by Analytics."))
    });
    app.click(cx, format!("sign-in-{}-remove", sign_in.id));
    app.settle(cx);
    assert_eq!(app.saved().sign_ins.len(), 1);
}

#[gpui_kit::test]
fn an_unused_sign_in_can_be_removed(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, false);
    let app = launch(cx, &provider, workspace);
    open_sign_ins(&app, cx);
    app.click(cx, format!("sign-in-{}-remove", sign_in.id));
    app.wait_until(cx, "the empty page", WAIT, |window, _| {
        label(window, "sign-ins-add-description")
            .is_some_and(|text| text.starts_with("A sign-in lets connections"))
    });
    close_settings(&app, cx);
    app.wait_until(cx, "the removal", WAIT, |_, _| {
        app.saved().sign_ins.is_empty()
    });
}

#[gpui_kit::test]
fn a_connection_can_use_a_sign_in_with_its_own_username(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, false);
    let app = launch(cx, &provider, workspace);
    app.click(cx, "add-connection");
    app.wait_for(cx, "connection-name");
    app.fill(cx, "connection-name", "Analytics XL");
    app.fill(cx, "connection-host", "kyuubi.example.test");
    app.fill(cx, "connection-port", "10009");
    app.fill(cx, "connection-username", "kyuubi-analytics-xl");
    app.select(cx, "connection-authentication", "Sign-in (OpenID Connect)");
    app.wait_for(cx, "connection-sign-in");
    app.update(cx, |window, _| {
        assert!(!crate::support::present(
            window,
            &"connection-password".into()
        ));
    });
    app.click(cx, "save-profile");
    app.wait_until(cx, "the missing sign-in", WAIT, |window, _| {
        label(window, "connection-form-error-accessibility")
            .is_some_and(|error| error.contains("Choose a sign-in"))
    });
    app.select(cx, "connection-sign-in", "Company");
    app.click(cx, "save-profile");
    // The sign-in does not send tokens to this host.
    app.wait_until(cx, "the host error", WAIT, |window, _| {
        label(window, "connection-form-error-accessibility")
            .is_some_and(|error| error.contains("does not send tokens to kyuubi.example.test"))
    });
    app.fill(cx, "connection-host", "127.0.0.1");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    let saved = || {
        app.saved()
            .profiles
            .into_iter()
            .find(|profile| profile.name == "Analytics XL")
    };
    app.wait_until(cx, "the saved connection", WAIT, |_, _| saved().is_some());
    let profile = saved().unwrap();
    assert_eq!(
        profile.authentication,
        Authentication::Oidc {
            sign_in: sign_in.id
        }
    );
    assert!(profile.tls);
    assert_eq!(profile.username, "kyuubi-analytics-xl");
    assert_eq!(app.credentials.count(), 0, "no password was saved");
}

#[gpui_kit::test]
fn running_without_a_sign_in_explains_the_recovery_and_keeps_the_sql(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (mut workspace, _) = workspace(&provider, true);
    workspace.tabs[0].sql = "SELECT 1".into();
    workspace.profiles[0].host = "127.0.0.1".into();
    let app = launch(cx, &provider, workspace);
    app.click(cx, "run");
    app.wait_until(cx, "the sign-in error", WAIT, |window, _| {
        label(window, "query-status")
            .is_some_and(|status| status.starts_with("Error · Sign-in required"))
    });
    app.click(cx, "output-copy-error");
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap();
    assert!(
        copied.contains("Sign in to \"Company\" in Settings > Sign-ins"),
        "{copied}"
    );
    app.wait_until(cx, "the saved SQL", WAIT, |_, _| {
        app.saved().tabs[0].sql == "SELECT 1"
    });
    assert_eq!(provider.authorization_grants(), 0, "no browser opened");
}

#[gpui_kit::test]
fn the_lifecycle_fields_stay_reachable_below_the_authentication_fields(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, _) = workspace(&provider, false);
    let app = launch(cx, &provider, workspace);
    app.click(cx, "add-connection");
    app.wait_for(cx, "connection-name");
    app.scroll_to(cx, "connection-idle-behavior");
    app.select(cx, "connection-idle-behavior", "Keep connected");
    app.wait_for(cx, "connection-keep-alive-query");
}
