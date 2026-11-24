use crate::support::{MemoryCredentials, TestApp, bounds_of, label};
use gpui_kit::{TestAppContext, px, test::TestWindowExt as _};
use qrow::storage;
use std::time::Duration;

fn registry_json(app: &TestApp) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(storage::registry_path(app.data_dir())).unwrap()).unwrap()
}

#[gpui_kit::test]
fn create_switch_rename_duplicate_and_delete_workspaces_in_single_window(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::launch_store(cx, dir, MemoryCredentials::default());

    // Initial Default workspace exists and global settings work.
    let initial_reg = registry_json(&app);
    assert_eq!(initial_reg["workspaces"].as_array().unwrap().len(), 1);
    assert_eq!(initial_reg["workspaces"][0]["name"], "Default");
    let default_id =
        uuid::Uuid::parse_str(initial_reg["active_workspace"].as_str().unwrap()).unwrap();

    // Type SQL in Default workspace and change a global setting (toggle results minimized).
    app.click(cx, "sql-editor");
    app.update(cx, |window, cx| {
        window.within("sql-editor").input("SELECT 'in_default'", cx);
    });
    app.press(cx, "secondary-j");

    // Create a second workspace via NewWorkspace action.
    app.dispatch(cx, qrow::ui::NewWorkspace);
    app.wait_for(cx, "workspace-name-input");
    app.fill(cx, "workspace-name-input", "Analytics");
    app.click(cx, "submit-workspace-name");

    // Qrow switched into Analytics automatically; global setting stayed minimized,
    // while SQL editor is isolated and blank.
    app.wait_until(
        cx,
        "switch to Analytics",
        Duration::from_secs(10),
        |_, _| {
            let reg = registry_json(&app);
            reg["workspaces"].as_array().unwrap().len() == 2
                && reg["active_workspace"] != default_id.to_string()
        },
    );
    let reg = registry_json(&app);
    let analytics_id = uuid::Uuid::parse_str(reg["active_workspace"].as_str().unwrap()).unwrap();

    // Default workspace kept its SQL on disk when switching out.
    let default_saved =
        storage::load(&storage::workspace_file_path(app.data_dir(), default_id)).unwrap();
    assert_eq!(default_saved.tabs[0].sql, "SELECT 'in_default'");
    // Global setting persisted in settings.json.
    let settings = storage::load_settings(&storage::settings_path(app.data_dir())).unwrap();
    assert!(settings.results_minimized);

    // Type distinct SQL in Analytics.
    app.click(cx, "sql-editor");
    app.update(cx, |window, cx| {
        window
            .within("sql-editor")
            .input("SELECT 'in_analytics'", cx);
    });

    // Open Manage Workspaces, duplicate Analytics, rename the duplicate, and delete it.
    app.dispatch(cx, qrow::ui::ManageWorkspaces);
    app.wait_for(cx, "manage-workspaces-list");
    // Pressing Escape on an open actions menu dismisses only the menu.
    app.click(cx, format!("workspace-actions-{analytics_id}"));
    app.wait_for(cx, "popup-menu");
    app.press(cx, "escape");
    app.wait_gone(cx, "popup-menu");
    app.wait_for(cx, "manage-workspaces-list");

    app.click(cx, format!("workspace-actions-{analytics_id}"));
    app.choose(cx, "popup-menu", "Duplicate");

    app.wait_until(
        cx,
        "Analytics copy created",
        Duration::from_secs(10),
        |_, _| registry_json(&app)["workspaces"].as_array().unwrap().len() == 3,
    );
    let reg = registry_json(&app);
    let copy_entry = reg["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "Analytics copy")
        .expect("duplicated workspace entry");
    let copy_id = uuid::Uuid::parse_str(copy_entry["id"].as_str().unwrap()).unwrap();

    // Duplicated workspace includes unsaved live SQL from active Analytics workspace.
    let copy_saved = storage::load(&storage::workspace_file_path(app.data_dir(), copy_id)).unwrap();
    assert_eq!(copy_saved.tabs[0].sql, "SELECT 'in_analytics'");

    // Rename the duplicate to "Staging".
    app.click(cx, format!("workspace-actions-{copy_id}"));
    app.choose(cx, "popup-menu", "Rename…");
    app.wait_for(cx, "workspace-name-input");
    app.fill(cx, "workspace-name-input", "Staging");
    app.click(cx, "submit-workspace-name");
    app.wait_until(cx, "renamed to Staging", Duration::from_secs(10), |_, _| {
        registry_json(&app)["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == copy_id.to_string() && e["name"] == "Staging")
    });

    // Manage Workspaces reopens automatically after rename; delete inactive Staging.
    app.wait_for(cx, "manage-workspaces-list");
    app.click(cx, format!("workspace-actions-{copy_id}"));
    app.choose(cx, "popup-menu", "Delete…");
    app.wait_for(cx, "confirm-delete-workspace");
    app.click(cx, "confirm-delete-workspace");
    app.wait_until(cx, "Staging deleted", Duration::from_secs(10), |_, _| {
        registry_json(&app)["workspaces"].as_array().unwrap().len() == 2
    });
    assert!(!storage::workspace_directory(app.data_dir(), copy_id).exists());

    // Select Default card and click the footer Switch button to switch workspaces.
    app.wait_for(cx, "manage-workspaces-switch");
    app.click(cx, format!("workspace-row-{default_id}"));
    app.click(cx, "manage-workspaces-switch");
    app.wait_until(
        cx,
        "switched back to Default",
        Duration::from_secs(10),
        |_, _| registry_json(&app)["active_workspace"] == default_id.to_string(),
    );
    let analytics_saved =
        storage::load(&storage::workspace_file_path(app.data_dir(), analytics_id)).unwrap();
    assert_eq!(analytics_saved.tabs[0].sql, "SELECT 'in_analytics'");

    // Delete the active workspace (Default): switches to Analytics and removes Default.
    app.dispatch(cx, qrow::ui::ManageWorkspaces);
    app.wait_for(cx, "manage-workspaces-list");
    app.click(cx, format!("workspace-actions-{default_id}"));
    app.choose(cx, "popup-menu", "Delete…");
    app.wait_for(cx, "confirm-delete-workspace");
    app.click(cx, "confirm-delete-workspace");
    app.wait_until(
        cx,
        "active workspace deleted and switched",
        Duration::from_secs(10),
        |_, _| {
            let reg = registry_json(&app);
            reg["workspaces"].as_array().unwrap().len() == 1
                && reg["active_workspace"] == analytics_id.to_string()
        },
    );
    assert!(!storage::workspace_directory(app.data_dir(), default_id).exists());
}

#[gpui_kit::test]
fn workspace_name_dialog_validates_empty_and_duplicate_names(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::launch_store(cx, dir, MemoryCredentials::default());

    app.dispatch(cx, qrow::ui::NewWorkspace);
    app.wait_for(cx, "workspace-name-input");

    // Empty name is rejected inline.
    app.click(cx, "submit-workspace-name");
    app.wait_for(cx, "workspace-name-error");
    assert_eq!(
        registry_json(&app)["workspaces"].as_array().unwrap().len(),
        1
    );

    // Duplicate name (case-insensitive) is rejected inline.
    app.fill(cx, "workspace-name-input", "default");
    app.click(cx, "submit-workspace-name");
    app.wait_for(cx, "workspace-name-error");
    assert_eq!(
        registry_json(&app)["workspaces"].as_array().unwrap().len(),
        1
    );

    // Valid name succeeds and closes the dialog.
    app.fill(cx, "workspace-name-input", "Reporting");
    app.click(cx, "submit-workspace-name");
    app.wait_until(cx, "Reporting created", Duration::from_secs(10), |_, _| {
        registry_json(&app)["workspaces"].as_array().unwrap().len() == 2
    });
}

#[gpui_kit::test]
fn switching_workspace_while_export_runs_asks_for_confirmation(cx: &mut TestAppContext) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::launch_store(cx, dir, MemoryCredentials::default());
    let default_id =
        uuid::Uuid::parse_str(registry_json(&app)["active_workspace"].as_str().unwrap()).unwrap();

    // Create and switch into a second workspace.
    app.dispatch(cx, qrow::ui::NewWorkspace);
    app.wait_for(cx, "workspace-name-input");
    app.fill(cx, "workspace-name-input", "Analytics");
    app.click(cx, "submit-workspace-name");
    app.wait_until(cx, "Analytics active", Duration::from_secs(10), |_, _| {
        registry_json(&app)["active_workspace"] != default_id.to_string()
    });
    let analytics_id =
        uuid::Uuid::parse_str(registry_json(&app)["active_workspace"].as_str().unwrap()).unwrap();

    // Register a running export job and attempt to switch back to Default.
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = cx.update(|cx| cx.global::<qrow::export::Jobs>().register(cancel.clone()));
    app.dispatch(cx, qrow::ui::SwitchToWorkspace(default_id));
    app.wait_for(cx, "keep-working-switch");
    app.click(cx, "keep-working-switch");
    app.wait_gone(cx, "keep-working-switch");
    assert!(!cancel.load(Ordering::Relaxed));
    assert_eq!(
        registry_json(&app)["active_workspace"],
        analytics_id.to_string()
    );

    // Confirming "Switch anyway" cancels the export and switches to Default.
    let worker_cancel = cancel.clone();
    let worker = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !worker_cancel.load(Ordering::Relaxed) {
            assert!(
                std::time::Instant::now() < deadline,
                "Workspace switch did not cancel the export"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(guard);
    });
    app.dispatch(cx, qrow::ui::SwitchToWorkspace(default_id));
    app.wait_for(cx, "switch-anyway");
    app.click(cx, "switch-anyway");
    worker.join().unwrap();
    assert!(cancel.load(Ordering::Relaxed));
    app.wait_until(
        cx,
        "switched to Default",
        Duration::from_secs(10),
        |_, _| registry_json(&app)["active_workspace"] == default_id.to_string(),
    );
}

#[gpui_kit::test]
fn failed_target_open_keeps_current_workspace_active_and_writable(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::launch_store(cx, dir, MemoryCredentials::default());
    let default_id =
        uuid::Uuid::parse_str(registry_json(&app)["active_workspace"].as_str().unwrap()).unwrap();

    // Create a second workspace and switch back to Default.
    app.dispatch(cx, qrow::ui::NewWorkspace);
    app.wait_for(cx, "workspace-name-input");
    app.fill(cx, "workspace-name-input", "Broken");
    app.click(cx, "submit-workspace-name");
    app.wait_until(cx, "Broken active", Duration::from_secs(10), |_, _| {
        registry_json(&app)["active_workspace"] != default_id.to_string()
    });
    let broken_id =
        uuid::Uuid::parse_str(registry_json(&app)["active_workspace"].as_str().unwrap()).unwrap();
    app.dispatch(cx, qrow::ui::SwitchToWorkspace(default_id));
    app.wait_until(cx, "back in Default", Duration::from_secs(10), |_, _| {
        registry_json(&app)["active_workspace"] == default_id.to_string()
    });

    // Corrupt the target workspace on disk and attempt to switch to it.
    std::fs::write(
        storage::workspace_file_path(app.data_dir(), broken_id),
        "{not valid json",
    )
    .unwrap();
    app.dispatch(cx, qrow::ui::SwitchToWorkspace(broken_id));
    app.settle(cx);

    // Default remains active and edits still persist cleanly to disk.
    assert_eq!(
        registry_json(&app)["active_workspace"],
        default_id.to_string()
    );
    app.click(cx, "sql-editor");
    app.update(cx, |window, cx| {
        window
            .within("sql-editor")
            .input("SELECT 'still_writable'", cx);
    });
    app.wait_until(
        cx,
        "Default still persists after failed switch",
        Duration::from_secs(10),
        |_, _| {
            storage::load(&storage::workspace_file_path(app.data_dir(), default_id))
                .ok()
                .and_then(|ws| ws.tabs.first().map(|t| t.sql.clone()))
                .as_deref()
                == Some("SELECT 'still_writable'")
        },
    );
}

#[gpui_kit::test]
fn dismissing_delete_confirmation_with_escape_reopens_manager(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::launch_store(cx, dir, MemoryCredentials::default());

    app.dispatch(cx, qrow::ui::NewWorkspace);
    app.wait_for(cx, "workspace-name-input");
    app.fill(cx, "workspace-name-input", "Extra");
    app.click(cx, "submit-workspace-name");
    app.wait_until(cx, "Extra created", Duration::from_secs(10), |_, _| {
        registry_json(&app)["workspaces"].as_array().unwrap().len() == 2
    });
    let extra_id =
        uuid::Uuid::parse_str(registry_json(&app)["active_workspace"].as_str().unwrap()).unwrap();

    app.dispatch(cx, qrow::ui::ManageWorkspaces);
    app.wait_for(cx, "manage-workspaces-list");
    app.click(cx, format!("workspace-actions-{extra_id}"));
    app.choose(cx, "popup-menu", "Delete…");
    app.wait_for(cx, "confirm-delete-workspace");

    // Pressing Escape dismisses the confirmation and returns keyboard focus to
    // Manage Workspaces so a second Escape closes Manage Workspaces.
    app.press(cx, "escape");
    app.wait_gone(cx, "confirm-delete-workspace");
    app.wait_for(cx, "manage-workspaces-list");
    assert_eq!(
        registry_json(&app)["workspaces"].as_array().unwrap().len(),
        2
    );

    // Clicking Cancel on Delete also returns keyboard focus to Manage Workspaces.
    app.click(cx, format!("workspace-actions-{extra_id}"));
    app.choose(cx, "popup-menu", "Delete…");
    app.wait_for(cx, "cancel-delete-workspace");
    app.click(cx, "cancel-delete-workspace");
    app.wait_gone(cx, "cancel-delete-workspace");
    app.wait_for(cx, "manage-workspaces-list");
    app.press(cx, "escape");
    app.wait_gone(cx, "manage-workspaces-list");

    // Reopen and verify Export dialog Escape and Cancel both return focus to Manage Workspaces.
    app.dispatch(cx, qrow::ui::ManageWorkspaces);
    app.wait_for(cx, "manage-workspaces-list");
    app.click(cx, format!("workspace-actions-{extra_id}"));
    app.choose(cx, "popup-menu", "Export…");
    app.wait_for(cx, "export-workspace-dialog");
    app.press(cx, "escape");
    app.wait_gone(cx, "export-workspace-dialog");
    app.wait_for(cx, "manage-workspaces-list");

    app.click(cx, format!("workspace-actions-{extra_id}"));
    app.choose(cx, "popup-menu", "Export…");
    app.wait_for(cx, "export-workspace-dialog");
    app.click(cx, "export-workspace-cancel");
    app.wait_gone(cx, "export-workspace-dialog");
    app.wait_for(cx, "manage-workspaces-list");

    // Second Escape closes Manage Workspaces cleanly.
    app.press(cx, "escape");
    app.wait_gone(cx, "manage-workspaces-list");
}

#[gpui_kit::test]
fn manage_workspaces_supports_keyboard_selection_and_enter_to_switch(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::launch_store(cx, dir, MemoryCredentials::default());
    let default_id =
        uuid::Uuid::parse_str(registry_json(&app)["active_workspace"].as_str().unwrap()).unwrap();

    // Create a second workspace ("Analytics"); it becomes active.
    app.dispatch(cx, qrow::ui::NewWorkspace);
    app.wait_for(cx, "workspace-name-input");
    app.fill(cx, "workspace-name-input", "Analytics");
    app.click(cx, "submit-workspace-name");
    app.wait_until(cx, "Analytics active", Duration::from_secs(10), |_, _| {
        registry_json(&app)["active_workspace"] != default_id.to_string()
    });

    // Open Manage Workspaces (Analytics is selected at index 1). Pressing Up
    // selects Default (index 0) and Enter switches back to Default.
    app.dispatch(cx, qrow::ui::ManageWorkspaces);
    app.wait_for(cx, "manage-workspaces-list");
    app.press(cx, "up");
    app.press(cx, "enter");
    app.wait_until(
        cx,
        "keyboard switch back to Default",
        Duration::from_secs(10),
        |_, _| registry_json(&app)["active_workspace"] == default_id.to_string(),
    );
    app.wait_gone(cx, "manage-workspaces-list");
}

#[gpui_kit::test]
fn workspace_layout_persists_per_workspace_across_switches(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::launch_store(cx, dir, MemoryCredentials::default());
    let default_id =
        uuid::Uuid::parse_str(registry_json(&app)["active_workspace"].as_str().unwrap()).unwrap();

    // Hide the sidebar in Default workspace.
    app.dispatch(cx, qrow::ui::ToggleSidebar);
    app.wait_until(
        cx,
        "Default sidebar_open=false saved",
        Duration::from_secs(10),
        |_, _| {
            storage::load(&storage::workspace_file_path(app.data_dir(), default_id))
                .ok()
                .and_then(|ws| ws.layout)
                .is_some_and(|layout| !layout.sidebar_open)
        },
    );

    // Create a second workspace: its layout starts with sidebar open.
    app.dispatch(cx, qrow::ui::NewWorkspace);
    app.wait_for(cx, "workspace-name-input");
    app.fill(cx, "workspace-name-input", "LayoutB");
    app.click(cx, "submit-workspace-name");
    app.wait_until(cx, "LayoutB active", Duration::from_secs(10), |_, _| {
        registry_json(&app)["active_workspace"] != default_id.to_string()
    });
    app.wait_for(cx, "connections-list");

    // Switch back to Default: its saved layout restores (sidebar remains closed).
    app.dispatch(cx, qrow::ui::SwitchToWorkspace(default_id));
    app.wait_until(
        cx,
        "Default active again",
        Duration::from_secs(10),
        |_, _| registry_json(&app)["active_workspace"] == default_id.to_string(),
    );
    let default_ws =
        storage::load(&storage::workspace_file_path(app.data_dir(), default_id)).unwrap();
    assert!(default_ws.layout.is_some_and(|l| !l.sidebar_open));
}

#[gpui_kit::test]
fn export_import_and_backup_workspace_workflow(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let export_dir = tempfile::tempdir().unwrap();
    let app = TestApp::launch_store(cx, dir, MemoryCredentials::default());
    let default_id =
        uuid::Uuid::parse_str(registry_json(&app)["active_workspace"].as_str().unwrap()).unwrap();

    // Put distinct SQL into Default workspace so we can verify export/import/backup contents.
    app.click(cx, "sql-editor");
    app.update(cx, |window, cx| {
        window
            .within("sql-editor")
            .input("SELECT 'portable_bundle'", cx);
    });

    // Export Default workspace to a portable `.qrow-workspace.json` bundle
    // with saved SQL explicitly opted in.
    let bundle_path = export_dir.path().join("Default.qrow-workspace.json");
    let bundle_for_export = bundle_path.clone();
    app.update(cx, |window, cx| {
        app.qrow
            .update(cx, |qrow, cx| {
                qrow.export_workspace_to_path_with_options(
                    default_id,
                    bundle_for_export,
                    qrow::model::workspace_export::ExportOptions {
                        include_sql: true,
                        include_parameters: false,
                        include_assistant_notes: false,
                    },
                    window,
                    cx,
                );
            })
            .unwrap();
    });
    app.wait_for(cx, "workspace-exported-toast");
    let exported_text = std::fs::read_to_string(&bundle_path).unwrap();
    assert!(exported_text.contains("\"qrow_workspace\": 1"));
    assert!(exported_text.contains("SELECT 'portable_bundle'"));
    assert!(!exported_text.contains("\"settings\""));

    // Import that bundle back: creates "Default copy" and switches into it.
    app.update(cx, |window, cx| {
        app.qrow
            .update(cx, |qrow, cx| {
                qrow.import_workspace_from_path(bundle_path, false, window, cx);
            })
            .unwrap();
    });
    app.wait_until(
        cx,
        "imported workspace active",
        Duration::from_secs(10),
        |_, _| {
            let reg = registry_json(&app);
            reg["workspaces"].as_array().unwrap().len() == 2
                && reg["active_workspace"] != default_id.to_string()
        },
    );
    let reg = registry_json(&app);
    let imported_id = uuid::Uuid::parse_str(reg["active_workspace"].as_str().unwrap()).unwrap();
    let imported_ws =
        storage::load(&storage::workspace_file_path(app.data_dir(), imported_id)).unwrap();
    assert_eq!(imported_ws.tabs[0].sql, "SELECT 'portable_bundle'");

    // Back up the active workspace to an explicit destination file.
    let backup_path = export_dir.path().join("backup.json");
    let backup_for_save = backup_path.clone();
    app.update(cx, |window, cx| {
        app.qrow
            .update(cx, |qrow, cx| {
                qrow.backup_workspace_to_path(imported_id, backup_for_save, window, cx);
            })
            .unwrap();
    });
    app.wait_for(cx, "workspace-backup-toast");
    let backup_ws = storage::load(&backup_path).unwrap();
    assert_eq!(backup_ws.tabs[0].sql, "SELECT 'portable_bundle'");
}

#[gpui_kit::test]
fn export_workspace_dialog_shows_single_line_option_checkboxes(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::launch_store(cx, dir, MemoryCredentials::default());

    app.dispatch(cx, qrow::ui::ExportWorkspace);
    app.wait_for(cx, "export-workspace-dialog");

    let checkboxes = [
        ("export-workspace-include-sql", "Include saved SQL"),
        (
            "export-workspace-include-parameters",
            "Include session parameters",
        ),
        ("export-workspace-include-notes", "Include assistant notes"),
    ];

    app.update(cx, |window, _| {
        // Small checkboxes use 0.875 rem text with 1.25 relative line height,
        // without secondary description lines underneath.
        let single_line = window.rem_size() * 0.875 * 1.25;
        for (id, expected_label) in checkboxes {
            assert_eq!(label(window, id).as_deref(), Some(expected_label));
            let height = bounds_of(window, id).size.height;
            assert!(
                (height - single_line).abs() <= px(2.),
                "{id}: height {height:?} should match a single label line (~{single_line:?})"
            );
        }
    });

    for (id, _) in checkboxes {
        app.click(cx, id);
    }
    app.press(cx, "escape");
    app.wait_gone(cx, "export-workspace-dialog");
}
