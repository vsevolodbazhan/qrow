use qrow::{
    model::Workspace,
    storage::{self, Saver, WorkspaceFile},
};
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

const TIMEOUT: Duration = Duration::from_secs(5);

struct TestProcess(Child);
impl Drop for TestProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn workspace_lock_child() {
    let Some(path) = std::env::var_os("QROW_TEST_LOCK_PATH") else {
        return;
    };
    let _file = WorkspaceFile::acquire(path.into()).unwrap();
    println!("workspace locked");
    let _ = std::io::stdin().read(&mut [0]);
}

#[test]
fn competing_process_cannot_load_for_writing_and_crash_releases_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let mut state = Workspace::default();
    state.tabs[0].sql = "SELECT 'preserve my work'".into();
    WorkspaceFile::acquire(path.clone())
        .unwrap()
        .save(&state)
        .unwrap();
    let mut child = TestProcess(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "workspace_lock_child", "--nocapture"])
            .env("QROW_TEST_LOCK_PATH", &path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let output = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            if line.unwrap().contains("workspace locked") {
                let _ = tx.send(());
                break;
            }
        }
    });
    rx.recv_timeout(TIMEOUT)
        .expect("child did not claim workspace");
    let error = Saver::open(path.clone(), || {}).err().unwrap();
    assert!(error.to_string().contains("already open"));
    assert_eq!(storage::load(&path).unwrap(), state);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    reader.join().unwrap();
    let (restored, mut saver) = Saver::open(path.clone(), || {}).unwrap();
    assert_eq!(restored, state);
    saver.finish(restored).unwrap();
    assert!(path.with_extension("lock").exists());
}

#[test]
fn failed_final_save_is_acknowledged_and_can_retry_without_releasing_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let (mut state, mut saver) = Saver::open(path.clone(), || {}).unwrap();
    saver
        .flush(state.clone())
        .unwrap()
        .recv_timeout(TIMEOUT)
        .unwrap()
        .unwrap();
    let original = fs::read(&path).unwrap();
    fs::rename(&path, dir.path().join("original.json")).unwrap();
    fs::create_dir(&path).unwrap();
    state.tabs[0].sql = "SELECT '日本語 final edit'".into();
    let error = saver
        .flush(state.clone())
        .unwrap()
        .recv_timeout(TIMEOUT)
        .unwrap()
        .unwrap_err();
    assert!(error.contains("Workspace save failed"));
    assert!(saver.finish(state.clone()).is_err());
    assert!(WorkspaceFile::acquire(path.clone()).is_err());
    assert_eq!(
        fs::read(dir.path().join("original.json")).unwrap(),
        original
    );
    assert!(!fs::read_dir(dir.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|extension| extension == "tmp")
    }));
    fs::remove_dir(&path).unwrap();
    saver.finish(state.clone()).unwrap();
    assert_eq!(WorkspaceFile::acquire(path).unwrap().load().unwrap(), state);
}

#[test]
fn flush_acknowledges_its_snapshot_and_subsequent_edits_are_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let (mut state, mut saver) = Saver::open(path.clone(), || {}).unwrap();
    state.tabs[0].sql = "SELECT 1".into();
    state.settings.ui_scale = 1.2;
    state.settings.ui_font_family = "Helvetica".into();
    state.settings.editor_font_family = "Monaco".into();
    state.settings.editor_font_size = 16.;
    state.settings.editor_line_height = 1.1;
    state.settings.logs_font_family = "Courier".into();
    state.settings.logs_font_size = 15.;
    state.settings.logs_line_height = 1.4;
    saver.save(Workspace::default()).unwrap();
    let receipt = saver.flush(state.clone()).unwrap();
    receipt.recv_timeout(TIMEOUT).unwrap().unwrap();
    assert_eq!(storage::load(&path).unwrap(), state);
    state.tabs[0].sql = "SELECT 2".into();
    saver.finish(state.clone()).unwrap();
    assert_eq!(storage::load(&path).unwrap(), state);
    assert!(saver.flush(state).is_err());
    saver.stop().unwrap();
}

#[test]
fn invalid_workspace_is_preserved_and_does_not_leave_a_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    for bytes in [
        b"broken".to_vec(),
        serde_json::to_vec(&Workspace {
            version: 99,
            ..Workspace::default()
        })
        .unwrap(),
    ] {
        fs::write(&path, &bytes).unwrap();
        assert!(Saver::open(path.clone(), || {}).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(WorkspaceFile::acquire(path.clone()).is_ok());
    }
}

#[test]
fn autosave_failure_wakes_ui_and_workspace_files_are_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let (tx, wake) = mpsc::channel();
    let (state, mut saver) = Saver::open(path.clone(), move || {
        let _ = tx.send(());
    })
    .unwrap();
    fs::create_dir(&path).unwrap();
    saver.save(state.clone()).unwrap();
    wake.recv_timeout(TIMEOUT).unwrap();
    assert!(
        saver
            .errors
            .recv_timeout(TIMEOUT)
            .unwrap()
            .contains("Workspace save failed")
    );
    fs::remove_dir(&path).unwrap();
    saver.finish(state).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in [&path, &path.with_extension("lock")] {
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
