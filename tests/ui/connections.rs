use crate::support::{
    MemoryCredentials, TestApp, connection_row, label, menu_item, offline_profile,
};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::{
    model::{SavedTab, Workspace},
    storage::Credentials,
};
use std::time::Duration;

fn open_new_connection(app: &TestApp, cx: &mut TestAppContext) {
    app.click(cx, "add-connection");
    app.wait_for(cx, "connection-name");
}

fn fill_connection(app: &TestApp, cx: &mut TestAppContext, name: &str) {
    app.fill(cx, "connection-name", name);
    app.fill(cx, "connection-host", "127.0.0.1");
    app.fill(cx, "connection-port", "10009");
    app.fill(cx, "connection-username", "synthetic-user");
    app.fill(cx, "connection-password", "synthetic-password");
}

fn wait_error(app: &TestApp, cx: &mut TestAppContext, expected: &str) {
    app.wait_until(cx, expected, Duration::from_secs(10), |window, _| {
        label(window, "connection-form-error-accessibility").as_deref() == Some(expected)
    });
}

fn cancel_form(app: &TestApp, cx: &mut TestAppContext) {
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "connection-name");
}

fn saved_names(app: &TestApp) -> Vec<String> {
    let mut names: Vec<_> = app.saved().profiles.into_iter().map(|p| p.name).collect();
    names.sort();
    names
}

/// A workspace with these offline connections, one tab each, and a password
/// for each connection.
fn connections(names: &[&str]) -> (Workspace, MemoryCredentials) {
    let credentials = MemoryCredentials::default();
    let profiles: Vec<_> = names.iter().map(|name| offline_profile(name)).collect();
    for profile in &profiles {
        credentials
            .set_password(profile.id, "synthetic-password")
            .unwrap();
    }
    let tabs = profiles
        .iter()
        .map(|p| SavedTab::new(1, Some(p.id)))
        .collect();
    (
        Workspace {
            profiles,
            tabs,
            ..Workspace::default()
        },
        credentials,
    )
}

#[gpui_kit::test]
fn new_connection_saves_its_password_in_the_injected_store(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    open_new_connection(&app, cx);
    fill_connection(&app, cx, "Synthetic Alpha");
    app.click(cx, "save-profile");

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

#[gpui_kit::test]
fn a_missing_username_keeps_the_form_open(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    open_new_connection(&app, cx);
    app.fill(cx, "connection-host", "127.0.0.1");
    app.click(cx, "save-profile");
    wait_error(&app, cx, "Enter a username.");
    app.update(cx, |window, _| {
        assert!(window.try_find("connection-username").is_some())
    });
    cancel_form(&app, cx);
    assert!(app.saved().profiles.is_empty());
    assert_eq!(app.credentials.count(), 0);
}

#[gpui_kit::test]
fn connection_names_stay_unique_on_create_and_rename(cx: &mut TestAppContext) {
    let (workspace, credentials) = connections(&["Qrow E2E", "Other"]);
    let other = workspace.profiles[1].id;
    let app = TestApp::launch_with(cx, workspace, credentials);

    // The name check runs before the password reaches the password store.
    open_new_connection(&app, cx);
    fill_connection(&app, cx, "Qrow E2E");
    app.click(cx, "save-profile");
    wait_error(&app, cx, "A connection with this name already exists.");
    cancel_form(&app, cx);
    assert_eq!(app.credentials.count(), 2);

    app.context_menu(cx, connection_row(other));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    app.fill(cx, "connection-name", "Qrow E2E");
    app.click(cx, "save-profile");
    wait_error(&app, cx, "A connection with this name already exists.");
    cancel_form(&app, cx);
    assert_eq!(saved_names(&app), ["Other", "Qrow E2E"]);
}

#[gpui_kit::test]
fn the_connection_menu_edits_duplicates_and_deletes(cx: &mut TestAppContext) {
    let (workspace, credentials) = connections(&["Qrow E2E"]);
    let original = workspace.profiles[0].id;
    let app = TestApp::launch_with(cx, workspace, credentials);

    app.context_menu(cx, connection_row(original));
    app.update(cx, |window, _| {
        // The connection section, then the schemas section, in this order.
        let order = [
            "Connection",
            "Edit",
            "Duplicate",
            "Delete",
            "Schemas",
            "Refresh",
            "Collapse",
        ]
        .map(|item| {
            menu_item(window, "popup-menu", item)
                .unwrap_or_else(|| panic!("The menu has no {item}"))
        });
        assert!(order.is_sorted_by(|a, b| a < b), "{order:?}");
    });
    // A dismissed menu is released; the leak detector fails the test otherwise.
    app.press(cx, "escape");
    app.wait_gone(cx, "popup-menu");

    app.context_menu(cx, connection_row(original));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-password");
    cancel_form(&app, cx);

    for expected in ["Qrow E2E copy", "Qrow E2E copy 2"] {
        app.context_menu(cx, connection_row(original));
        app.choose(cx, "popup-menu", "Duplicate");
        app.wait_for(cx, "connection-password");
        app.fill(cx, "connection-password", "copy-password");
        app.click(cx, "save-profile");
        app.wait_gone(cx, "connection-name");
        app.wait_until(cx, expected, Duration::from_secs(10), |_, _| {
            app.saved().profiles.iter().any(|p| p.name == expected)
        });
    }
    let copy = app
        .saved()
        .profiles
        .into_iter()
        .find(|p| p.name == "Qrow E2E copy 2")
        .unwrap();
    assert_eq!(
        app.credentials.get(copy.id).as_deref(),
        Some("copy-password")
    );

    app.context_menu(cx, connection_row(copy.id));
    app.choose(cx, "popup-menu", "Delete");
    app.click(cx, "confirm-delete-connection");
    app.wait_until(
        cx,
        "the deleted connection",
        Duration::from_secs(10),
        |_, _| app.saved().profiles.iter().all(|p| p.id != copy.id),
    );
    app.wait_until(
        cx,
        "the deleted password",
        Duration::from_secs(10),
        |_, _| app.credentials.get(copy.id).is_none(),
    );
    assert_eq!(saved_names(&app), ["Qrow E2E", "Qrow E2E copy"]);
}
