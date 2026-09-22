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
    assert_eq!(catalog.active().name, "Default");
    saver.finish(loaded).unwrap();
    let (catalog, mut created, mut saver) =
        workspaces::open(root, None, Some(" Analytics / 日本語 "), || {}).unwrap();
    assert!(created.profiles.is_empty());
    assert!(created.tabs[0].sql.is_empty());
    assert_eq!(catalog.active().name, "Analytics / 日本語");
    let id = catalog.active().id;
    created.tabs[0].sql = "SELECT 'another workspace'".into();
    created.settings.ui_scale = 1.25;
    saver.finish(created.clone()).unwrap();
    let (restored, loaded, mut saver) = workspaces::open(root, None, None, || {}).unwrap();
    assert_eq!(restored.active().id, id);
    assert_eq!(loaded, created);
    saver.finish(loaded).unwrap();
    let (_, loaded, mut saver) = workspaces::open(root, Some(Uuid::nil()), None, || {}).unwrap();
    assert_eq!(loaded, original);
    saver.finish(loaded).unwrap();
    assert_eq!(
        storage::load(&catalog.path(root, id).unwrap()).unwrap(),
        created
    );
}

#[test]
fn rejects_invalid_names_without_changing_selection_or_list() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["", "  ", "default", "Default", "a\nb", &"a".repeat(81)] {
        assert!(workspaces::open(dir.path(), None, Some(name), || {}).is_err());
        assert_eq!(Catalog::load(dir.path()).unwrap().entries().len(), 1);
        assert!(!dir.path().join("workspaces.json").exists());
    }
}

#[test]
fn locked_corrupt_and_missing_destinations_preserve_selection() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (catalog, state, mut saver) = workspaces::open(root, None, Some("Second"), || {}).unwrap();
    let id = catalog.active().id;
    saver.finish(state).unwrap();
    let (_, state, mut source) = workspaces::open(root, Some(Uuid::nil()), None, || {}).unwrap();
    let path = catalog.path(root, id).unwrap();
    let lock = WorkspaceFile::acquire(path.clone()).unwrap();
    assert!(workspaces::open(root, Some(id), None, || {}).is_err());
    assert!(Catalog::load(root).unwrap().active().id.is_nil());
    drop(lock);
    std::fs::write(&path, "broken").unwrap();
    assert!(workspaces::open(root, Some(id), None, || {}).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken");
    std::fs::remove_file(&path).unwrap();
    assert!(workspaces::open(root, Some(id), None, || {}).is_err());
    assert!(Catalog::load(root).unwrap().active().id.is_nil());
    source.finish(state).unwrap();
}

#[test]
fn invalid_catalog_is_not_overwritten_and_unknown_ids_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    assert!(workspaces::open(dir.path(), Some(Uuid::new_v4()), None, || {}).is_err());
    let path = dir.path().join("workspaces.json");
    std::fs::write(&path, "broken").unwrap();
    assert!(workspaces::open(dir.path(), None, None, || {}).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "broken");
}
