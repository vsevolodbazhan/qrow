use qrow::{
    model::{Profile, Workspace},
    storage::{self, WorkspaceFile},
    workspaces::{self, Catalog},
};
use uuid::Uuid;

#[test]
fn preserves_legacy_workspace_and_restores_selection_with_isolated_state() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut original = Workspace::default();
    original.profiles.push(Profile {
        name: "Synthetic profile".into(),
        ..Profile::default()
    });
    original.tabs[0].profile = Some(original.profiles[0].id);
    original.normalize();
    original.tabs[0].sql = "SELECT 'default 日本語😀'".into();
    WorkspaceFile::acquire(root.join("workspace.json"))
        .unwrap()
        .save(&original)
        .unwrap();
    let (catalog, loaded, mut saver) = workspaces::open(root, None, None, || {}).unwrap();
    assert_eq!(loaded, original);
    assert_eq!(catalog.active().unwrap().name, "Default");
    saver.finish(loaded).unwrap();
    let (catalog, mut created, mut saver) =
        workspaces::open(root, None, Some(" Analytics / 日本語 "), || {}).unwrap();
    assert!(created.profiles.is_empty());
    assert!(created.tabs[0].sql.is_empty());
    assert_eq!(catalog.active().unwrap().name, "Analytics / 日本語");
    let id = catalog.active().unwrap().id;
    created.tabs[0].sql = "SELECT 'another workspace'".into();
    created.settings.ui_scale = 1.25;
    saver.finish(created.clone()).unwrap();
    let (restored, loaded, mut saver) = workspaces::open(root, None, None, || {}).unwrap();
    assert_eq!(restored.active().unwrap().id, id);
    assert_eq!(loaded, created);
    saver.finish(loaded).unwrap();
    let (_, loaded, mut saver) = workspaces::open(root, Some(Uuid::nil()), None, || {}).unwrap();
    assert_eq!(loaded, original);
    saver.finish(loaded).unwrap();
    assert_eq!(
        storage::load(&catalog.path(root, id).unwrap()).unwrap(),
        created
    );
    let renamed = workspaces::rename(root, Uuid::nil(), "Original").unwrap();
    assert_eq!(renamed.active().unwrap().name, "Original");
    assert_eq!(
        renamed.path(root, Uuid::nil()).unwrap(),
        root.join("workspace.json")
    );
    let (restored, loaded, mut saver) = workspaces::restore(root, || {}).unwrap().unwrap();
    assert_eq!(restored.active().unwrap().name, "Original");
    assert_eq!(loaded, original);
    saver.finish(loaded).unwrap();
}

#[test]
fn rejects_invalid_names_without_changing_selection_or_list() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["", "  ", "a\nb", &"a".repeat(81)] {
        assert!(workspaces::open(dir.path(), None, Some(name), || {}).is_err());
        assert_eq!(Catalog::load(dir.path()).unwrap().entries().len(), 0);
        assert!(!dir.path().join("workspaces.json").exists());
    }
}

#[test]
fn locked_corrupt_and_missing_destinations_preserve_selection() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    WorkspaceFile::acquire(root.join("workspace.json"))
        .unwrap()
        .save(&Workspace::default())
        .unwrap();
    let (catalog, state, mut saver) = workspaces::open(root, None, Some("Second"), || {}).unwrap();
    let id = catalog.active().unwrap().id;
    saver.finish(state).unwrap();
    let (_, state, mut source) = workspaces::open(root, Some(Uuid::nil()), None, || {}).unwrap();
    let path = catalog.path(root, id).unwrap();
    let lock = WorkspaceFile::acquire(path.clone()).unwrap();
    assert!(workspaces::open(root, Some(id), None, || {}).is_err());
    assert!(Catalog::load(root).unwrap().active().unwrap().id.is_nil());
    drop(lock);
    std::fs::write(&path, "broken").unwrap();
    assert!(workspaces::open(root, Some(id), None, || {}).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken");
    std::fs::remove_file(&path).unwrap();
    assert!(workspaces::open(root, Some(id), None, || {}).is_err());
    assert!(Catalog::load(root).unwrap().active().unwrap().id.is_nil());
    source.finish(state).unwrap();
}

#[test]
fn invalid_catalog_is_not_overwritten_and_unknown_ids_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    assert!(workspaces::open(dir.path(), Some(Uuid::new_v4()), None, || {}).is_err());
    let path = dir.path().join("workspaces.json");
    std::fs::write(&path, "broken").unwrap();
    assert!(workspaces::open(dir.path(), None, None, || {}).is_err());
    assert!(workspaces::rename(dir.path(), Uuid::nil(), "Renamed").is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "broken");
}

#[test]
fn fresh_install_has_no_workspace_until_created_and_remembers_last_opened() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(Catalog::load(root).unwrap().entries().is_empty());
    assert!(workspaces::restore(root, || {}).unwrap().is_none());
    assert!(!root.join("workspaces.json").exists());
    assert!(!root.join("workspace.json").exists());
    let (first, state, mut saver) = workspaces::open(root, None, Some("First"), || {}).unwrap();
    let first_id = first.active().unwrap().id;
    saver.finish(state).unwrap();
    let (second, state, mut saver) = workspaces::open(root, None, Some("Default"), || {}).unwrap();
    assert_eq!(second.entries().len(), 2);
    assert!(!second.active().unwrap().id.is_nil());
    saver.finish(state).unwrap();
    let (_, state, mut saver) = workspaces::open(root, Some(first_id), None, || {}).unwrap();
    saver.finish(state).unwrap();
    let (restored, state, mut saver) = workspaces::restore(root, || {}).unwrap().unwrap();
    assert_eq!(restored.active().unwrap().id, first_id);
    saver.finish(state).unwrap();
}

#[test]
fn rename_preserves_identity_files_and_selection_and_rejects_invalid_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (catalog, mut state, mut saver) =
        workspaces::open(root, None, Some("First"), || {}).unwrap();
    let id = catalog.active().unwrap().id;
    let path = catalog.path(root, id).unwrap();
    state.tabs[0].sql = "SELECT 'rename keeps SQL'".into();
    saver.finish(state.clone()).unwrap();
    let (second, other, mut saver) = workspaces::open(root, None, Some("Other"), || {}).unwrap();
    let active = second.active().unwrap().id;
    let renamed = workspaces::rename(root, id, "  Renamed 日本語😀  ").unwrap();
    assert_eq!(renamed.active().unwrap().id, active);
    assert_eq!(renamed.entries()[0].name, "Renamed 日本語😀");
    assert_eq!(renamed.path(root, id).unwrap(), path);
    assert_eq!(storage::load(&path).unwrap(), state);
    for invalid in ["", "other", "a\nb", &"a".repeat(81)] {
        assert!(workspaces::rename(root, id, invalid).is_err());
        assert_eq!(Catalog::load(root).unwrap(), renamed);
    }
    assert!(workspaces::rename(root, Uuid::new_v4(), "Missing").is_err());
    assert!(workspaces::open(root, None, Some("OTHER"), || {}).is_err());
    workspaces::rename(root, active, "OTHER").unwrap();
    saver.finish(other).unwrap();
    let (restored, other, mut saver) = workspaces::restore(root, || {}).unwrap().unwrap();
    assert_eq!(restored.active().unwrap().name, "OTHER");
    saver.finish(other).unwrap();
}

fn create_workspace(root: &std::path::Path, name: &str) -> Uuid {
    let (catalog, mut workspace, mut saver) =
        workspaces::open(root, None, Some(name), || {}).unwrap();
    workspace.tabs[0].sql = format!("SELECT '{name}'");
    saver.finish(workspace).unwrap();
    catalog.active().unwrap().id
}

#[test]
fn deletion_selects_most_recent_workspace_and_last_deletion_stays_empty_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let first = create_workspace(root, "First");
    let second = create_workspace(root, "Second");
    let third = create_workspace(root, "Third");
    let (_, workspace, mut saver) = workspaces::open(root, Some(first), None, || {}).unwrap();
    saver.finish(workspace).unwrap();
    let (catalog, workspace, mut source) =
        workspaces::open(root, Some(third), None, || {}).unwrap();
    source.flush(workspace).unwrap().recv().unwrap().unwrap();
    let path = catalog.path(root, third).unwrap();
    let deleted =
        workspaces::delete(root, third, |_| panic!("No passwords expected"), || {}).unwrap();
    assert!(deleted.warning.is_none());
    assert!(!path.exists());
    assert_eq!(deleted.catalog.active().unwrap().id, first);
    assert!(!deleted.catalog.entries().iter().any(|e| e.id == third));
    source.stop().unwrap();
    let (workspace, mut saver) = deleted.next.unwrap();
    assert_eq!(workspace.tabs[0].sql, "SELECT 'First'");
    saver.finish(workspace).unwrap();
    let (_, workspace, mut source) = workspaces::restore(root, || {}).unwrap().unwrap();
    source.flush(workspace).unwrap().recv().unwrap().unwrap();
    let deleted = workspaces::delete(root, first, |_| Ok(()), || {}).unwrap();
    source.stop().unwrap();
    assert_eq!(deleted.catalog.active().unwrap().id, second);
    let (workspace, mut source) = deleted.next.unwrap();
    source.flush(workspace).unwrap().recv().unwrap().unwrap();
    let deleted = workspaces::delete(root, second, |_| Ok(()), || {}).unwrap();
    source.stop().unwrap();
    assert!(deleted.next.is_none());
    assert!(deleted.catalog.entries().is_empty());
    assert!(workspaces::restore(root, || {}).unwrap().is_none());
    assert!(!root.join("workspace.json").exists());
    let new = create_workspace(root, "First");
    assert_ne!(new, first);
    assert_eq!(Catalog::load(root).unwrap().entries().len(), 1);
}

#[test]
fn failed_deletion_preserves_source_and_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let first = create_workspace(root, "First");
    let second = create_workspace(root, "Second");
    let (catalog, workspace, mut source) = workspaces::restore(root, || {}).unwrap().unwrap();
    source
        .flush(workspace.clone())
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    let first_path = catalog.path(root, first).unwrap();
    let source_path = catalog.path(root, second).unwrap();
    let lock = WorkspaceFile::acquire(first_path.clone()).unwrap();
    assert!(
        workspaces::delete(
            root,
            second,
            |_| panic!("Do not delete passwords on failure"),
            || {}
        )
        .is_err()
    );
    assert_eq!(Catalog::load(root).unwrap(), catalog);
    assert_eq!(storage::load(&source_path).unwrap(), workspace);
    drop(lock);
    let bytes = std::fs::read(&first_path).unwrap();
    std::fs::write(&first_path, "broken").unwrap();
    assert!(workspaces::delete(root, second, |_| Ok(()), || {}).is_err());
    assert_eq!(Catalog::load(root).unwrap(), catalog);
    std::fs::remove_file(&first_path).unwrap();
    assert!(workspaces::delete(root, second, |_| Ok(()), || {}).is_err());
    std::fs::write(&first_path, bytes).unwrap();
    assert!(workspaces::delete(root, first, |_| Ok(()), || {}).is_err());
    let lock = WorkspaceFile::acquire(root.join("workspaces.json")).unwrap();
    assert!(workspaces::delete(root, second, |_| Ok(()), || {}).is_err());
    drop(lock);
    assert_eq!(Catalog::load(root).unwrap(), catalog);
    source.finish(workspace).unwrap();
}

#[test]
fn interrupted_cleanup_retries_password_removal_without_restoring_legacy_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut workspace = Workspace::default();
    let profile = Profile::default();
    let id = profile.id;
    workspace.profiles.push(profile);
    WorkspaceFile::acquire(root.join("workspace.json"))
        .unwrap()
        .save(&workspace)
        .unwrap();
    let (_, workspace, mut source) = workspaces::restore(root, || {}).unwrap().unwrap();
    source.flush(workspace).unwrap().recv().unwrap().unwrap();
    let deleted = workspaces::delete(
        root,
        Uuid::nil(),
        |candidate| {
            assert_eq!(candidate, id);
            anyhow::bail!("Synthetic Keychain failure")
        },
        || {},
    )
    .unwrap();
    assert!(deleted.warning.unwrap().contains("cleanup is incomplete"));
    assert!(deleted.catalog.entries().is_empty());
    assert!(deleted.next.is_none());
    assert!(!root.join("workspace.json").exists());
    // Startup cleanup must not race the process that still owns the source saver.
    assert!(workspaces::finish_deletions(root, |_| Ok(())).is_err());
    source.stop().unwrap();
    let mut removed = vec![];
    workspaces::finish_deletions(root, |id| {
        removed.push(id);
        Ok(())
    })
    .unwrap();
    assert_eq!(removed, [id]);
    workspaces::finish_deletions(root, |_| panic!("Cleanup already finished")).unwrap();
    assert!(workspaces::restore(root, || {}).unwrap().is_none());
}

#[test]
fn deleting_workspace_keeps_passwords_referenced_by_remaining_workspaces() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let shared = Profile::default();
    let first = create_workspace(root, "First");
    let (_, mut workspace, mut saver) = workspaces::open(root, Some(first), None, || {}).unwrap();
    workspace.profiles.push(shared.clone());
    saver.finish(workspace).unwrap();
    let (catalog, mut workspace, mut source) =
        workspaces::open(root, None, Some("Second"), || {}).unwrap();
    let own = Profile::default();
    let own_id = own.id;
    workspace.profiles.extend([shared, own]);
    source.flush(workspace).unwrap().recv().unwrap().unwrap();
    let mut removed = vec![];
    let deleted = workspaces::delete(
        root,
        catalog.active().unwrap().id,
        |id| {
            removed.push(id);
            Ok(())
        },
        || {},
    )
    .unwrap();
    source.stop().unwrap();
    assert_eq!(removed, [own_id]);
    let (workspace, mut saver) = deleted.next.unwrap();
    saver.finish(workspace).unwrap();
}

#[test]
fn version_one_catalog_migrates_selection_and_uses_creation_order_for_unknown_history() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let first = create_workspace(root, "First");
    let second = create_workspace(root, "Second");
    let third = create_workspace(root, "Third");
    let path = root.join("workspaces.json");
    std::fs::write(&path, serde_json::to_vec(&serde_json::json!({
        "version": 1, "active": first, "entries": [
            {"id": first, "name": "First"}, {"id": second, "name": "Second"}, {"id": third, "name": "Third"}
        ]
    })).unwrap()).unwrap();
    let (_, workspace, mut source) = workspaces::restore(root, || {}).unwrap().unwrap();
    source.flush(workspace).unwrap().recv().unwrap().unwrap();
    let deleted = workspaces::delete(root, first, |_| Ok(()), || {}).unwrap();
    source.stop().unwrap();
    assert_eq!(deleted.catalog.active().unwrap().id, third);
    let (workspace, mut saver) = deleted.next.unwrap();
    saver.finish(workspace).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(saved["version"], 2);
    assert_eq!(saved["recent"], serde_json::json!([third, second]));
}

#[test]
fn invalid_history_and_pending_deletions_are_not_applied() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let id = create_workspace(root, "First");
    let path = root.join("workspaces.json");
    let valid: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (key, value) in [
        ("version", serde_json::json!(99)),
        ("recent", serde_json::json!([id, id])),
        ("recent", serde_json::json!([])),
        (
            "pending_deletions",
            serde_json::json!([{"id": id, "profiles": []}]),
        ),
    ] {
        let mut invalid = valid.clone();
        invalid[key] = value;
        let bytes = serde_json::to_vec(&invalid).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            workspaces::finish_deletions(root, |_| panic!(
                "Invalid intent must not delete passwords"
            ))
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
