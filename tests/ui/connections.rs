use crate::support::{
    MemoryCredentials, TestApp, assert_tooltip_header_center, connection_row, label, menu_item,
    offline_profile,
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{InputEvent as _, MouseMoveEvent, TestAppContext};
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
    app.scroll_to(cx, "connection-password");
    app.fill(cx, "connection-password", "synthetic-password");
}

fn wait_error(app: &TestApp, cx: &mut TestAppContext, expected: &str) {
    app.wait_until(cx, expected, Duration::from_secs(10), |window, _| {
        label(window, "connection-form-error-accessibility").as_deref() == Some(expected)
    });
}

fn cancel_form(app: &TestApp, cx: &mut TestAppContext) {
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "save-profile");
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

fn assert_order(app: &TestApp, cx: &mut TestAppContext, expected: &[&str]) {
    app.wait_until(
        cx,
        "the saved connection order",
        Duration::from_secs(10),
        |_, _| {
            app.saved()
                .profiles
                .iter()
                .map(|profile| profile.name.as_str())
                .eq(expected.iter().copied())
        },
    );
    let saved = app.saved();
    app.update(cx, |window, _| {
        let positions: Vec<_> = saved
            .profiles
            .iter()
            .map(|profile| window.find(connection_row(profile.id)).bounds().top())
            .collect();
        assert!(positions.is_sorted_by(|a, b| a < b), "{positions:?}");
    });
}

#[gpui_kit::test]
fn dragging_connections_saves_order_and_keeps_the_active_tab(cx: &mut TestAppContext) {
    let (workspace, credentials) = connections(&["Alpha", "Beta", "Gamma"]);
    let ids: Vec<_> = workspace
        .profiles
        .iter()
        .map(|profile| profile.id)
        .collect();
    let tabs = workspace.tabs.clone();
    let app = TestApp::launch_with(cx, workspace, credentials);
    for (source, target, after, expected) in [
        (ids[2], ids[0], false, ["Gamma", "Alpha", "Beta"]),
        (ids[2], ids[1], true, ["Alpha", "Beta", "Gamma"]),
        (ids[0], ids[1], true, ["Beta", "Alpha", "Gamma"]),
        (ids[2], ids[0], false, ["Beta", "Gamma", "Alpha"]),
    ] {
        app.update(cx, |window, cx| {
            window.drag_to(
                format!("profile-{source}"),
                format!(
                    "connection-drop-{}-{target}",
                    if after { "after" } else { "before" }
                ),
                cx,
            );
        });
        assert_order(&app, cx, &expected);
        assert_eq!(app.saved().tabs, tabs);
        assert_eq!(app.saved().active_tab, 0);
    }
    let restored = TestApp::launch(cx, app.saved());
    assert_order(&restored, cx, &["Beta", "Gamma", "Alpha"]);
    assert_eq!(restored.saved().tabs, tabs);
}

#[gpui_kit::test]
fn connection_drops_on_self_or_outside_the_list_keep_order(cx: &mut TestAppContext) {
    let (workspace, credentials) = connections(&["Alpha", "Beta"]);
    let id = workspace.profiles[0].id;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.update(cx, |window, cx| {
        window.drag_to(
            format!("profile-{id}"),
            format!("connection-drop-after-{id}"),
            cx,
        )
    });
    assert_order(&app, cx, &["Alpha", "Beta"]);
    app.update(cx, |window, cx| {
        window.drag_to(format!("profile-{id}"), "run", cx)
    });
    assert_order(&app, cx, &["Alpha", "Beta"]);
    app.select_connection(cx, &app.saved().profiles[1]);
    assert_eq!(app.saved().profiles.len(), 2);
}

#[gpui_kit::test]
fn connections_move_by_menu_and_keyboard_with_boundary_guards(cx: &mut TestAppContext) {
    let (workspace, credentials) = connections(&["Alpha", "Beta", "Gamma"]);
    let beta = workspace.profiles[1].id;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.context_menu(cx, connection_row(beta));
    app.choose(cx, "popup-menu", "Move up");
    app.wait_gone(cx, "popup-menu");
    assert_order(&app, cx, &["Beta", "Alpha", "Gamma"]);
    app.context_menu(cx, connection_row(beta));
    app.choose(cx, "popup-menu", "Move up");
    app.wait_for(cx, "popup-menu");
    app.press(cx, "escape");
    // The disclosure focuses and selects the tree row.
    app.click(
        cx,
        format!(
            "c{separator}{beta}{separator}disclosure",
            separator = '\u{1f}'
        ),
    );
    app.press(cx, "alt-up");
    assert_order(&app, cx, &["Beta", "Alpha", "Gamma"]);
    app.press(cx, "alt-down");
    assert_order(&app, cx, &["Alpha", "Beta", "Gamma"]);
    app.press(cx, "alt-down");
    assert_order(&app, cx, &["Alpha", "Gamma", "Beta"]);
    app.press(cx, "alt-down");
    assert_order(&app, cx, &["Alpha", "Gamma", "Beta"]);
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
fn a_tooltip_in_the_form_closes_with_the_form(cx: &mut TestAppContext) {
    let (workspace, credentials) = connections(&["Qrow E2E"]);
    let profile = workspace.profiles[0].id;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.context_menu(cx, connection_row(profile));
    app.choose(cx, "popup-menu", "Edit");
    // The shortcut needs the focus in the form.
    app.fill(cx, "connection-name", "Qrow E2E");
    app.update(cx, |window, cx| {
        let position = window.find("save-profile").bounds().center();
        window.dispatch_event(
            MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Default::default(),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    });
    cx.executor().advance_clock(Duration::from_millis(800));
    app.wait_for(cx, "tooltip");
    app.update(cx, |window, cx| {
        assert_eq!(
            label(window, "status-tooltip-title").as_deref(),
            Some("Save connection")
        );
        assert_tooltip_header_center(window, cx, "status-tooltip-shortcut");
    });

    // The shortcut closes the form under the pointer, so the Save button
    // never gets a hover-out. The tooltip closes with the form.
    app.press(cx, "cmd-enter");
    app.wait_gone(cx, "save-profile");
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(window.try_find("tooltip").is_none())
    });
}

#[gpui_kit::test]
fn the_connection_menu_edits_duplicates_and_deletes(cx: &mut TestAppContext) {
    let (workspace, credentials) = connections(&["Qrow E2E"]);
    let original = workspace.profiles[0].id;
    let app = TestApp::launch_with(cx, workspace, credentials);

    app.context_menu(cx, connection_row(original));
    app.update(cx, |window, _| {
        // The connection section, then the schemas section, from top to
        // bottom. Section titles are not items, so they have their own IDs.
        let top = |window: &mut gpui_kit::Window, entry: &str| {
            if let Some(title) = window.try_find(format!("menu-section-{entry}")) {
                return title.bounds().origin.y;
            }
            let index = menu_item(window, "popup-menu", entry)
                .unwrap_or_else(|| panic!("The menu has no {entry}"));
            window
                .within("popup-menu".to_owned())
                .find(index)
                .bounds()
                .origin
                .y
        };
        let order = [
            "Connection",
            "Disconnect",
            "Edit",
            "Duplicate",
            "Delete",
            "Schemas",
            "Refresh",
            "Collapse",
        ]
        .map(|entry| top(window, entry));
        assert!(order.is_sorted_by(|a, b| a < b), "{order:?}");
        // A section title is not a command.
        assert_eq!(
            window.find("menu-section-Connection").label(),
            Some("Connection")
        );
    });
    // A dismissed menu is released; the leak detector fails the test otherwise.
    app.press(cx, "escape");
    app.wait_gone(cx, "popup-menu");

    // With no live session, Disconnect cannot run. Keyboard navigation
    // skips it and reaches Edit, the first enabled command.
    app.context_menu(cx, connection_row(original));
    app.choose(cx, "popup-menu", "Disconnect");
    app.wait_for(cx, "popup-menu");
    app.press(cx, "down");
    app.press(cx, "enter");
    app.wait_for(cx, "connection-name");
    cancel_form(&app, cx);

    app.context_menu(cx, connection_row(original));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-password");
    cancel_form(&app, cx);

    for expected in ["Qrow E2E copy", "Qrow E2E copy 2"] {
        app.context_menu(cx, connection_row(original));
        app.choose(cx, "popup-menu", "Duplicate");
        app.wait_for(cx, "connection-password");
        app.scroll_to(cx, "connection-password");
        app.fill(cx, "connection-password", "copy-password");
        app.click(cx, "save-profile");
        app.wait_gone(cx, "save-profile");
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

#[gpui_kit::test]
fn the_response_timeout_is_validated_and_saved(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    open_new_connection(&app, cx);
    fill_connection(&app, cx, "Slow Engine");
    app.scroll_to(cx, "connection-response-timeout");
    app.update(cx, |window, _| {
        assert_eq!(
            crate::support::value(window, "connection-response-timeout").as_deref(),
            Some("300")
        );
    });
    app.fill(cx, "connection-response-timeout", "5");
    app.click(cx, "save-profile");
    wait_error(
        &app,
        cx,
        "Response timeout must be between 10 and 3600 seconds.",
    );
    app.scroll_to(cx, "connection-response-timeout");
    app.fill(cx, "connection-response-timeout", "600");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "save-profile");
    app.wait_until(cx, "the saved timeout", Duration::from_secs(10), |_, _| {
        app.saved()
            .profiles
            .iter()
            .any(|profile| profile.lifecycle.response_timeout_seconds == 600)
    });
}

#[gpui_kit::test]
fn connection_settings_open_on_the_general_page(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    open_new_connection(&app, cx);
    app.connection_page(cx, "dbt");
    app.wait_for(cx, "connection-dbt-manifest");
    app.wait_gone(cx, "connection-name");
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "save-profile");
    open_new_connection(&app, cx);
    app.update(cx, |window, _| {
        assert!(window.try_find("connection-dbt-manifest").is_none());
    });
}

#[gpui_kit::test]
fn postgres_connection_uses_password_and_saves_its_database_type(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    open_new_connection(&app, cx);
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "connection-database-type").as_deref(),
            Some("Connection Type")
        );
        assert_eq!(
            label(window, "setting-help-Connection Type").as_deref(),
            Some("The database engine of the server.")
        );
    });
    app.select(cx, "connection-database-type", "Postgres");
    app.update(cx, |window, _| {
        assert_eq!(
            crate::support::value(window, "connection-name").as_deref(),
            Some("Postgres")
        );
        assert_eq!(
            crate::support::value(window, "connection-port").as_deref(),
            Some("5432")
        );
        assert_eq!(
            crate::support::value(window, "connection-database").as_deref(),
            Some("postgres")
        );
        assert!(window.try_find("connection-authentication").is_none());
        assert!(window.try_find("connection-tls").is_none());
        assert_eq!(
            label(window, "connection-postgres-ssl-mode").as_deref(),
            Some("TLS Mode")
        );
        assert_eq!(
            crate::support::value(window, "connection-postgres-ssl-mode").as_deref(),
            Some("Require TLS")
        );
    });
    fill_connection(&app, cx, "Postgres test");
    app.fill(cx, "connection-port", "5432");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(
        cx,
        "the saved Postgres profile",
        Duration::from_secs(10),
        |_, _| !app.saved().profiles.is_empty(),
    );
    assert_eq!(
        app.saved().profiles[0].database_type,
        qrow::model::DatabaseType::Postgres
    );
    assert_eq!(
        app.saved().profiles[0].postgres_ssl_mode,
        Some(qrow::model::PostgresSslMode::Require)
    );
    assert_eq!(
        app.saved().profiles[0].authentication,
        qrow::model::Authentication::Password
    );
}

#[gpui_kit::test]
fn postgres_tls_choices_save_and_keep_legacy_verification(cx: &mut TestAppContext) {
    use qrow::model::{DatabaseType, PostgresSslMode};
    let (mut workspace, credentials) = connections(&["Postgres legacy"]);
    let id = workspace.profiles[0].id;
    workspace.profiles[0].database_type = DatabaseType::Postgres;
    workspace.profiles[0].tls = true;
    let app = TestApp::launch_with(cx, workspace, credentials);
    for (label, mode) in [
        ("Require TLS", PostgresSslMode::Require),
        ("Disabled", PostgresSslMode::Disable),
        ("Verify certificate", PostgresSslMode::VerifyFull),
    ] {
        app.context_menu(cx, connection_row(id));
        app.choose(cx, "popup-menu", "Edit");
        app.wait_for(cx, "connection-name");
        if app.saved().profiles[0].postgres_ssl_mode.is_none() {
            app.update(cx, |window, _| {
                assert_eq!(
                    crate::support::value(window, "connection-postgres-ssl-mode").as_deref(),
                    Some("Verify certificate")
                )
            });
        }
        app.select(cx, "connection-postgres-ssl-mode", label);
        app.click(cx, "save-profile");
        app.wait_gone(cx, "connection-name");
        app.wait_until(
            cx,
            "the saved TLS choice",
            Duration::from_secs(10),
            |_, _| app.saved().profiles[0].postgres_ssl_mode == Some(mode),
        );
        assert_eq!(
            app.saved().profiles[0].tls,
            mode != PostgresSslMode::Disable
        );
    }
    app.context_menu(cx, connection_row(id));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    app.update(cx, |window, _| {
        assert_eq!(
            crate::support::value(window, "connection-postgres-ssl-mode").as_deref(),
            Some("Verify certificate")
        )
    });
    cancel_form(&app, cx);
}

#[gpui_kit::test]
fn changing_database_type_keeps_custom_connection_fields(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    open_new_connection(&app, cx);
    app.fill(cx, "connection-name", "Custom");
    app.fill(cx, "connection-port", "6543");
    app.scroll_to(cx, "connection-database");
    app.fill(cx, "connection-database", "custom_database");
    app.scroll_to(cx, "connection-database-type");
    app.select(cx, "connection-database-type", "Postgres");
    app.update(cx, |window, _| {
        assert_eq!(
            crate::support::value(window, "connection-name").as_deref(),
            Some("Custom")
        );
        assert_eq!(
            crate::support::value(window, "connection-port").as_deref(),
            Some("6543")
        );
        assert_eq!(
            crate::support::value(window, "connection-database").as_deref(),
            Some("custom_database")
        );
    });
    cancel_form(&app, cx);
}

#[gpui_kit::test]
fn connection_icons_identify_postgres_and_spark(cx: &mut TestAppContext) {
    let (mut workspace, credentials) = connections(&["Spark test", "Postgres test"]);
    workspace.profiles[1].database_type = qrow::model::DatabaseType::Postgres;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.wait_label(cx, "Spark (HiveServer2) database");
    app.wait_label(cx, "Postgres database");
}

#[gpui_kit::test]
fn connection_help_is_shared_and_does_not_repeat_the_database_type(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    open_new_connection(&app, cx);
    let fields = [
        "Host",
        "Port",
        "Username",
        "Password",
        "Initial Database",
        "Session Parameters",
    ];
    let shared = app.update(cx, |window, _| {
        fields.map(|field| label(window, format!("setting-help-{field}")).unwrap())
    });
    app.select(cx, "connection-database-type", "Postgres");
    app.update(cx, |window, _| {
        for (field, expected) in fields.into_iter().zip(shared) {
            assert_eq!(
                label(window, format!("setting-help-{field}")).as_deref(),
                Some(expected.as_str())
            );
        }
        for help in gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|element| {
                element
                    .path()
                    .iter()
                    .any(|id| format!("{id:?}").contains("setting-help-"))
            })
        {
            let text = help.label().unwrap_or_default();
            assert!(
                !["Postgres", "Kyuubi", "HiveServer2", "LDAP"]
                    .iter()
                    .any(|kind| text.contains(kind)),
                "database type leaked into help: {text}"
            );
        }
    });
    app.scroll_to(cx, "connection-response-timeout");
    app.wait_label(
        cx,
        "Seconds to wait for setup and cancellation, from 10 to 3600.",
    );
    app.scroll_to(cx, "connection-database-type");
    app.select(cx, "connection-database-type", "Spark (HiveServer2)");
    app.scroll_to(cx, "connection-response-timeout");
    app.wait_label(
        cx,
        "Seconds to wait for a server response, from 10 to 3600.",
    );
    cancel_form(&app, cx);
}

#[gpui_kit::test]
fn trino_connection_saves_catalog_schema_and_optional_password(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    open_new_connection(&app, cx);
    app.select(cx, "connection-database-type", "Trino");
    app.update(cx, |window, _| {
        assert_eq!(
            crate::support::value(window, "connection-name").as_deref(),
            Some("Trino")
        );
        assert_eq!(
            crate::support::value(window, "connection-port").as_deref(),
            Some("8080")
        );
        assert_eq!(
            crate::support::value(window, "connection-database").as_deref(),
            Some("tpch")
        );
        assert_eq!(
            label(window, "connection-database").as_deref(),
            Some("Initial Catalog")
        );
        assert_eq!(
            label(window, "connection-trino-schema").as_deref(),
            Some("Initial Schema")
        );
        assert!(window.try_find("connection-authentication").is_some());
        assert!(window.try_find("connection-postgres-ssl-mode").is_none());
    });
    app.fill(cx, "connection-host", "localhost");
    app.fill(cx, "connection-username", "synthetic-user");
    app.scroll_to(cx, "connection-trino-schema");
    app.fill(cx, "connection-trino-schema", "tiny");
    app.press(cx, "cmd-enter");
    app.wait_gone(cx, "connection-name");
    app.wait_until(
        cx,
        "the saved Trino profile",
        Duration::from_secs(10),
        |_, _| !app.saved().profiles.is_empty(),
    );
    let profile = &app.saved().profiles[0];
    assert_eq!(profile.database_type, qrow::model::DatabaseType::Trino);
    assert_eq!(profile.database, "tpch");
    assert_eq!(profile.trino_schema, "tiny");
    assert_eq!(app.credentials.get(profile.id).as_deref(), Some(""));
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-trino-schema");
    app.update(cx, |window, _| {
        assert_eq!(
            crate::support::value(window, "connection-trino-schema").as_deref(),
            Some("tiny")
        );
    });
    app.scroll_to(cx, "connection-database-type");
    app.select(cx, "connection-database-type", "Postgres");
    app.wait_gone(cx, "connection-trino-schema");
    app.select(cx, "connection-database-type", "Trino");
    app.wait_for(cx, "connection-trino-schema");
    app.press(cx, "escape");
    app.wait_gone(cx, "connection-name");
}
