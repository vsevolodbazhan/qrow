//! The Sign-ins sidebar, Sign-in Settings, the sidebar buttons of the status
//! bar, and sign-in authentication in Connection Settings, against a mock
//! OpenID Connect provider in the test process.
use crate::support::{
    MemoryCredentials, SignIns, TestApp, assert_sign_in_dot,
    assistant::editor_text,
    click_element, find_in, label, offline_profile,
    oidc::{Provider, subject, trust},
    present, value,
};
use gpui_kit::TestAppContext;
use gpui_kit::component::ActiveTheme;
use gpui_kit::test::TestWindowExt;
use qrow::{
    model::{Authentication, SavedTab, SignIn, Workspace},
    storage::{MemoryTokenStore, TokenStore},
    ui::{ShowConnections, ShowSignIns, ToggleActivity, ToggleSidebar},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

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
    app.wait_until(cx, account, WAIT, |window, _| {
        label(window, format!("sign-in-{}", sign_in.id))
            .is_some_and(|label| label.ends_with(account))
    });
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, row_account(sign_in)).as_deref(),
            Some(
                if sign_in.provider == qrow::model::SignInProvider::TrinoExternal {
                    "Trino"
                } else {
                    "OIDC"
                }
            )
        );
        assert!(
            window
                .try_find(format!("sign-in-{}-sign-in", sign_in.id))
                .is_none()
        );
    });
}

/// Opens Sign-in Settings of `sign_in` from its sidebar row.
fn open_settings(app: &TestApp, cx: &mut TestAppContext, sign_in: &SignIn) {
    app.click(cx, format!("sign-in-{}", sign_in.id));
    app.wait_for(cx, "sign-in-name");
}

/// Chooses `option` in the Sign-in list of Connection Settings.
fn choose_sign_in(app: &TestApp, cx: &mut TestAppContext, option: &str) {
    app.scroll_to(cx, "connection-sign-in");
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
fn command_b_hides_and_shows_the_connections_from_the_sidebar(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, _) = workspace(&provider, false);
    let app = launch(cx, &provider, workspace);
    app.wait_for(cx, "add-connection");
    // The focus is in the sidebar when it closes.
    app.click(cx, "connections-list");
    app.press(cx, "cmd-b");
    app.wait_gone(cx, "add-connection");
    app.press(cx, "cmd-b");
    app.wait_for(cx, "add-connection");
    // From the Sign-ins sidebar, ⌘B shows the Connections.
    open_sign_ins(&app, cx);
    app.press(cx, "cmd-b");
    app.wait_for(cx, "add-connection");
}

#[gpui_kit::test]
fn the_sql_editor_takes_the_focus_after_the_focused_result_table_closes(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    app.wait_for(cx, "add-connection");
    let cell = app.update(cx, |window, _| {
        find_in(window, ("row", 1usize), ("cell", 1usize)).expect("a result cell")
    });
    app.update(cx, |window, cx| click_element(window, &cell, cx));
    // The Logs panel takes the place of the focused result table, like after
    // a failed query.
    app.click(cx, "output-panel-tab");
    app.press(cx, "cmd-b");
    app.wait_gone(cx, "add-connection");
    app.press(cx, "cmd-b");
    app.wait_for(cx, "add-connection");
    // The keys go to the SQL editor.
    app.update(cx, |window, cx| {
        window.press("cmd-a", cx);
        window.input("SELECT 2", cx);
    });
    app.wait_until(cx, "the typed SQL", WAIT, |window, _| {
        editor_text(window).as_deref() == Some("SELECT 2")
    });
}

#[gpui_kit::test]
fn command_b_with_activity_open_keeps_the_activity_focus(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, _) = workspace(&provider, false);
    let app = launch(cx, &provider, workspace);
    app.dispatch(cx, ToggleActivity);
    app.wait_for(cx, "activity");
    app.press(cx, "cmd-b");
    // Escape still reaches Activity, so its focus stayed.
    app.press(cx, "escape");
    app.wait_gone(cx, "activity");
}

#[gpui_kit::test]
fn the_sign_ins_button_counts_the_sign_ins_that_need_attention(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, true);
    let app = launch(cx, &provider, workspace);
    // A connection uses the sign-in, and nobody signed in.
    app.wait_until(cx, "the attention count", WAIT, |window, _| {
        label(window, "show-sign-ins").as_deref() == Some("Sign-Ins, 1 sign-in needs attention")
    });
    open_sign_ins(&app, cx);
    wait_row(&app, cx, &sign_in, "Not signed in");
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Sign in…");
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
    app.wait_until(cx, "no attention count", WAIT, |window, _| {
        label(window, "show-sign-ins").as_deref() == Some("Sign-Ins")
    });
}

#[gpui_kit::test]
fn a_new_sign_in_signs_in_with_the_browser_and_signs_out(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let app = launch(cx, &provider, Workspace::default());
    open_sign_ins(&app, cx);
    app.wait_gone(cx, "sign-ins-list");
    app.click(cx, "add-sign-in");
    app.wait_for(cx, "sign-in-name");

    // Name, issuer, client ID, scopes, resource, and callback ports.
    type_fields(
        &app,
        cx,
        &[
            "Company",
            "http://insecure.example.test",
            "qrow-desktop",
            "kyuubi",
            "",
        ],
    );
    // Enter in a field saves the form. The form keeps the input and reports
    // the field that is wrong.
    app.press(cx, "enter");
    app.wait_until(cx, "the issuer error", WAIT, |window, _| {
        label(window, "sign-in-form-error").is_some_and(|error| error.contains("HTTPS URL"))
    });
    for _ in 0..3 {
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
    assert_eq!(sign_in.scopes, vec!["kyuubi"]);

    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Sign in…");
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
    for width in [850., 1280.] {
        cx.simulate_window_resize(
            app.window,
            gpui_kit::size(gpui_kit::px(width), gpui_kit::px(820.)),
        );
        app.settle(cx);
        app.update(cx, |window, _| {
            let status = window.find("sign-in-account-status").bounds();
            let sign_out = window.find("sign-in-account-sign-out").bounds();
            assert!(
                status.right() < sign_out.left(),
                "Sign out follows the status"
            );
            assert!(
                f32::from(status.center().y - sign_out.center().y).abs() < 1.,
                "the account status and Sign Out share a vertical center"
            );
            assert!(window.find("sign-in-account-sign-out").visible());
        });
    }
    // The account must accept a click after scrolling back from the tags.
    cx.simulate_window_resize(
        app.window,
        gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(600.)),
    );
    app.settle(cx);
    app.scroll_to(cx, "sign-in-connections");
    app.scroll_to(cx, "sign-in-account-sign-out");
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
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Sign in…");
    wait_row(&app, cx, &sign_in, "The last action failed");
    // The failure needs the user, so the status bar counts it.
    app.wait_until(cx, "the attention count", WAIT, |window, _| {
        label(window, "show-sign-ins").as_deref() == Some("Sign-Ins, 1 sign-in needs attention")
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
    let (mut workspace, sign_in) = workspace(&provider, true);
    workspace.profiles[0].name = "analytics-s (dev)".into();
    let first = format!("sign-in-connection-{}", workspace.profiles[0].id);
    let mut second = offline_profile("analytics-m (dev)");
    second.authentication = Authentication::Oidc {
        sign_in: sign_in.id,
    };
    let second_id = format!("sign-in-connection-{}", second.id);
    let unrelated = offline_profile("Password connection");
    let unrelated_id = format!("sign-in-connection-{}", unrelated.id);
    workspace.profiles.extend([second, unrelated]);
    let app = launch(cx, &provider, workspace);
    open_sign_ins(&app, cx);
    open_settings(&app, cx, &sign_in);
    app.scroll_to(cx, "sign-in-connections");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "sign-in-connections-count").as_deref(),
            Some("2 connections")
        );
        assert_eq!(
            label(window, first.clone()).as_deref(),
            Some("analytics-s (dev)")
        );
        assert_eq!(
            label(window, second_id.clone()).as_deref(),
            Some("analytics-m (dev)")
        );
        let first = window.find(first.clone()).bounds();
        let second = window.find(second_id.clone()).bounds();
        assert_eq!(first.top(), second.top());
        assert!(
            first.right() < second.left(),
            "short connection tags share one line"
        );
        assert!(window.find(second_id.clone()).visible());
        assert!(window.try_find(unrelated_id.clone()).is_none());
        assert!(window.try_find("sign-in-connections-empty").is_none());
    });
    app.click(cx, "cancel-sign-in-editor");
    app.wait_gone(cx, "sign-in-name");
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.update(cx, |window, _| {
        assert!(window.try_find("sign-in-delete-reason").is_none());
    });
    app.hover_labelled(cx, "Delete");
    cx.executor().advance_clock(Duration::from_millis(800));
    app.wait_for(cx, "tooltip");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "popup-menu-tooltip-text").as_deref(),
            Some("Change authentication in the connections first.")
        );
    });
    app.press(cx, "enter");
    app.wait_gone(cx, "popup-menu");
    assert_eq!(app.saved().sign_ins.len(), 1);
    assert!(!app.update(cx, |window, _| present(
        window,
        &"confirm-delete-sign-in".into()
    )));
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Delete");
    app.settle(cx);
    assert!(!app.update(cx, |window, _| present(
        window,
        &"confirm-delete-sign-in".into()
    )));
    assert_eq!(app.saved().sign_ins.len(), 1);
    app.press(cx, "escape");
    app.wait_gone(cx, "popup-menu");
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.hover_labelled(cx, "Delete");
    cx.executor().advance_clock(Duration::from_millis(800));
    app.wait_for(cx, "tooltip");
    app.press(cx, "escape");
    app.wait_gone(cx, "popup-menu");
    app.wait_gone(cx, "tooltip");
}

#[gpui_kit::test]
fn connection_tags_wrap_inside_the_sign_in_form(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (mut workspace, sign_in) = workspace(&provider, true);
    workspace.profiles[0].name = "analytics-small (development, synthetic shared warehouse)".into();
    let first_id = format!("sign-in-connection-{}", workspace.profiles[0].id);
    let mut second = offline_profile("analytics-medium (development, synthetic shared warehouse)");
    second.authentication = Authentication::Oidc {
        sign_in: sign_in.id,
    };
    let second_id = format!("sign-in-connection-{}", second.id);
    workspace.profiles.push(second);
    let app = launch(cx, &provider, workspace);
    cx.simulate_window_resize(
        app.window,
        gpui_kit::size(gpui_kit::px(850.), gpui_kit::px(820.)),
    );
    open_sign_ins(&app, cx);
    open_settings(&app, cx, &sign_in);
    app.scroll_to(cx, "sign-in-connections");
    app.update(cx, |window, _| {
        let group = window.find("sign-in-connections").bounds();
        let first = window.find(first_id.clone()).bounds();
        let second = window.find(second_id.clone()).bounds();
        assert!(
            first.bottom() < second.top(),
            "long connection tags wrap to another line"
        );
        for bounds in [first, second] {
            assert!(bounds.left() >= group.left() && bounds.right() <= group.right());
        }
        assert!(window.find(second_id.clone()).visible());
    });
}

#[gpui_kit::test]
fn an_unused_sign_in_can_be_deleted(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, false);
    let app = launch(cx, &provider, workspace);
    open_sign_ins(&app, cx);
    open_settings(&app, cx, &sign_in);
    app.scroll_to(cx, "sign-in-connections-empty");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "sign-in-connections-count").as_deref(),
            Some("0 connections")
        );
        assert_eq!(
            label(window, "sign-in-connections-empty").as_deref(),
            Some("No connections use this sign-in.")
        );
    });
    app.click(cx, "cancel-sign-in-editor");
    app.wait_gone(cx, "sign-in-name");
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.update(cx, |window, _| {
        assert!(window.try_find("sign-in-delete-reason").is_none());
    });
    app.hover_labelled(cx, "Delete");
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    assert!(!app.update(cx, |window, _| present(window, &"tooltip".into())));
    app.choose(cx, "popup-menu", "Delete");
    // Delete asks first, and Cancel keeps the sign-in.
    app.click(cx, "cancel-delete-sign-in");
    app.wait_gone(cx, "cancel-delete-sign-in");
    assert_eq!(app.saved().sign_ins.len(), 1);
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Delete");
    app.click(cx, "confirm-delete-sign-in");
    app.wait_gone(cx, "sign-ins-list");
    app.wait_until(cx, "the removal", WAIT, |_, _| {
        app.saved().sign_ins.is_empty()
    });
}

/// Failed removal must preserve both the token record and the saved sign-in.
#[derive(Default)]
struct FailingDeletion {
    tokens: MemoryTokenStore,
    fail: AtomicBool,
}

impl TokenStore for FailingDeletion {
    fn load_tokens(&self, id: uuid::Uuid) -> anyhow::Result<Option<zeroize::Zeroizing<String>>> {
        self.tokens.load_tokens(id)
    }
    fn save_tokens(&self, id: uuid::Uuid, record: &str) -> anyhow::Result<()> {
        self.tokens.save_tokens(id, record)
    }
    fn delete_tokens(&self, id: uuid::Uuid) -> anyhow::Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            anyhow::bail!(
                "Could not delete the sign-in tokens from macOS Keychain: Invalid attempt to change the owner of this item."
            );
        }
        self.tokens.delete_tokens(id)
    }
}

#[gpui_kit::test]
fn failed_deletion_shows_a_red_dot_and_preserves_the_sign_in_for_retry(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, false);
    let tokens = Arc::new(FailingDeletion::default());
    tokens.fail.store(true, Ordering::SeqCst);
    let mut sign_ins = SignIns::new(trust(), Some(Arc::new(provider.browser("alice"))));
    sign_ins.tokens = tokens.clone();
    let app = TestApp::launch_with_sign_ins(cx, workspace, MemoryCredentials::default(), sign_ins);
    open_sign_ins(&app, cx);
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Sign in…");
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
    let record = tokens.load_tokens(sign_in.id).unwrap().unwrap();
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Delete");
    app.click(cx, "confirm-delete-sign-in");
    wait_row(&app, cx, &sign_in, "The last action failed");
    app.update(cx, |window, cx| {
        assert_sign_in_dot(window, sign_in.id, cx.theme().danger);
    });
    assert_eq!(app.saved().sign_ins.len(), 1);
    assert_eq!(tokens.load_tokens(sign_in.id).unwrap().unwrap(), record);
    // The error remains visible in Settings, and the account remains signed in.
    open_settings(&app, cx, &sign_in);
    app.wait_until(cx, "the Keychain error", WAIT, |window, _| {
        label(window, "sign-in-account-error")
            .is_some_and(|error| error.contains("Invalid attempt to change the owner"))
    });
    app.wait_for(cx, "sign-in-account-sign-out");
    app.click(cx, "cancel-sign-in-editor");
    app.wait_gone(cx, "sign-in-name");
    tokens.fail.store(false, Ordering::SeqCst);
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Delete");
    app.click(cx, "confirm-delete-sign-in");
    app.wait_gone(cx, "sign-ins-list");
    app.wait_until(cx, "the saved removal", WAIT, |_, _| {
        app.saved().sign_ins.is_empty()
    });
    assert!(tokens.load_tokens(sign_in.id).unwrap().is_none());
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
    app.select(cx, "connection-authentication", "Sign-in");
    app.wait_for(cx, "connection-sign-in");
    app.scroll_to(cx, "connection-sign-in");
    app.click(cx, "connection-sign-in");
    app.click(cx, "connection-new-sign-in");
    // Sign-in Settings opens above Connection Settings.
    app.wait_for(cx, "sign-in-name");
    type_fields(
        &app,
        cx,
        &["Company", &provider.issuer, "qrow-desktop", "", ""],
    );
    app.click(cx, "save-sign-in-editor");
    app.wait_gone(cx, "sign-in-name");
    app.wait_until(cx, "the new choice", WAIT, |window, _| {
        value(window, "connection-sign-in").as_deref() == Some("Company")
    });
    app.click(cx, "save-profile");
    app.wait_gone(cx, "save-profile");
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
    app.select(cx, "connection-authentication", "Sign-in");
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
    app.wait_gone(cx, "save-profile");
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
    assert_eq!(profile.host, "kyuubi.example.test");
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
    app.wait_until(cx, prefix, WAIT, |_, cx| app.status(cx).starts_with(prefix));
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
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Sign in…");
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
fn an_unreachable_provider_shows_a_yellow_sign_in_dot(cx: &mut TestAppContext) {
    let provider = Provider::start();
    provider.set_access_ttl(10);
    let (workspace, sign_in) = unreachable_workspace(&provider);
    let app = launch(cx, &provider, workspace);
    open_sign_ins(&app, cx);
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Sign in…");
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
    provider.set_down(true);
    app.click(cx, "run");
    wait_row(&app, cx, &sign_in, "Cannot reach the provider");
    app.update(cx, |window, cx| {
        assert_sign_in_dot(window, sign_in.id, cx.theme().warning);
    });
}

#[gpui_kit::test]
fn a_query_cancelled_during_the_refresh_does_not_open_the_browser(cx: &mut TestAppContext) {
    let provider = Provider::start();
    provider.set_access_ttl(10);
    let (workspace, sign_in) = unreachable_workspace(&provider);
    let app = launch(cx, &provider, workspace);
    open_sign_ins(&app, cx);
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Sign in…");
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
    provider.revoke("alice");
    // The refresh takes long enough to cancel the query during it.
    provider.set_refresh_delay(Duration::from_secs(1));
    app.click(cx, "run");
    wait_query_status(&app, cx, "Connecting");
    app.click(cx, "cancel");
    wait_query_status(&app, cx, "Error: Sign-in required");
    app.settle(cx);
    assert_eq!(
        provider.authorization_grants(),
        1,
        "no second browser sign-in"
    );
}

#[gpui_kit::test]
fn two_tabs_that_need_the_same_sign_in_share_one_browser_sign_in(cx: &mut TestAppContext) {
    let provider = Provider::start();
    provider.set_access_ttl(10);
    let (mut workspace, sign_in) = unreachable_workspace(&provider);
    let mut second = SavedTab::new(2, workspace.profiles.first().map(|profile| profile.id));
    second.sql = "SELECT 2".into();
    workspace.tabs.push(second);
    let app = launch(cx, &provider, workspace);
    open_sign_ins(&app, cx);
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Sign in…");
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
    provider.revoke("alice");
    // Both refreshes fail at about the same time.
    provider.set_refresh_delay(Duration::from_millis(500));
    app.click(cx, "run");
    app.click_labelled(cx, "Query 2");
    app.click(cx, "run");
    wait_query_status(&app, cx, "Error: Connection failed");
    app.click_starting(cx, "Query 1");
    wait_query_status(&app, cx, "Error: Connection failed");
    assert_eq!(
        provider.authorization_grants(),
        2,
        "one browser sign-in for both tabs"
    );
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
fn rerunning_a_cancelled_query_reopens_the_pending_browser_sign_in(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (mut workspace, sign_in) = unreachable_workspace(&provider);
    let mut second = SavedTab::new(2, workspace.profiles.first().map(|profile| profile.id));
    second.sql = "SELECT 2".into();
    workspace.tabs.push(second);
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let browser_urls = opened.clone();
    let finish = provider.browser("alice");
    let browser = move |url: &str| {
        let count = {
            let mut urls = browser_urls.lock().unwrap();
            urls.push(url.to_owned());
            urls.len()
        };
        if count == 3 { finish(url) } else { Ok(()) }
    };
    let app = TestApp::launch_with_sign_ins(
        cx,
        workspace,
        MemoryCredentials::default(),
        SignIns::new(trust(), Some(Arc::new(browser))),
    );
    for count in 1..=3 {
        app.click(cx, "run");
        app.wait_until(cx, "the reopened browser", WAIT, |_, _| {
            opened.lock().unwrap().len() == count
        });
        if count == 1 {
            app.click_labelled(cx, "Query 2");
            app.click(cx, "run");
            wait_query_status(&app, cx, "Waiting for sign-in");
            app.settle(cx);
            assert_eq!(
                opened.lock().unwrap().len(),
                1,
                "the second tab shares the attempt"
            );
            app.click_starting(cx, "Query 1");
        }
        if count < 3 {
            wait_query_status(&app, cx, "Waiting for sign-in");
            app.click(cx, "cancel");
            wait_query_status(&app, cx, "Cancelled: Sign-in not finished");
        }
    }
    wait_query_status(&app, cx, "Error: Connection failed");
    app.click_starting(cx, "Query 2");
    wait_query_status(&app, cx, "Error: Connection failed");
    open_sign_ins(&app, cx);
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
    let urls = opened.lock().unwrap();
    assert_eq!(urls.len(), 3);
    assert!(
        urls.iter().all(|url| url == &urls[0]),
        "one shared sign-in attempt"
    );
    assert_eq!(provider.authorization_grants(), 1);
}

#[gpui_kit::test]
fn a_browser_reopen_failure_keeps_the_pending_sign_in_available(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let provider = Provider::start();
    let (workspace, _) = unreachable_workspace(&provider);
    let opened = Arc::new(AtomicUsize::new(0));
    let browser_count = opened.clone();
    let finish = provider.browser("alice");
    let browser = move |url: &str| match browser_count.fetch_add(1, Ordering::SeqCst) {
        0 => Ok(()),
        1 => anyhow::bail!("synthetic browser error"),
        _ => finish(url),
    };
    let app = TestApp::launch_with_sign_ins(
        cx,
        workspace,
        MemoryCredentials::default(),
        SignIns::new(trust(), Some(Arc::new(browser))),
    );
    app.click(cx, "run");
    app.wait_until(cx, "the initial browser", WAIT, |_, _| {
        opened.load(Ordering::SeqCst) == 1
    });
    wait_query_status(&app, cx, "Waiting for sign-in");
    app.click(cx, "cancel");
    wait_query_status(&app, cx, "Cancelled: Sign-in not finished");
    app.click(cx, "run");
    app.wait_until(cx, "the browser error", WAIT, |window, _| {
        label(window, "workspace-message")
            .is_some_and(|message| message.starts_with("Could not open the sign-in page."))
    });
    wait_query_status(&app, cx, "Waiting for sign-in");
    app.click(cx, "cancel");
    wait_query_status(&app, cx, "Cancelled: Sign-in not finished");
    app.click(cx, "run");
    wait_query_status(&app, cx, "Error: Connection failed");
    assert_eq!(opened.load(Ordering::SeqCst), 3);
    assert_eq!(provider.authorization_grants(), 1);
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

#[gpui_kit::test]
fn copy_settings_puts_the_sign_in_without_its_account_on_the_clipboard(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, false);
    let app = launch(cx, &provider, workspace);
    open_sign_ins(&app, cx);
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Sign in…");
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(String::new()));
    app.context_menu(cx, format!("sign-in-{}", sign_in.id));
    app.choose(cx, "popup-menu", "Copy settings");
    app.wait_gone(cx, "popup-menu");
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&copied).unwrap();
    assert_eq!(json["name"], "Company");
    assert_eq!(json["issuer"], provider.issuer.as_str());
    assert!(json.get("database_hosts").is_none());
    // The account and the identifier stay with this user.
    for private in ["alice", &sign_in.id.to_string(), "identity", "token"] {
        assert!(!copied.contains(private), "{private} in {copied}");
    }
}

#[gpui_kit::test]
fn a_pasted_sign_in_opens_for_review_and_saves_as_a_new_sign_in(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, existing) = workspace(&provider, false);
    let app = launch(cx, &provider, workspace);
    let mut shared = provider.sign_in("Company");
    shared.callback_ports = vec![8765];
    let message = format!("Here is our sign-in:\n{}", shared.to_shared_text());
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(message));
    open_sign_ins(&app, cx);
    app.click(cx, "paste-sign-in");
    app.wait_for(cx, "sign-in-pasted-note");
    app.update(cx, |window, _| {
        // The name of the existing sign-in stays unique.
        assert_eq!(
            value(window, "sign-in-name").as_deref(),
            Some("Company copy")
        );
        assert_eq!(
            value(window, "sign-in-issuer").as_deref(),
            Some(provider.issuer.as_str())
        );
        assert!(!present(window, &"sign-in-database-hosts".into()));
        assert_eq!(
            value(window, "sign-in-callback-ports").as_deref(),
            Some("8765")
        );
        // A new sign-in has no account yet.
        assert!(!present(window, &"sign-in-account-status".into()));
    });
    // Cancel adds nothing.
    app.click(cx, "cancel-sign-in-editor");
    app.wait_gone(cx, "sign-in-name");
    app.settle(cx);
    assert_eq!(app.saved().sign_ins.len(), 1);
    app.click(cx, "paste-sign-in");
    app.wait_for(cx, "sign-in-pasted-note");
    app.click(cx, "save-sign-in-editor");
    app.wait_gone(cx, "sign-in-name");
    app.wait_until(cx, "the pasted sign-in", WAIT, |_, _| {
        app.saved().sign_ins.len() == 2
    });
    let saved = app.saved();
    let pasted = &saved.sign_ins[1];
    assert_eq!(pasted.name, "Company copy");
    assert_ne!(pasted.id, existing.id);
    assert_ne!(pasted.id, shared.id);
    assert_eq!(pasted.identity, None);
    assert_eq!(pasted.callback_ports, vec![8765]);
    wait_row(&app, cx, pasted, "Not signed in");
    // Sign-in Settings of a saved sign-in shows no note.
    open_settings(&app, cx, pasted);
    app.update(cx, |window, _| {
        assert!(!present(window, &"sign-in-pasted-note".into()));
    });
}

#[gpui_kit::test]
fn pasting_text_that_is_not_a_sign_in_tells_why_and_adds_nothing(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let app = launch(cx, &provider, Workspace::default());
    open_sign_ins(&app, cx);
    // The empty sidebar shows only its header, with the paste and add
    // buttons.
    app.update(cx, |window, _| {
        assert!(!present(window, &"sign-ins-list".into()));
        assert!(present(window, &"add-sign-in".into()));
        assert_eq!(
            label(window, "paste-sign-in").as_deref(),
            Some("Paste sign-in")
        );
    });
    for (clipboard, reason) in [
        ("SELECT 1".to_owned(), "does not contain sign-in settings"),
        (
            provider
                .sign_in("Company")
                .to_shared_text()
                .replace("\"qrow_sign_in\": 1", "\"qrow_sign_in\": 99"),
            "newer version",
        ),
    ] {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(clipboard));
        app.click(cx, "paste-sign-in");
        app.wait_for(cx, "close-paste-sign-in-error");
        app.update(cx, |window, _| {
            let error = label(window, "paste-sign-in-error").unwrap_or_default();
            assert!(error.contains(reason), "{error}");
            assert!(!present(window, &"sign-in-name".into()));
        });
        app.click(cx, "close-paste-sign-in-error");
        app.wait_gone(cx, "close-paste-sign-in-error");
    }
    assert!(!app.update(cx, |window, _| present(window, &"sign-ins-list".into())));
    assert!(app.saved().sign_ins.is_empty());
}

#[gpui_kit::test]
fn shared_sign_ins_offer_only_oidc(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    open_sign_ins(&app, cx);
    app.click(cx, "add-sign-in");
    app.wait_for(cx, "sign-in-name");
    app.click(cx, "sign-in-provider");
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(crate::support::labelled(window, "Trino External Authentication").is_none());
    });
    app.press(cx, "escape");
    app.wait_for(cx, "sign-in-issuer");
    app.wait_for(cx, "sign-in-client-id");
    app.click(cx, "cancel-sign-in-editor");
}

#[gpui_kit::test]
fn trino_external_authentication_progress_can_be_cancelled_before_connect(cx: &mut TestAppContext) {
    use crate::support::trino_protocol::{Reply, Server};
    use qrow::model::DatabaseType;
    let mut replies = vec![Reply { status: 401, body: String::new(), headers: "WWW-Authenticate: Bearer x_redirect_server=\"{origin}/browser\", x_token_server=\"{origin}/token\"\r\n".into() }];
    replies.extend((0..300).map(|_| Reply::page(serde_json::json!({"nextUri":"{origin}/token"}))));
    let mut server = Server::new(true, replies);
    server.profile.authentication = Authentication::TrinoExternal;
    let mut tab = SavedTab::new(1, Some(server.profile.id));
    tab.sql = "SELECT 42".into();
    let opens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let browser_opens = opens.clone();
    let app = TestApp::launch_with_sign_ins(
        cx,
        Workspace {
            profiles: vec![server.profile.clone()],
            tabs: vec![tab],
            ..Workspace::default()
        },
        MemoryCredentials::default(),
        SignIns::new(
            trust(),
            Some(Arc::new(move |_| {
                browser_opens.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })),
        ),
    );
    app.click(cx, "run");
    app.wait_status(cx, "Waiting for browser sign-in");
    app.click(cx, "cancel");
    app.wait_status(cx, "Cancelled");
    assert_eq!(opens.load(Ordering::SeqCst), 1);
    assert_eq!(app.saved().profiles[0].database_type, DatabaseType::Trino);
}

#[gpui_kit::test]
fn trino_connection_selects_and_restores_external_authentication(cx: &mut TestAppContext) {
    use qrow::model::DatabaseType;
    let app = TestApp::launch(cx, Workspace::default());
    app.click(cx, "add-connection");
    app.select(cx, "connection-database-type", "Trino");
    app.fill(cx, "connection-host", "localhost");
    app.fill(cx, "connection-username", "alice");
    app.select(cx, "connection-authentication", "External");
    app.update(cx, |window, _| {
        assert!(!present(window, &"connection-sign-in".into()));
        assert!(!present(window, &"connection-password".into()));
    });
    app.click(cx, "save-profile");
    app.wait_until(cx, "HTTPS is required", WAIT, |window, _| {
        label(window, "connection-form-error-accessibility")
            .is_some_and(|error| error.contains("TLS"))
    });
    app.click(cx, "connection-tls");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "saved external connection", WAIT, |_, _| {
        !app.saved().profiles.is_empty()
    });
    let profile = app.saved().profiles[0].clone();
    assert_eq!(profile.database_type, DatabaseType::Trino);
    assert_eq!(profile.authentication, Authentication::TrinoExternal);
    assert!(app.saved().sign_ins.is_empty());
    app.context_menu(cx, crate::support::connection_row(profile.id));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-authentication");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-authentication").as_deref(),
            Some("External")
        )
    });
    app.select(cx, "connection-authentication", "Password");
    app.wait_for(cx, "connection-password");
    app.click(cx, "cancel-profile");
}

#[gpui_kit::test]
fn selecting_trino_authenticates_without_running_editor_sql_and_routes_errors(
    cx: &mut TestAppContext,
) {
    use crate::support::trino_protocol::{Reply, Server};
    let mut server = Server::new(true, vec![Reply {
        status: 401,
        body: String::new(),
        headers: "WWW-Authenticate: Bearer x_redirect_server=\"{origin}/browser\", x_token_server=\"{origin}/token\"\r\n".into(),
    }]);
    server.profile.authentication = Authentication::TrinoExternal;
    let connection = server.profile.id;
    let unrelated = offline_profile("Other connection");
    let mut tab = SavedTab::new(1, Some(connection));
    tab.sql = "SELECT 42".into();
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let browser_entered = entered.clone();
    let browser_release = release.clone();
    let app = TestApp::launch_with_sign_ins(
        cx,
        Workspace {
            profiles: vec![server.profile.clone(), unrelated.clone()],
            tabs: vec![tab, SavedTab::new(1, Some(unrelated.id))],
            ..Workspace::default()
        },
        MemoryCredentials::default(),
        SignIns::new(
            trust(),
            Some(Arc::new(move |_| {
                browser_entered.store(true, Ordering::SeqCst);
                while !browser_release.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                anyhow::bail!("Synthetic browser failure")
            })),
        ),
    );
    app.click(cx, format!("profile-{connection}"));
    app.wait_until(cx, "automatic browser launch", WAIT, |_, _| {
        entered.load(Ordering::SeqCst)
    });
    app.wait_for(cx, "connection-authentication-progress");
    // A different visible connection must not receive the failure.
    app.click(cx, format!("profile-{}", unrelated.id));
    release.store(true, Ordering::SeqCst);
    app.wait_until(cx, "connection authentication error", WAIT, |window, _| {
        label(window, crate::support::connection_row(connection))
            .is_some_and(|label| label.contains("unread error"))
    });
    app.click(cx, format!("connection-status-{connection}"));
    app.wait_for(cx, "activity");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "activity-connection").as_deref(),
            Some(server.profile.name.as_str())
        )
    });
    assert!(
        app.copy_activity(cx)
            .contains("Cannot open the sign-in browser")
    );
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].ends_with("SELECT 1"));
    assert!(!server.requests()[0].contains("SELECT 42"));
}

#[gpui_kit::test]
fn changing_connection_type_removes_external_authentication(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    app.click(cx, "add-connection");
    app.select(cx, "connection-database-type", "Trino");
    app.select(cx, "connection-authentication", "External");
    app.select(cx, "connection-database-type", "Spark (HiveServer2)");
    app.wait_for(cx, "connection-password");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-authentication").as_deref(),
            Some("Password")
        )
    });
    app.click(cx, "connection-authentication");
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(crate::support::labelled(window, "External").is_none())
    });
    app.press(cx, "escape");
    app.click(cx, "cancel-profile");
}

#[gpui_kit::test]
fn selecting_oidc_connection_starts_browser_sign_in(cx: &mut TestAppContext) {
    let provider = Provider::start();
    let (workspace, sign_in) = workspace(&provider, true);
    let connection = workspace.profiles[0].id;
    let app = launch(cx, &provider, workspace);
    app.click(cx, format!("profile-{connection}"));
    open_sign_ins(&app, cx);
    wait_row(&app, cx, &sign_in, "alice@qrow.test");
    assert_eq!(provider.authorization_grants(), 1);
    assert!(!app.logs(cx).contains("Submitted query"));
}

#[gpui_kit::test]
fn successful_trino_query_retry_clears_the_previous_authentication_error(cx: &mut TestAppContext) {
    use crate::support::trino_protocol::{Reply, Server};
    let challenge = || {
        Reply {
        status: 401,
        body: String::new(),
        headers: "WWW-Authenticate: Bearer x_redirect_server=\"{origin}/browser\", x_token_server=\"{origin}/token\"\r\n".into(),
    }
    };
    let done = || {
        Reply::page(
            serde_json::json!({"columns":[{"name":"value","type":"integer"}],"data":[[42]]}),
        )
    };
    let mut server = Server::new(
        true,
        vec![
            challenge(),
            challenge(),
            Reply::page(serde_json::json!({"token":"synthetic-opaque"})),
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
            done(),
            done(),
        ],
    );
    server.profile.authentication = Authentication::TrinoExternal;
    let mut tab = SavedTab::new(1, Some(server.profile.id));
    tab.sql = "SELECT 42".into();
    let opens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let browser_opens = opens.clone();
    let app = TestApp::launch_with_sign_ins(
        cx,
        Workspace {
            profiles: vec![server.profile.clone()],
            tabs: vec![tab],
            ..Workspace::default()
        },
        MemoryCredentials::default(),
        SignIns::new(
            trust(),
            Some(Arc::new(move |_| {
                if browser_opens.fetch_add(1, Ordering::SeqCst) == 0 {
                    anyhow::bail!("Synthetic browser failure");
                }
                Ok(())
            })),
        ),
    );
    app.click(cx, "run");
    app.wait_status(cx, "Error: Connection failed");
    app.click(cx, "run");
    app.wait_status(cx, "Complete");
    assert!(app.saved().sign_ins.is_empty());
    assert_eq!(opens.load(Ordering::SeqCst), 2);
}
