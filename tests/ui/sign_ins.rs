//! The Sign-ins sidebar, Sign-in Settings, the sidebar buttons of the status
//! bar, and sign-in authentication in Connection Settings, against a mock
//! OpenID Connect provider in the test process.
use crate::support::{
    MemoryCredentials, SignIns, TestApp, label, offline_profile,
    oidc::{Provider, subject, trust},
    present, value,
};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::{
    model::{Authentication, SavedTab, SignIn, Workspace},
    storage::TokenStore,
    ui::{ShowConnections, ShowSignIns, ToggleSidebar},
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

/// Shows the Sign-ins sidebar, like a click on its status bar button.
fn open_sign_ins(app: &TestApp, cx: &mut TestAppContext) {
    app.click(cx, "show-sign-ins");
    app.wait_for(cx, "add-sign-in");
}

/// The second line of the sidebar row of `sign_in`.
fn row_account(sign_in: &SignIn) -> String {
    format!("sign-in-{}-account", sign_in.id)
}

/// Waits until the sidebar row of `sign_in` shows `account`.
fn wait_row(app: &TestApp, cx: &mut TestAppContext, sign_in: &SignIn, account: &str) {
    let id = row_account(sign_in);
    app.wait_until(cx, account, WAIT, |window, _| {
        label(window, id.clone()).as_deref() == Some(account)
    });
}

/// Opens Sign-in Settings of `sign_in` from its sidebar row.
fn open_settings(app: &TestApp, cx: &mut TestAppContext, sign_in: &SignIn) {
    app.click(cx, format!("sign-in-{}", sign_in.id));
    app.wait_for(cx, "sign-in-name");
}

/// Chooses `option` in the Sign-in list of Connection Settings.
fn choose_sign_in(app: &TestApp, cx: &mut TestAppContext, option: &str) {
    app.click(cx, "connection-sign-in");
    app.settle(cx);
    app.update(cx, |window, cx| window.input(option, cx));
    app.settle(cx);
    app.press(cx, "enter");
    app.wait_until(cx, option, WAIT, |window, _| {
        value(window, "connection-sign-in").as_deref() == Some(option)
    });
    // Enter chooses the row and keeps the list open. Escape closes it.
    if app.update(cx, |window, _| {
        window.try_find("connection-new-sign-in").is_some()
    }) {
        app.press(cx, "escape");
    }
    app.wait_gone(cx, "connection-new-sign-in");
    app.wait_for(cx, "connection-name");
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
fn the_status_bar_buttons_choose_and_hide_the_sidebar_panel(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, _) = workspace(&provider, false);
    let app = launch(cx, &provider, workspace);
    let shows_panel = |app: &TestApp, cx: &mut TestAppContext| {
        app.update(cx, |window, _| {
            (
                present(window, &"add-connection".into()),
                present(window, &"add-sign-in".into()),
            )
        })
    };
    app.wait_for(cx, "add-connection");
    assert_eq!(shows_panel(&app, cx), (true, false));
    open_sign_ins(&app, cx);
    assert_eq!(shows_panel(&app, cx), (false, true));
    // The button of the visible panel hides the sidebar.
    app.click(cx, "show-sign-ins");
    app.wait_gone(cx, "add-sign-in");
    assert_eq!(shows_panel(&app, cx), (false, false));
    // ⌘B shows the panel that was visible last.
    app.dispatch(cx, ToggleSidebar);
    app.wait_for(cx, "add-sign-in");
    app.dispatch(cx, ShowConnections);
    app.wait_for(cx, "add-connection");
    assert_eq!(shows_panel(&app, cx), (true, false));
    app.dispatch(cx, ShowConnections);
    app.wait_gone(cx, "add-connection");
    app.dispatch(cx, ShowSignIns);
    app.wait_for(cx, "add-sign-in");
    // The tab strip has no sidebar or assistant toggles.
    app.update(cx, |window, _| {
        assert!(!present(window, &"sidebar-toggle".into()));
    });
}

#[gpui_kit::test]
fn the_sign_ins_button_counts_the_sign_ins_that_need_attention(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, true);
    let app = launch(cx, &provider, workspace);
    // A connection uses the sign-in, and nobody signed in.
    app.wait_until(cx, "the attention count", WAIT, |window, _| {
        label(window, "show-sign-ins").as_deref() == Some("Sign-ins, 1 sign-in needs attention")
    });
    open_sign_ins(&app, cx);
    wait_row(&app, cx, &sign_in, "Not signed in");
    app.click(cx, format!("sign-in-{}-sign-in", sign_in.id));
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
    app.wait_until(cx, "no attention count", WAIT, |window, _| {
        label(window, "show-sign-ins").as_deref() == Some("Sign-ins")
    });
}

#[gpui_kit::test]
fn a_new_sign_in_signs_in_with_the_browser_and_signs_out(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let app = launch(cx, &provider, Workspace::default());
    open_sign_ins(&app, cx);
    app.wait_for(cx, "sign-ins-empty");
    app.click(cx, "add-sign-in");
    app.wait_for(cx, "sign-in-name");

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
    app.wait_gone(cx, "sign-in-name");
    app.wait_until(cx, "the saved sign-in", WAIT, |_, _| {
        app.saved().sign_ins.len() == 1
    });
    let sign_in = app.saved().sign_ins[0].clone();
    wait_row(&app, cx, &sign_in, "Not signed in");
    assert_eq!(sign_in.allowed_hosts, vec!["127.0.0.1"]);
    assert_eq!(sign_in.scopes, vec!["kyuubi"]);

    app.click(cx, format!("sign-in-{}-sign-in", sign_in.id));
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
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

    // Sign-in Settings shows the account, and signs out.
    open_settings(&app, cx, &sign_in);
    app.wait_until(cx, "the account", WAIT, |window, _| {
        label(window, "sign-in-account-status").as_deref() == Some("Signed in as alice@qrow.test.")
    });
    app.click(cx, "sign-in-account-sign-out");
    app.wait_until(cx, "the signed-out account", WAIT, |window, _| {
        label(window, "sign-in-account-status").as_deref() == Some("Not signed in.")
    });
    assert!(tokens.load_tokens(sign_in.id).unwrap().is_none());
    app.click(cx, "cancel-sign-in-editor");
    app.wait_gone(cx, "sign-in-name");
    wait_row(&app, cx, &sign_in, "Not signed in");
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
    wait_row(&app, cx, &sign_in, "The last action failed");
    // The failure needs the user, so the status bar counts it.
    app.wait_until(cx, "the attention count", WAIT, |window, _| {
        label(window, "show-sign-ins").as_deref() == Some("Sign-ins, 1 sign-in needs attention")
    });
    open_settings(&app, cx, &sign_in);
    app.wait_until(cx, "the reason", WAIT, |window, _| {
        label(window, "sign-in-account-error")
            .is_some_and(|error| error.contains("The provider denied the sign-in"))
    });
    app.update(cx, |window, _| {
        assert!(window.find("sign-in-account-sign-in").label().is_some());
    });
}

#[gpui_kit::test]
fn a_sign_in_that_connections_use_cannot_be_deleted(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, true);
    let app = launch(cx, &provider, workspace);
    open_sign_ins(&app, cx);
    open_settings(&app, cx, &sign_in);
    app.wait_until(cx, "the connections that use it", WAIT, |window, _| {
        label(window, "sign-in-connections")
            .is_some_and(|text| text.starts_with("Used by Analytics."))
    });
    app.click(cx, "cancel-sign-in-editor");
    app.wait_gone(cx, "sign-in-name");
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Delete");
    app.settle(cx);
    assert!(!app.update(cx, |window, _| present(
        window,
        &"confirm-delete-sign-in".into()
    )));
    assert_eq!(app.saved().sign_ins.len(), 1);
}

#[gpui_kit::test]
fn an_unused_sign_in_can_be_deleted(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, false);
    let app = launch(cx, &provider, workspace);
    open_sign_ins(&app, cx);
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Delete");
    // Delete asks first, and Cancel keeps the sign-in.
    app.click(cx, "cancel-delete-sign-in");
    app.wait_gone(cx, "cancel-delete-sign-in");
    assert_eq!(app.saved().sign_ins.len(), 1);
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Delete");
    app.click(cx, "confirm-delete-sign-in");
    app.wait_for(cx, "sign-ins-empty");
    app.wait_until(cx, "the removal", WAIT, |_, _| {
        app.saved().sign_ins.is_empty()
    });
}

#[gpui_kit::test]
fn connection_settings_adds_a_sign_in_and_chooses_it(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let app = launch(cx, &provider, Workspace::default());
    app.click(cx, "add-connection");
    app.wait_for(cx, "connection-name");
    app.fill(cx, "connection-name", "Analytics XL");
    app.fill(cx, "connection-host", "127.0.0.1");
    app.fill(cx, "connection-port", "10009");
    app.fill(cx, "connection-username", "kyuubi-analytics-xl");
    app.select(cx, "connection-authentication", "Sign-in (OpenID Connect)");
    app.wait_for(cx, "connection-sign-in");
    app.click(cx, "connection-sign-in");
    app.click(cx, "connection-new-sign-in");
    // Sign-in Settings opens above Connection Settings.
    app.wait_for(cx, "sign-in-name");
    type_fields(
        &app,
        cx,
        &[
            "Company",
            &provider.issuer,
            "qrow-desktop",
            "",
            "",
            "127.0.0.1",
        ],
    );
    app.click(cx, "save-sign-in-editor");
    app.wait_gone(cx, "sign-in-name");
    app.wait_until(cx, "the new choice", WAIT, |window, _| {
        value(window, "connection-sign-in").as_deref() == Some("Company")
    });
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "the saved connection", WAIT, |_, _| {
        app.saved()
            .profiles
            .iter()
            .any(|profile| profile.name == "Analytics XL")
    });
    let saved = app.saved();
    let profile = saved
        .profiles
        .iter()
        .find(|profile| profile.name == "Analytics XL")
        .unwrap();
    assert_eq!(
        profile.authentication,
        Authentication::Oidc {
            sign_in: saved.sign_ins[0].id
        }
    );
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
    // A sign-in does not turn on TLS. The form warns about the token instead.
    app.wait_for(cx, "connection-tls");
    app.click(cx, "save-profile");
    app.wait_until(cx, "the missing sign-in", WAIT, |window, _| {
        label(window, "connection-form-error-accessibility")
            .is_some_and(|error| error.contains("Choose a sign-in"))
    });
    choose_sign_in(&app, cx, "Company");
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
    assert!(!profile.tls);
    assert_eq!(profile.username, "kyuubi-analytics-xl");
    assert_eq!(app.credentials.count(), 0, "no password was saved");
}

/// A workspace whose connection uses the sign-in and a server address
/// without a server, so that a query that starts fails to connect.
fn unreachable_workspace(provider: &Provider) -> (Workspace, SignIn) {
    let (mut workspace, sign_in) = workspace(provider, true);
    workspace.tabs[0].sql = "SELECT 1".into();
    workspace.profiles[0].host = "127.0.0.1".into();
    workspace.profiles[0].port = 1;
    (workspace, sign_in)
}

fn wait_query_status(app: &TestApp, cx: &mut TestAppContext, prefix: &str) {
    app.wait_until(cx, prefix, WAIT, |window, _| {
        label(window, "query-status").is_some_and(|status| status.starts_with(prefix))
    });
}

#[gpui_kit::test]
fn running_without_a_sign_in_signs_in_with_the_browser_and_then_runs(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = unreachable_workspace(&provider);
    let app = launch(cx, &provider, workspace);
    app.click(cx, "run");
    // The browser sign-in runs first. The query then opens a session, which
    // fails here because no server listens.
    wait_query_status(&app, cx, "Error: Connection failed");
    assert_eq!(provider.authorization_grants(), 1);
    app.wait_until(cx, "the saved identity", WAIT, |_, _| {
        app.saved().sign_ins[0]
            .identity
            .as_ref()
            .is_some_and(|identity| identity.subject == subject("alice"))
    });
    open_sign_ins(&app, cx);
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
    app.wait_until(cx, "the saved SQL", WAIT, |_, _| {
        app.saved().tabs[0].sql == "SELECT 1"
    });
}

#[gpui_kit::test]
fn an_expired_sign_in_opens_the_browser_and_runs_the_query_after_it(cx: &mut TestAppContext) {
    let provider = Provider::start();
    // Each session needs a refreshed token.
    provider.set_access_ttl(10);
    let (workspace, sign_in) = unreachable_workspace(&provider);
    let app = launch(cx, &provider, workspace);
    open_sign_ins(&app, cx);
    app.click(cx, format!("sign-in-{}-sign-in", sign_in.id));
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
    // The provider no longer accepts the refresh token, as after a long
    // break. The sign-in looks valid until a query needs a token.
    provider.revoke("alice");
    app.click(cx, "run");
    wait_query_status(&app, cx, "Error: Connection failed");
    assert_eq!(provider.authorization_grants(), 2);
    // The sidebar still shows the Sign-ins panel.
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
}

#[gpui_kit::test]
fn a_denied_sign_in_ends_the_waiting_query_with_the_reason(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, _) = unreachable_workspace(&provider);
    let app = TestApp::launch_with_sign_ins(
        cx,
        workspace,
        MemoryCredentials::default(),
        SignIns::new(trust(), Some(Arc::new(provider.denying_browser()))),
    );
    app.click(cx, "run");
    wait_query_status(&app, cx, "Error: Sign-in required");
    app.click(cx, "output-copy-error");
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap();
    assert!(
        copied.contains("The sign-in failed, so the query did not run"),
        "{copied}"
    );
}

#[gpui_kit::test]
fn cancel_ends_a_query_that_waits_for_the_browser(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = unreachable_workspace(&provider);
    // A browser in which nobody finishes the sign-in.
    let app = TestApp::launch_with_sign_ins(
        cx,
        workspace,
        MemoryCredentials::default(),
        SignIns::new(trust(), Some(Arc::new(|_: &str| Ok(())))),
    );
    app.click(cx, "run");
    wait_query_status(&app, cx, "Waiting for sign-in");
    app.click(cx, "cancel");
    wait_query_status(&app, cx, "Cancelled: Sign-in not finished");
    // The browser sign-in continues, and the sidebar can cancel it.
    open_sign_ins(&app, cx);
    wait_row(&app, cx, &sign_in, "Waiting for the browser…");
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

#[gpui_kit::test]
fn the_pointer_moves_between_the_fields_of_the_sign_in_form(cx: &mut TestAppContext) {
    use gpui_kit::{InputEvent as _, MouseButton, MouseDownEvent, MouseUpEvent, point};
    let provider = Provider::start();
    let app = launch(cx, &provider, Workspace::default());
    open_sign_ins(&app, cx);
    app.click(cx, "add-sign-in");
    app.wait_for(cx, "sign-in-name");
    // Fields take the width of the dialog, so a long issuer URL stays readable.
    app.update(cx, |window, _| {
        let width = f32::from(window.find("sign-in-issuer").bounds().size.width);
        assert!(width > 400., "the issuer field is {width} px wide");
    });
    let mut typed = std::collections::HashMap::<&str, String>::new();
    // The name field has focus when the form opens. Each click moves the
    // focus, also back to the name field, at the center or near an edge.
    for (field, across, text) in [
        ("sign-in-issuer", 0.5, "i"),
        ("sign-in-name", 0.5, "n"),
        ("sign-in-client-id", 0.9, "c"),
        ("sign-in-name", 0.05, "m"),
    ] {
        app.update(cx, |window, cx| {
            let bounds = window.find(field).bounds();
            let position = point(
                bounds.origin.x + bounds.size.width * across,
                bounds.origin.y + bounds.size.height / 2.,
            );
            let modifiers = Default::default();
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            window.input(text, cx);
        });
        app.settle(cx);
        typed.entry(field).or_default().push_str(text);
        app.update(cx, |window, _| {
            for (field, expected) in &typed {
                assert_eq!(
                    window.find(*field).value(),
                    Some(expected.as_str()),
                    "{field}"
                );
            }
        });
    }
}
