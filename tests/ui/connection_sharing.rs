//! Copy settings, Paste connection, Import connections, and Export
//! connections.
use crate::support::{MemoryCredentials, TestApp, connection_row, label, offline_profile, value};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ClipboardItem, TestAppContext};
use qrow::{
    model::{Authentication, SavedTab, SignIn, Workspace},
    storage::Credentials,
};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);
const PASSWORD: &str = "synthetic-password";

fn sign_in() -> SignIn {
    SignIn {
        name: "Company".into(),
        issuer: "https://id.example.test/realms/data".into(),
        client_id: "qrow-desktop".into(),
        ..SignIn::default()
    }
}

/// A workspace with a password connection, "Warehouse", and a connection
/// with the sign-in "Company", "Lake".
fn workspace() -> (Workspace, MemoryCredentials) {
    let credentials = MemoryCredentials::default();
    let sign_in = sign_in();
    let warehouse = offline_profile("Warehouse");
    credentials.set_password(warehouse.id, PASSWORD).unwrap();
    let lake = qrow::model::Profile {
        authentication: Authentication::Oidc {
            sign_in: sign_in.id,
        },
        ..offline_profile("Lake")
    };
    let tabs = vec![
        SavedTab::new(1, Some(warehouse.id)),
        SavedTab::new(1, Some(lake.id)),
    ];
    (
        Workspace {
            profiles: vec![warehouse, lake],
            sign_ins: vec![sign_in],
            tabs,
            ..Workspace::default()
        },
        credentials,
    )
}

fn clipboard(cx: &mut TestAppContext) -> String {
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default()
}

fn copy_settings(app: &TestApp, cx: &mut TestAppContext, id: uuid::Uuid) -> String {
    cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
    app.context_menu(cx, connection_row(id));
    app.choose(cx, "popup-menu", "Copy settings");
    app.wait_gone(cx, "popup-menu");
    clipboard(cx)
}

fn paste(app: &TestApp, cx: &mut TestAppContext, text: String) {
    cx.write_to_clipboard(ClipboardItem::new_string(text));
    app.click(cx, "paste-connection");
    app.wait_for(cx, "connection-pasted-note");
}

fn toast_detail(app: &TestApp, cx: &mut TestAppContext, toast: &'static str) -> String {
    app.wait_for(cx, toast);
    app.update(cx, |window, _| {
        window
            .within(toast)
            .find("detail")
            .label()
            .unwrap_or_default()
            .to_owned()
    })
}

#[gpui_kit::test]
fn copy_settings_puts_the_connection_without_its_password_on_the_clipboard(
    cx: &mut TestAppContext,
) {
    let (workspace, credentials) = workspace();
    let (warehouse, lake) = (workspace.profiles[0].clone(), workspace.profiles[1].clone());
    let sign_in = workspace.sign_ins[0].clone();
    let app = TestApp::launch_with(cx, workspace, credentials);
    let copied = copy_settings(&app, cx, warehouse.id);
    let json: serde_json::Value = serde_json::from_str(&copied).unwrap();
    assert_eq!(json["qrow_connection"], 1);
    assert_eq!(json["name"], "Warehouse");
    assert_eq!(json["host"], "example.invalid");
    assert_eq!(json["username"], "synthetic");
    for private in [PASSWORD, &warehouse.id.to_string(), "\"id\""] {
        assert!(!copied.contains(private), "{private} in {copied}");
    }
    // A sign-in goes with its settings, not its identifier.
    let copied = copy_settings(&app, cx, lake.id);
    let json: serde_json::Value = serde_json::from_str(&copied).unwrap();
    assert_eq!(json["authentication"]["method"], "oidc");
    assert_eq!(json["authentication"]["sign_in"]["issuer"], sign_in.issuer);
    assert!(!copied.contains(&sign_in.id.to_string()), "{copied}");
}

#[gpui_kit::test]
fn a_pasted_connection_opens_for_review_and_saves_with_a_new_password(cx: &mut TestAppContext) {
    let (workspace, credentials) = workspace();
    let warehouse = workspace.profiles[0].clone();
    let app = TestApp::launch_with(cx, workspace, credentials);
    let copied = copy_settings(&app, cx, warehouse.id);
    let message = format!("Here is our warehouse:\n```\n{copied}\n```\nUse {{your name}}.");
    paste(&app, cx, message.clone());
    app.update(cx, |window, _| {
        // The name of the existing connection stays unique.
        assert_eq!(
            value(window, "connection-name").as_deref(),
            Some("Warehouse copy")
        );
        assert_eq!(
            value(window, "connection-host").as_deref(),
            Some("example.invalid")
        );
        assert_eq!(
            value(window, "connection-username").as_deref(),
            Some("synthetic")
        );
        // A masked field reports no value, so the password shows no text.
        assert!(
            value(window, "connection-password")
                .unwrap_or_default()
                .is_empty()
        );
        assert!(
            label(window, "connection-pasted-note")
                .unwrap()
                .contains("trust the host")
        );
    });
    // Cancel adds nothing.
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "save-profile");
    app.settle(cx);
    assert_eq!(app.saved().profiles.len(), 2);

    paste(&app, cx, message);
    app.click(cx, "save-profile");
    app.wait_until(cx, "the password error", WAIT, |window, _| {
        label(window, "connection-form-error-accessibility").as_deref()
            == Some("Enter the password for this connection.")
    });
    app.scroll_to(cx, "connection-password");
    app.fill(cx, "connection-password", "pasted-password");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "save-profile");
    app.wait_until(cx, "the pasted connection", WAIT, |_, _| {
        app.saved().profiles.len() == 3
    });
    let pasted = app.saved().profiles[2].clone();
    assert_eq!(pasted.name, "Warehouse copy");
    assert_ne!(pasted.id, warehouse.id);
    assert_eq!(pasted.host, warehouse.host);
    assert_eq!(pasted.catalog, warehouse.catalog);
    assert_eq!(
        app.credentials.get(pasted.id).as_deref(),
        Some("pasted-password")
    );
    // Connection Settings of a saved connection show no note.
    app.context_menu(cx, connection_row(pasted.id));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    app.update(cx, |window, _| {
        assert!(window.try_find("connection-pasted-note").is_none());
    });
}

#[gpui_kit::test]
fn a_pasted_connection_uses_a_matching_sign_in_or_adds_its_own_on_save(cx: &mut TestAppContext) {
    let (workspace, credentials) = workspace();
    let lake = workspace.profiles[1].clone();
    let existing = workspace.sign_ins[0].clone();
    let app = TestApp::launch_with(cx, workspace, credentials);
    let copied = copy_settings(&app, cx, lake.id);

    // The workspace has a sign-in with the same settings.
    paste(&app, cx, copied.clone());
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-sign-in").as_deref(),
            Some("Company")
        );
        assert!(
            !label(window, "connection-pasted-note")
                .unwrap()
                .contains("adds the sign-in")
        );
    });
    app.click(cx, "save-profile");
    app.wait_gone(cx, "save-profile");
    app.wait_until(cx, "the pasted connection", WAIT, |_, _| {
        app.saved().profiles.len() == 3
    });
    let saved = app.saved();
    assert_eq!(saved.sign_ins.len(), 1);
    assert_eq!(
        saved.profiles[2].authentication,
        Authentication::Oidc {
            sign_in: existing.id
        }
    );

    // Another issuer needs a new sign-in. Only Save adds it.
    let other = copied.replace("id.example.test", "other.example.test");
    paste(&app, cx, other.clone());
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-sign-in").as_deref(),
            Some("Company copy")
        );
        assert!(
            label(window, "connection-pasted-note")
                .unwrap()
                .contains("Save also adds the sign-in \"Company copy\".")
        );
    });
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "save-profile");
    app.settle(cx);
    assert_eq!(app.saved().sign_ins.len(), 1);
    paste(&app, cx, other);
    app.click(cx, "save-profile");
    app.wait_gone(cx, "save-profile");
    app.wait_until(cx, "the new sign-in", WAIT, |_, _| {
        app.saved().sign_ins.len() == 2
    });
    let saved = app.saved();
    let added = &saved.sign_ins[1];
    assert_eq!(added.name, "Company copy");
    assert_eq!(added.issuer, "https://other.example.test/realms/data");
    assert_eq!(added.identity, None);
    assert_eq!(
        saved.profiles[3].authentication,
        Authentication::Oidc { sign_in: added.id }
    );
}

#[gpui_kit::test]
fn pasting_other_text_explains_why_and_adds_nothing(cx: &mut TestAppContext) {
    let (workspace, credentials) = workspace();
    let app = TestApp::launch_with(cx, workspace, credentials);
    cx.write_to_clipboard(ClipboardItem::new_string("SELECT 1".into()));
    app.dispatch(cx, qrow::ui::PasteConnection);
    app.wait_for(cx, "paste-connection-error");
    app.update(cx, |window, _| {
        assert!(
            label(window, "paste-connection-error")
                .unwrap()
                .contains("does not contain connection settings")
        );
        assert!(window.try_find("connection-name").is_none());
    });
    app.click(cx, "close-paste-connection-error");
    app.wait_gone(cx, "paste-connection-error");
}

#[gpui_kit::test]
fn exported_connections_import_with_free_names_and_their_sign_ins(cx: &mut TestAppContext) {
    let (workspace, credentials) = workspace();
    let existing = workspace.sign_ins[0].id;
    let app = TestApp::launch_with(cx, workspace, credentials);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.json");

    app.dispatch(cx, qrow::ui::ExportConnections);
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|suggested| {
        assert!(suggested.is_dir(), "{suggested:?}");
        Some(path.clone())
    });
    app.wait_until(cx, "the exported file", WAIT, |_, _| path.exists());
    app.wait_for(cx, "connections-exported-title");
    app.update(cx, |window, _| {
        assert_eq!(
            window.find("connections-exported-title").label(),
            Some("Exported 2 connections without passwords")
        );
    });
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains(PASSWORD), "{text}");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["qrow_connections"], 1);
    assert_eq!(json["connections"].as_array().unwrap().len(), 2);

    // Import into the same workspace: new names, and the same sign-in.
    app.dispatch(cx, qrow::ui::ImportConnections);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(!options.multiple && options.files && !options.directories);
        Some(vec![path.clone()])
    });
    app.wait_until(cx, "the imported connections", WAIT, |_, _| {
        app.saved().profiles.len() == 4
    });
    assert_eq!(
        toast_detail(&app, cx, "connections-imported-toast"),
        "Edit each connection to enter its password."
    );
    let saved = app.saved();
    let names: Vec<_> = saved.profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Warehouse", "Lake", "Warehouse copy", "Lake copy"]);
    assert_eq!(saved.sign_ins.len(), 1);
    assert_eq!(
        saved.profiles[3].authentication,
        Authentication::Oidc { sign_in: existing }
    );
    assert!(app.credentials.get(saved.profiles[2].id).is_none());
    // Each imported connection has a query tab, so that it can be selected.
    for profile in &saved.profiles[2..] {
        assert!(
            saved.tabs.iter().any(|tab| tab.profile == Some(profile.id)),
            "{}",
            profile.name
        );
    }
    app.click(cx, connection_row(saved.profiles[3].id));
    app.wait_until(cx, "the imported connection", WAIT, |window, _| {
        label(window, "current-connection").as_deref() == Some("Current connection: Lake copy")
    });
}

#[gpui_kit::test]
fn imported_connections_fill_an_empty_workspace(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.json");
    let sign_in = sign_in();
    let lake = qrow::model::Profile {
        authentication: Authentication::Oidc {
            sign_in: sign_in.id,
        },
        ..offline_profile("Lake")
    };
    std::fs::write(
        &path,
        qrow::model::sharing::connections_file_text([(
            &lake,
            lake.catalog.clone(),
            Some(&sign_in),
        )]),
    )
    .unwrap();
    app.dispatch(cx, qrow::ui::ImportConnections);
    cx.simulate_path_prompt_response(|_| Some(vec![path.clone()]));
    app.wait_until(cx, "the imported connection", WAIT, |_, _| {
        app.saved().profiles.len() == 1
    });
    let saved = app.saved();
    assert_eq!(saved.sign_ins.len(), 1);
    assert_eq!(saved.sign_ins[0].name, "Company");
    let id = saved.profiles[0].id;
    assert_eq!(
        saved.profiles[0].authentication,
        Authentication::Oidc {
            sign_in: saved.sign_ins[0].id
        }
    );
    // The query tab that had no connection uses the imported one.
    assert!(saved.tabs.iter().all(|tab| tab.profile == Some(id)));
    app.wait_for(cx, "connections-imported-toast");
    app.update(cx, |window, _| {
        // A sign-in connection needs no password.
        assert!(
            window
                .within("connections-imported-toast")
                .try_find("detail")
                .is_none()
        );
    });
}

#[gpui_kit::test]
fn an_invalid_file_imports_nothing_and_says_why(cx: &mut TestAppContext) {
    let (workspace, credentials) = workspace();
    let app = TestApp::launch_with(cx, workspace, credentials);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.json");
    std::fs::write(
        &path,
        r#"{"qrow_connections": 1, "connections": [
            {"name": "Good", "host": "h", "port": 1, "username": "u", "database": "d"},
            {"name": "Bad", "host": "h", "port": 1, "username": "", "database": "d"}
        ]}"#,
    )
    .unwrap();
    app.dispatch(cx, qrow::ui::ImportConnections);
    cx.simulate_path_prompt_response(|_| Some(vec![path.clone()]));
    let detail = toast_detail(&app, cx, "connections-file-toast");
    assert!(detail.starts_with("Connection 2 \"Bad\": "), "{detail}");
    assert_eq!(app.saved().profiles.len(), 2);
}

#[gpui_kit::test]
fn export_without_connections_says_why(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    app.dispatch(cx, qrow::ui::ExportConnections);
    assert_eq!(
        toast_detail(&app, cx, "connections-file-toast"),
        "Add a connection first."
    );
    assert!(!cx.did_prompt_for_new_path());
}

#[gpui_kit::test]
fn an_imported_trino_connection_keeps_an_empty_password(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.json");
    let trino = qrow::model::Profile {
        database_type: qrow::model::DatabaseType::Trino,
        port: 8080,
        ..offline_profile("Trino")
    };
    let postgres = qrow::model::Profile {
        database_type: qrow::model::DatabaseType::Postgres,
        port: 5432,
        ..offline_profile("Postgres")
    };
    std::fs::write(
        &path,
        qrow::model::sharing::connections_file_text([
            (&trino, trino.catalog.clone(), None),
            (&postgres, postgres.catalog.clone(), None),
        ]),
    )
    .unwrap();
    app.dispatch(cx, qrow::ui::ImportConnections);
    cx.simulate_path_prompt_response(|_| Some(vec![path.clone()]));
    app.wait_until(cx, "the imported connections", WAIT, |_, _| {
        app.saved().profiles.len() == 2
    });
    let saved = app.saved();
    app.wait_until(cx, "the empty Trino password", WAIT, |_, _| {
        app.credentials.get(saved.profiles[0].id).as_deref() == Some("")
    });
    // Postgres needs a password, so it has none until the user enters it.
    assert!(app.credentials.get(saved.profiles[1].id).is_none());
}
