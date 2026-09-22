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
