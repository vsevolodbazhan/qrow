use qrow::{
    model::{
        ASSISTANT_DATA_SHARING_NOTICE_VERSION, AssistantConversation, AssistantExecutionMode,
        AssistantHarness, AssistantTitleSource, Profile, SavedTab, WORKSPACE_VERSION, Workspace,
    },
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
            .args(["--exact", "storage::workspace_lock_child", "--nocapture"])
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
    assert_eq!(
        WorkspaceFile::acquire(path).unwrap().load().unwrap().0,
        state
    );
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
    state.settings.assistant_font_family = "Helvetica".into();
    state.settings.assistant_font_size = 17.;
    state.settings.assistant_line_height = 1.5;
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
fn version_one_tabs_migrate_to_connection_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let profile = Profile::default();
    let mut tab = SavedTab::new(1, None);
    tab.sql = "select λ".into();
    let mut json = serde_json::to_value(Workspace {
        version: 1,
        profiles: vec![profile.clone()],
        tabs: vec![tab],
        active_tab: 0,
        ..Workspace::default()
    })
    .unwrap();
    json.as_object_mut().unwrap().remove("active_tabs");
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();

    let restored = qrow::storage::load(&path).unwrap();
    assert_eq!(restored.version, WORKSPACE_VERSION);
    assert_eq!(restored.tabs[0].profile, Some(profile.id));
    assert_eq!(restored.tabs[0].sql, "select λ");
    assert_eq!(restored.active_tabs[&profile.id], restored.tabs[0].id);
}

#[test]
fn version_two_workspaces_get_safe_assistant_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let mut json = serde_json::to_value(Workspace::default()).unwrap();
    let object = json.as_object_mut().unwrap();
    object.insert("version".into(), 2.into());
    object.remove("assistant");
    object
        .get_mut("settings")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("assistant");
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();

    let restored = qrow::storage::load(&path).unwrap();
    assert_eq!(restored.version, WORKSPACE_VERSION);
    assert_eq!(restored.settings.assistant, Default::default());
    assert_eq!(restored.assistant, Default::default());
}

#[test]
fn populated_assistant_state_round_trips_through_workspace_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let profile = Profile::default();
    let tab = SavedTab::new(1, Some(profile.id));
    let mut workspace = Workspace {
        active_tabs: [(profile.id, tab.id)].into(),
        profiles: vec![profile.clone()],
        tabs: vec![tab],
        ..Workspace::default()
    };
    workspace.settings.assistant.enabled = true;
    workspace.settings.assistant.data_sharing_notice_version =
        ASSISTANT_DATA_SHARING_NOTICE_VERSION;
    workspace.settings.assistant.default_execution_mode = AssistantExecutionMode::RunAutomatically;
    workspace.settings.assistant.codex.model = Some("gpt-test".into());
    let mut conversation =
        AssistantConversation::new("thread-1", AssistantExecutionMode::RunAutomatically);
    conversation.title = "Revenue review".into();
    conversation.title_source = AssistantTitleSource::User;
    conversation.last_activity = 1_800_000_000;
    conversation.tab_id = Some(workspace.tabs[0].id);
    workspace.assistant.conversations.push(conversation);
    let mut detached =
        AssistantConversation::new("thread-2", AssistantExecutionMode::AskBeforeRunning);
    detached.detached_profile = Some(profile.id);
    workspace.assistant.conversations.push(detached);
    fs::write(&path, serde_json::to_vec(&workspace).unwrap()).unwrap();

    let restored = qrow::storage::load(&path).unwrap();

    assert_eq!(restored, workspace);
}

#[test]
fn version_seven_conversations_load_as_codex_and_claude_code_ones_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let mut workspace = Workspace::default();
    workspace.settings.assistant.harness = AssistantHarness::Claude;
    let mut claude =
        AssistantConversation::new("claude-1", AssistantExecutionMode::AskBeforeRunning);
    claude.harness = AssistantHarness::Claude;
    workspace.assistant.conversations.push(claude);
    fs::write(&path, serde_json::to_vec(&workspace).unwrap()).unwrap();
    let restored = qrow::storage::load(&path).unwrap();
    assert_eq!(
        restored.assistant.conversations[0].harness,
        AssistantHarness::Claude
    );
    assert_eq!(
        restored.settings.assistant.harness,
        AssistantHarness::Claude
    );

    // A version 7 workspace has no harness. Its conversations use Codex.
    let mut json = serde_json::to_value(&workspace).unwrap();
    json["version"] = 7.into();
    json["settings"]["assistant"]
        .as_object_mut()
        .unwrap()
        .remove("harness");
    json["assistant"]["conversations"][0]
        .as_object_mut()
        .unwrap()
        .remove("harness");
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let restored = qrow::storage::load(&path).unwrap();
    assert_eq!(restored.version, WORKSPACE_VERSION);
    assert_eq!(
        restored.assistant.conversations[0].harness,
        AssistantHarness::Codex
    );
    assert_eq!(restored.settings.assistant.harness, AssistantHarness::Codex);
}

#[test]
fn version_three_conversations_migrate_without_a_query_tab() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let profile = Profile::default();
    let tab = SavedTab::new(1, Some(profile.id));
    let mut workspace = Workspace {
        profiles: vec![profile.clone()],
        tabs: vec![tab],
        ..Workspace::default()
    };
    workspace
        .assistant
        .conversations
        .push(AssistantConversation::new(
            "thread-1",
            AssistantExecutionMode::AskBeforeRunning,
        ));
    let mut json = serde_json::to_value(&workspace).unwrap();
    json["version"] = 3.into();
    let conversation = json["assistant"]["conversations"][0]
        .as_object_mut()
        .unwrap();
    conversation.remove("tab_id");
    conversation.remove("detached_profile");
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();

    let restored = qrow::storage::load(&path).unwrap();

    assert_eq!(restored.version, WORKSPACE_VERSION);
    let conversation = &restored.assistant.conversations[0];
    assert_eq!(conversation.thread_id, "thread-1");
    assert_eq!(conversation.tab_id, None);
    assert_eq!(conversation.detached_profile, Some(profile.id));
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

/// A workspace that a newer Qrow saved, with fields that this build does
/// not know at the top, in the settings, and in a connection.
fn newer_workspace(path: &std::path::Path) -> serde_json::Value {
    let mut workspace = Workspace::default();
    workspace.profiles.push(Profile::default());
    let mut value = serde_json::to_value(&workspace).unwrap();
    value["written_by"] = "9.9.9".into();
    value["future_panel"] = serde_json::json!({"open": true});
    value["settings"]["future_option"] = 7.into();
    value["profiles"][0]["future_timeout"] = 30.into();
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    value
}

#[test]
fn a_save_keeps_the_fields_of_a_newer_version_and_backs_up_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let original = newer_workspace(&path);
    let original_text = fs::read(&path).unwrap();
    let mut file = WorkspaceFile::acquire(path.clone()).unwrap();
    let (mut workspace, newer) = file.load().unwrap();
    let newer = newer.expect("the file has unknown fields");
    assert_eq!(newer.written_by.as_deref(), Some("9.9.9"));
    // The workspace file resolves its path, and the temporary folder of
    // macOS is under a symbolic link (/var is /private/var).
    let folder = dir.path().canonicalize().unwrap();
    assert_eq!(newer.backup, folder.join("workspace.backup.json"));
    assert_eq!(fs::read(&newer.backup).unwrap(), original_text);
    assert!(newer.notice().contains("Qrow 9.9.9"));
    workspace.settings.results_minimized = true;
    file.save(&workspace).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["future_panel"], original["future_panel"]);
    assert_eq!(saved["settings"]["future_option"], 7);
    assert_eq!(saved["profiles"][0]["future_timeout"], 30);
    assert_eq!(saved["settings"]["results_minimized"], true);
    // The file now names this build as its last writer.
    assert_eq!(saved["written_by"], qrow::build_info::RELEASE_VERSION);
}

#[test]
fn a_file_of_this_version_has_no_backup_and_names_its_writer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let file = WorkspaceFile::acquire(path.clone()).unwrap();
    file.save(&Workspace::default()).unwrap();
    drop(file);
    let mut file = WorkspaceFile::acquire(path.clone()).unwrap();
    let (_, newer) = file.load().unwrap();
    assert_eq!(newer, None);
    assert!(!dir.path().join("workspace.backup.json").exists());
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["written_by"], qrow::build_info::RELEASE_VERSION);
}

#[test]
fn the_saver_reports_the_data_of_a_newer_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    newer_workspace(&path);
    let (workspace, mut saver) = Saver::open(path.clone(), || {}).unwrap();
    assert!(saver.newer.is_some());
    saver.finish(workspace).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["settings"]["future_option"], 7);
}

#[test]
fn forward_compatible_newer_version_preserves_version_and_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let mut value = serde_json::to_value(Workspace::default()).unwrap();
    value["version"] = (WORKSPACE_VERSION + 2).into();
    value["min_reader_version"] = WORKSPACE_VERSION.into();
    value["written_by"] = "9.9.9".into();
    value["future_flag"] = true.into();
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    let (mut workspace, mut saver) = Saver::open(path.clone(), || {}).unwrap();
    assert!(saver.newer.is_some());
    workspace.tabs[0].sql = "SELECT 42".into();
    saver.finish(workspace).unwrap();

    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], WORKSPACE_VERSION + 2);
    assert_eq!(saved["min_reader_version"], WORKSPACE_VERSION);
    assert_eq!(saved["future_flag"], true);
    assert_eq!(saved["tabs"][0]["sql"], "SELECT 42");
}

#[test]
fn legacy_v8_directory_migrates_to_v9_multi_workspace_layout() {
    let data_dir = tempfile::tempdir().unwrap();
    let local_dir = tempfile::tempdir().unwrap();
    let legacy_path = data_dir.path().join("workspace.json");

    let profile = Profile {
        name: "Analytics".into(),
        ..Profile::default()
    };
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = "SELECT 1".into();
    let mut workspace = Workspace {
        version: 8,
        profiles: vec![profile.clone()],
        tabs: vec![tab],
        active_tab: 0,
        ..Workspace::default()
    };
    workspace.settings.theme = "One Dark".into();
    workspace.settings.editor_font_size = 15.;

    let mut legacy_json = serde_json::to_value(&workspace).unwrap();
    legacy_json["written_by"] = "0.6.0".into();
    legacy_json["settings"]["future_setting"] = "keep-me".into();
    legacy_json["future_workspace_field"] = 99.into();
    let legacy_bytes = serde_json::to_vec_pretty(&legacy_json).unwrap();
    fs::write(&legacy_path, &legacy_bytes).unwrap();

    // Seed machine-local caches and transcripts in a separate local dir (Windows style).
    fs::create_dir_all(local_dir.path().join("catalog")).unwrap();
    fs::write(
        local_dir
            .path()
            .join("catalog")
            .join(format!("{}.json", profile.id)),
        b"{}",
    )
    .unwrap();
    fs::create_dir_all(local_dir.path().join("dbt")).unwrap();
    fs::write(local_dir.path().join("dbt/index.bin"), b"dbt").unwrap();
    fs::create_dir_all(local_dir.path().join("assistant/transcripts")).unwrap();
    fs::write(
        local_dir.path().join("assistant/transcripts/thread-1.json"),
        b"transcript",
    )
    .unwrap();

    let opened = storage::DataStore::open(
        data_dir.path().to_path_buf(),
        local_dir.path().to_path_buf(),
        || {},
    )
    .unwrap();
    assert_eq!(opened.store.workspaces().len(), 1);
    assert_eq!(opened.store.active_workspace_entry().name, "Default");
    assert_eq!(opened.settings.theme, "One Dark");
    assert_eq!(opened.settings.editor_font_size, 15.);
    assert_eq!(opened.workspace.profiles[0].id, profile.id);
    assert_eq!(opened.workspace.tabs[0].sql, "SELECT 1");

    // Pre-migration backup holds exact original bytes, and legacy file is gone.
    let backup = data_dir.path().join("backups/workspace.v8.backup.json");
    assert_eq!(fs::read(&backup).unwrap(), legacy_bytes);
    assert!(!legacy_path.exists());

    // Split workspace.json omits `settings` and preserves workspace unknown fields.
    let ws_json: serde_json::Value =
        serde_json::from_slice(&fs::read(opened.store.active_workspace_path()).unwrap()).unwrap();
    assert!(ws_json.get("settings").is_none());
    assert_eq!(ws_json["version"], WORKSPACE_VERSION);
    assert_eq!(ws_json["future_workspace_field"], 99);

    // Split settings.json preserves unknown settings fields.
    let settings_json: serde_json::Value =
        serde_json::from_slice(&fs::read(storage::settings_path(data_dir.path())).unwrap())
            .unwrap();
    assert_eq!(settings_json["settings"]["theme"], "One Dark");
    assert_eq!(settings_json["settings"]["future_setting"], "keep-me");

    // Local caches and transcripts moved under workspaces/<active-id>/.
    let ws_local = opened.store.active_local_directory();
    assert!(
        ws_local
            .join("catalog")
            .join(format!("{}.json", profile.id))
            .is_file()
    );
    assert!(ws_local.join("dbt/index.bin").is_file());
    assert!(
        ws_local
            .join("assistant/transcripts/thread-1.json")
            .is_file()
    );
    assert!(!local_dir.path().join("catalog").exists());
    assert!(!local_dir.path().join("dbt").exists());
    assert!(!local_dir.path().join("assistant").exists());

    // Probe helpers resolve both active and named workspace paths.
    assert_eq!(
        storage::resolve_active_workspace_path(data_dir.path()).unwrap(),
        opened.store.active_workspace_path()
    );
    assert_eq!(
        storage::resolve_workspace_path_by_name(data_dir.path(), "default").unwrap(),
        opened.store.active_workspace_path()
    );
}

#[test]
fn data_store_workspace_crud_remaps_ids_and_cleans_up_secrets() {
    use qrow::model::{Authentication, Identity, SharedCatalog, SignIn};
    use qrow::storage::{Credentials, MemoryTokenStore, TokenStore};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use uuid::Uuid;
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct TestCreds(Mutex<HashMap<Uuid, String>>);
    impl Credentials for TestCreds {
        fn password(&self, id: Uuid) -> anyhow::Result<Zeroizing<String>> {
            self.0
                .lock()
                .unwrap()
                .get(&id)
                .cloned()
                .map(Zeroizing::new)
                .ok_or_else(|| anyhow::anyhow!("missing"))
        }
        fn set_password(&self, id: Uuid, password: &str) -> anyhow::Result<()> {
            self.0.lock().unwrap().insert(id, password.into());
            Ok(())
        }
        fn delete_password(&self, id: Uuid) -> anyhow::Result<()> {
            self.0.lock().unwrap().remove(&id);
            Ok(())
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let mut opened =
        storage::DataStore::open(dir.path().to_path_buf(), dir.path().to_path_buf(), || {})
            .unwrap();

    // Populate active workspace with profile, shared catalog, sign-in, tab, conversation.
    let sign_in = SignIn {
        name: "Corp SSO".into(),
        identity: Some(Identity {
            issuer: "https://sso.example".into(),
            subject: "sub-1".into(),
            name: Some("Alice".into()),
            email: Some("alice@example".into()),
        }),
        ..SignIn::default()
    };
    let mut shared = SharedCatalog {
        id: Uuid::new_v4(),
        name: "Warehouse".into(),
        settings: Default::default(),
        preferred: None,
    };
    let profile = Profile {
        name: "Prod".into(),
        shared_catalog: Some(shared.id),
        authentication: Authentication::Oidc {
            sign_in: sign_in.id,
        },
        ..Profile::default()
    };
    shared.preferred = Some(profile.id);
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = "SELECT * FROM orders".into();
    let mut ws = opened.workspace.clone();
    ws.sign_ins.push(sign_in.clone());
    ws.shared_catalogs.push(shared.clone());
    ws.profiles.push(profile.clone());
    ws.tabs = vec![tab.clone()];
    ws.active_tab = 0;
    ws.assistant.conversations.push(AssistantConversation::new(
        "thread-src",
        AssistantExecutionMode::AskBeforeRunning,
    ));
    ws.normalize();
    opened.workspace_saver.finish(ws).unwrap();

    let creds = TestCreds::default();
    let tokens = MemoryTokenStore::default();
    creds.set_password(profile.id, "secret").unwrap();
    tokens
        .save_tokens(sign_in.id, "{\"access\":\"tok\"}")
        .unwrap();

    // Create + rename.
    let created = opened.store.create_workspace("Analytics").unwrap();
    assert_eq!(created.name, "Analytics");
    assert!(opened.store.create_workspace("analytics").is_err());
    let renamed = opened
        .store
        .rename_workspace(created.id, "Staging")
        .unwrap();
    assert_eq!(renamed.name, "Staging");

    // Duplicate remaps every UUID and clears identity + assistant threads.
    let source_id = opened.store.active_workspace_id();
    let dup = opened
        .store
        .duplicate_workspace(source_id, None, None)
        .unwrap();
    assert_eq!(dup.name, "Default copy");
    let dup_ws = storage::load(&opened.store.workspace_path(dup.id)).unwrap();
    assert_eq!(dup_ws.profiles.len(), 1);
    assert_ne!(dup_ws.profiles[0].id, profile.id);
    assert_ne!(dup_ws.sign_ins[0].id, sign_in.id);
    assert!(dup_ws.sign_ins[0].identity.is_none());
    assert_ne!(dup_ws.shared_catalogs[0].id, shared.id);
    assert_eq!(
        dup_ws.profiles[0].shared_catalog,
        Some(dup_ws.shared_catalogs[0].id)
    );
    assert_eq!(
        dup_ws.shared_catalogs[0].preferred,
        Some(dup_ws.profiles[0].id)
    );
    assert_eq!(
        dup_ws.profiles[0].authentication,
        Authentication::Oidc {
            sign_in: dup_ws.sign_ins[0].id,
        }
    );
    assert_ne!(dup_ws.tabs[0].id, tab.id);
    assert_eq!(dup_ws.tabs[0].profile, Some(dup_ws.profiles[0].id));
    assert_eq!(dup_ws.tabs[0].sql, "SELECT * FROM orders");
    assert!(dup_ws.assistant.conversations.is_empty());

    // Delete refuses while a workspace lock is held, then succeeds after release.
    let (_locked_ws, mut locked_saver) =
        opened.store.open_workspace_saver(source_id, || {}).unwrap();
    assert!(
        opened
            .store
            .delete_workspace(source_id, &creds, &tokens)
            .is_err()
    );
    assert!(creds.password(profile.id).is_ok());
    assert!(tokens.load_tokens(sign_in.id).unwrap().is_some());
    locked_saver.stop().unwrap();

    // Delete source workspace cleans up Keychain secrets and directories.
    let warnings = opened
        .store
        .delete_workspace(source_id, &creds, &tokens)
        .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(creds.password(profile.id).is_err());
    assert!(tokens.load_tokens(sign_in.id).unwrap().is_none());
    assert!(!opened.store.workspace_dir(source_id).exists());
    assert!(!opened.store.workspace_local_dir(source_id).exists());

    // Cannot delete the last remaining workspace.
    opened
        .store
        .delete_workspace(renamed.id, &creds, &tokens)
        .unwrap();
    assert!(
        opened
            .store
            .delete_workspace(dup.id, &creds, &tokens)
            .is_err()
    );
}

#[test]
fn assistant_settings_unknown_fields_survive_legacy_split_and_save() {
    let data_dir = tempfile::tempdir().unwrap();
    let local_dir = tempfile::tempdir().unwrap();
    let legacy_path = data_dir.path().join("workspace.json");

    let mut legacy_json = serde_json::to_value(Workspace {
        version: 8,
        ..Workspace::default()
    })
    .unwrap();
    legacy_json["settings"]["assistant"]["future_assistant_flag"] = "kept".into();
    fs::write(
        &legacy_path,
        serde_json::to_vec_pretty(&legacy_json).unwrap(),
    )
    .unwrap();

    let mut opened = storage::DataStore::open(
        data_dir.path().to_path_buf(),
        local_dir.path().to_path_buf(),
        || {},
    )
    .unwrap();
    let mut updated = opened.settings.clone();
    updated.editor_font_size = 16.;
    opened.settings_saver.finish(updated).unwrap();

    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(storage::settings_path(data_dir.path())).unwrap())
            .unwrap();
    assert_eq!(
        saved["settings"]["assistant"]["future_assistant_flag"],
        "kept"
    );
    assert_eq!(saved["settings"]["editor_font_size"], 16.0);
}

#[test]
fn second_machine_relocates_local_transcripts_and_missing_files_refuse_overwrite() {
    let roaming_dir = tempfile::tempdir().unwrap();
    let local_machine_1 = tempfile::tempdir().unwrap();
    let local_machine_2 = tempfile::tempdir().unwrap();

    // Machine 1 migrates legacy workspace.json (v3 with empty tabs + selected_thread).
    let legacy_path = roaming_dir.path().join("workspace.json");
    let mut v3 = serde_json::to_value(Workspace::default()).unwrap();
    v3["version"] = 3.into();
    v3["tabs"] = serde_json::json!([]);
    v3["assistant"]["selected_thread"] = "thread-v3".into();
    v3["assistant"]["conversations"] = serde_json::json!([{
        "thread_id": "thread-v3",
        "title": "Legacy",
        "title_source": "user",
        "last_activity": 1,
        "execution_mode": "ask_before_running"
    }]);
    fs::write(&legacy_path, serde_json::to_vec_pretty(&v3).unwrap()).unwrap();

    let mut opened_1 = storage::DataStore::open(
        roaming_dir.path().to_path_buf(),
        local_machine_1.path().to_path_buf(),
        || {},
    )
    .unwrap();
    let migrated_id = opened_1.store.active_workspace_id();
    assert_eq!(
        opened_1.workspace.assistant.conversations[0].tab_id,
        Some(opened_1.workspace.tabs[0].id)
    );
    opened_1.workspace_saver.stop().unwrap();
    opened_1.settings_saver.stop().unwrap();
    drop(opened_1);

    // Machine 2 has legacy local transcripts and receives the roaming dir.
    fs::create_dir_all(local_machine_2.path().join("assistant/transcripts")).unwrap();
    fs::write(
        local_machine_2
            .path()
            .join("assistant/transcripts/thread-v3.json"),
        b"machine-2-transcript",
    )
    .unwrap();
    let mut opened_2 = storage::DataStore::open(
        roaming_dir.path().to_path_buf(),
        local_machine_2.path().to_path_buf(),
        || {},
    )
    .unwrap();
    assert!(
        opened_2
            .store
            .workspace_local_dir(migrated_id)
            .join("assistant/transcripts/thread-v3.json")
            .is_file()
    );
    opened_2.workspace_saver.stop().unwrap();
    opened_2.settings_saver.stop().unwrap();
    drop(opened_2);

    // Removing the registered workspace.json fails loudly instead of overwriting with blank.
    let ws_path = storage::workspace_file_path(roaming_dir.path(), migrated_id);
    let saved_bytes = fs::read(&ws_path).unwrap();
    fs::remove_file(&ws_path).unwrap();
    let missing_err = storage::DataStore::open(
        roaming_dir.path().to_path_buf(),
        local_machine_2.path().to_path_buf(),
        || {},
    )
    .err()
    .expect("missing registered workspace must fail")
    .to_string();
    assert!(missing_err.contains("missing"), "{missing_err}");
    assert!(!ws_path.exists());

    // Restoring workspace.json and removing workspaces.json refuses to overwrite existing dirs.
    fs::write(&ws_path, &saved_bytes).unwrap();
    fs::remove_file(storage::registry_path(roaming_dir.path())).unwrap();
    let reg_err = storage::DataStore::open(
        roaming_dir.path().to_path_buf(),
        local_machine_2.path().to_path_buf(),
        || {},
    )
    .err()
    .expect("missing registry with existing workspace folders must fail")
    .to_string();
    assert!(
        reg_err.contains("Workspace registry is missing"),
        "{reg_err}"
    );
}
