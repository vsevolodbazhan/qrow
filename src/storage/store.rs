//! Multi-workspace storage: global `settings.json`, `workspaces.json` registry,
//! one-time `v8 -> v9` layout migration, and workspace CRUD.

use super::{
    Credentials, NewerData, SaveReceipt, SaveResult, Saver, TokenStore, WorkspaceFile,
    migrations::{
        self, ENVELOPE_KEYS, MigratedDocument, PreservedVersion, REGISTRY_SPEC, SETTINGS_SPEC,
        WORKSPACE_SPEC,
    },
    unknown::{self, Unknown},
    write_atomically,
};
use crate::model::{
    DEFAULT_WORKSPACE_NAME, SavedSettings, Settings, Workspace, WorkspaceEntry, WorkspaceRegistry,
    copied_workspace_name, validate_workspace_name,
};
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Root data directory for `settings.json`, `workspaces.json`, and
/// `workspaces/<id>/workspace.json`.
pub fn data_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("QROW_DATA_DIR") {
        return PathBuf::from(path);
    }
    super::workspace_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

pub fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

pub fn registry_path(data_dir: &Path) -> PathBuf {
    data_dir.join("workspaces.json")
}

pub fn backups_directory(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

pub fn workspace_directory(data_dir: &Path, id: Uuid) -> PathBuf {
    data_dir.join("workspaces").join(id.to_string())
}

pub fn workspace_file_path(data_dir: &Path, id: Uuid) -> PathBuf {
    workspace_directory(data_dir, id).join("workspace.json")
}

pub fn workspace_local_directory(local_dir: &Path, id: Uuid) -> PathBuf {
    local_dir.join("workspaces").join(id.to_string())
}

/// Resolves the active workspace file under `data_dir` without claiming a lock,
/// falling back to a legacy `<data_dir>/workspace.json` when unmigrated.
pub fn resolve_active_workspace_path(data_dir: &Path) -> Result<PathBuf> {
    let reg_path = registry_path(data_dir);
    if !reg_path.exists() {
        return Ok(data_dir.join("workspace.json"));
    }
    let data = fs::read(&reg_path).context("Could not read workspace registry")?;
    let migrated = migrations::migrate_bytes(&data, &REGISTRY_SPEC)?;
    let (mut registry, _): (WorkspaceRegistry, Unknown) =
        Unknown::read_value(&migrated.value, &ENVELOPE_KEYS)
            .context("Saved workspace registry is invalid")?;
    registry.sanitize(now_unix_seconds());
    Ok(workspace_file_path(data_dir, registry.active_workspace))
}

/// Resolves a named workspace file under `data_dir` (case-insensitive) without
/// claiming a lock.
pub fn resolve_workspace_path_by_name(data_dir: &Path, name: &str) -> Result<PathBuf> {
    let reg_path = registry_path(data_dir);
    let data = fs::read(&reg_path).context("Could not read workspace registry")?;
    let migrated = migrations::migrate_bytes(&data, &REGISTRY_SPEC)?;
    let (registry, _): (WorkspaceRegistry, Unknown) =
        Unknown::read_value(&migrated.value, &ENVELOPE_KEYS)
            .context("Saved workspace registry is invalid")?;
    let needle = name.trim().to_lowercase();
    let entry = registry
        .workspaces
        .iter()
        .find(|entry| entry.name.trim().to_lowercase() == needle)
        .with_context(|| format!("No workspace named \"{name}\""))?;
    Ok(workspace_file_path(data_dir, entry.id))
}

/// Reads `settings.json` without claiming a lock. Returns defaults if absent.
pub fn load_settings(path: &Path) -> Result<Settings> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(error) => return Err(error).context("Could not read saved settings"),
    };
    let migrated = migrations::migrate_bytes(&data, &SETTINGS_SPEC)?;
    let (mut saved, _): (SavedSettings, Unknown) =
        Unknown::read_value(&migrated.value, &ENVELOPE_KEYS)
            .context("Saved settings are invalid; the original file has been left untouched")?;
    saved.settings.sanitize();
    Ok(saved.settings)
}

/// Persistent handle for global `settings.json` with unknown-field preservation
/// and forward-compatible version stamping.
pub struct SettingsFile {
    path: PathBuf,
    backup_dir: PathBuf,
    unknown: Unknown,
    preserved_version: Option<PreservedVersion>,
}

impl SettingsFile {
    pub fn open(path: PathBuf, backup_dir: PathBuf) -> Result<(Settings, Option<NewerData>, Self)> {
        let mut file = Self {
            path,
            backup_dir,
            unknown: Unknown::default(),
            preserved_version: None,
        };
        let (settings, newer) = file.load()?;
        Ok((settings, newer, file))
    }

    fn load(&mut self) -> Result<(Settings, Option<NewerData>)> {
        let data = match fs::read(&self.path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Settings::default(), None));
            }
            Err(error) => return Err(error).context("Could not read saved settings"),
        };
        let migrated = migrations::migrate_bytes(&data, &SETTINGS_SPEC)?;
        if let Some(from_version) = migrated.migrated_from {
            let _ = migrations::back_up_version(&self.path, &self.backup_dir, from_version, &data)?;
        }
        let (mut saved, unknown): (SavedSettings, Unknown) =
            Unknown::read_value(&migrated.value, &ENVELOPE_KEYS)
                .context("Saved settings are invalid; the original file has been left untouched")?;
        saved.settings.sanitize();
        let newer = if unknown.is_empty() && migrated.preserved_version.is_none() {
            None
        } else {
            Some(NewerData {
                written_by: migrated.written_by,
                backup: unknown::back_up(&self.path, &data)?,
            })
        };
        self.unknown = unknown;
        self.preserved_version = migrated.preserved_version;
        Ok((saved.settings, newer))
    }

    pub fn save(&self, settings: &Settings) -> Result<()> {
        let parent = self.path.parent().context("Invalid settings path")?;
        fs::create_dir_all(parent).context("Could not create settings directory")?;
        let doc = SavedSettings {
            version: SETTINGS_SPEC.current,
            settings: settings.clone(),
        };
        let mut value = serde_json::to_value(doc)?;
        self.unknown.restore(&mut value);
        migrations::stamp_envelope(&mut value, &SETTINGS_SPEC, self.preserved_version);
        write_atomically(&self.path, &serde_json::to_vec_pretty(&value)?)
    }
}

enum SettingsSaveCommand {
    Save(Settings),
    Flush(Settings, mpsc::Sender<SaveResult>),
    Stop,
}

/// Background atomic saver for global `settings.json`.
pub struct SettingsSaver {
    tx: mpsc::Sender<SettingsSaveCommand>,
    pub errors: mpsc::Receiver<String>,
    pub newer: Option<NewerData>,
    handle: Option<thread::JoinHandle<()>>,
}

impl SettingsSaver {
    pub fn open(
        path: PathBuf,
        backup_dir: PathBuf,
        wake: impl Fn() + Send + 'static,
    ) -> Result<(Settings, Self)> {
        let (settings, newer, file) = SettingsFile::open(path, backup_dir)?;
        let (tx, rx) = mpsc::channel();
        let (errors_tx, errors) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut pending = None;
            while let Some(command) = pending.take().or_else(|| rx.recv().ok()) {
                let (mut state, receipt) = match command {
                    SettingsSaveCommand::Save(state) => (state, None),
                    SettingsSaveCommand::Flush(state, receipt) => (state, Some(receipt)),
                    SettingsSaveCommand::Stop => break,
                };
                if receipt.is_none() {
                    while let Ok(next) = rx.try_recv() {
                        match next {
                            SettingsSaveCommand::Save(next) => state = next,
                            next => {
                                pending = Some(next);
                                break;
                            }
                        }
                    }
                }
                let result = file
                    .save(&state)
                    .map_err(|e| format!("Settings save failed: {e:#}"));
                if let Some(receipt) = receipt {
                    let _ = receipt.send(result);
                    wake();
                } else if let Err(error) = result {
                    let _ = errors_tx.send(error);
                    wake();
                }
            }
        });
        Ok((
            settings,
            Self {
                tx,
                errors,
                newer,
                handle: Some(handle),
            },
        ))
    }

    pub fn save(&self, state: Settings) -> Result<()> {
        self.tx
            .send(SettingsSaveCommand::Save(state))
            .context("Settings saver stopped")
    }

    pub fn flush(&self, state: Settings) -> Result<SaveReceipt> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(SettingsSaveCommand::Flush(state, tx))
            .context("Settings saver stopped")?;
        Ok(rx)
    }

    pub fn finish(&mut self, state: Settings) -> Result<()> {
        self.flush(state)?
            .recv()
            .context("Settings saver stopped before confirming the save")?
            .map_err(anyhow::Error::msg)?;
        self.stop()
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(handle) = self.handle.take() {
            let _ = self.tx.send(SettingsSaveCommand::Stop);
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("Settings saver panicked"))?;
        }
        Ok(())
    }
}

impl Drop for SettingsSaver {
    fn drop(&mut self) {
        let _ = self.tx.send(SettingsSaveCommand::Stop);
    }
}

/// Result of opening the application data directory.
pub struct OpenedStore {
    pub store: DataStore,
    pub settings: Settings,
    pub settings_saver: SettingsSaver,
    pub workspace: Workspace,
    pub workspace_saver: Saver,
    pub notices: Vec<String>,
}

/// Manages the multi-workspace directory layout, `workspaces.json` registry,
/// app-level directory lock, and workspace lifecycle operations.
pub struct DataStore {
    data_dir: PathBuf,
    local_dir: PathBuf,
    _app_lock: fs::File,
    registry: WorkspaceRegistry,
    registry_unknown: Unknown,
    registry_preserved_version: Option<PreservedVersion>,
}

impl DataStore {
    /// Acquires `app.lock`, migrates a legacy v1..v8 flat layout if present,
    /// loads `workspaces.json`, `settings.json`, and opens the active
    /// workspace's background saver.
    pub fn open(
        data_dir: PathBuf,
        local_dir: PathBuf,
        wake: impl Fn() + Clone + Send + 'static,
    ) -> Result<OpenedStore> {
        fs::create_dir_all(&data_dir).context("Could not create Qrow data directory")?;
        fs::create_dir_all(&local_dir).context("Could not create Qrow local data directory")?;
        let data_dir = data_dir.canonicalize()?;
        let local_dir = local_dir.canonicalize()?;

        let app_lock = acquire_lock(&data_dir.join("app.lock"), "data directory")?;

        let reg_path = registry_path(&data_dir);
        let legacy_path = data_dir.join("workspace.json");
        let mut notices = Vec::new();

        if !reg_path.exists() {
            if legacy_path.exists() {
                migrate_legacy_layout(&data_dir, &local_dir, &legacy_path)?;
            } else {
                initialize_fresh_store(&data_dir)?;
            }
        }

        let reg_data = fs::read(&reg_path).context("Could not read workspace registry")?;
        let reg_migrated = migrations::migrate_bytes(&reg_data, &REGISTRY_SPEC)?;
        if let Some(from_version) = reg_migrated.migrated_from {
            let _ = migrations::back_up_version(
                &reg_path,
                &backups_directory(&data_dir),
                from_version,
                &reg_data,
            )?;
        }
        let (mut registry, registry_unknown): (WorkspaceRegistry, Unknown) =
            Unknown::read_value(&reg_migrated.value, &ENVELOPE_KEYS).context(
                "Saved workspace registry is invalid; the original file has been left untouched",
            )?;
        if !registry_unknown.is_empty() || reg_migrated.preserved_version.is_some() {
            let backup = unknown::back_up(&reg_path, &reg_data)?;
            notices.push(
                NewerData {
                    written_by: reg_migrated.written_by,
                    backup,
                }
                .notice(),
            );
        }
        let registry_dirty = registry.sanitize(now_unix_seconds());

        let (settings, settings_saver) = SettingsSaver::open(
            settings_path(&data_dir),
            backups_directory(&data_dir),
            wake.clone(),
        )?;
        if let Some(newer) = &settings_saver.newer {
            notices.push(newer.notice());
        }

        let store = Self {
            data_dir,
            local_dir,
            _app_lock: app_lock,
            registry,
            registry_unknown,
            registry_preserved_version: reg_migrated.preserved_version,
        };
        if registry_dirty {
            store.save_registry()?;
        }

        let (workspace, workspace_saver) =
            store.open_workspace_saver(store.registry.active_workspace, wake)?;
        if let Some(newer) = &workspace_saver.newer {
            notices.push(newer.notice());
        }

        Ok(OpenedStore {
            store,
            settings,
            settings_saver,
            workspace,
            workspace_saver,
            notices,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn local_dir(&self) -> &Path {
        &self.local_dir
    }

    pub fn backups_dir(&self) -> PathBuf {
        backups_directory(&self.data_dir)
    }

    pub fn active_workspace_id(&self) -> Uuid {
        self.registry.active_workspace
    }

    pub fn active_workspace_entry(&self) -> &WorkspaceEntry {
        self.registry
            .workspaces
            .iter()
            .find(|entry| entry.id == self.registry.active_workspace)
            .unwrap_or(&self.registry.workspaces[0])
    }

    pub fn workspaces(&self) -> &[WorkspaceEntry] {
        &self.registry.workspaces
    }

    pub fn workspace_entry(&self, id: Uuid) -> Option<&WorkspaceEntry> {
        self.registry.workspaces.iter().find(|entry| entry.id == id)
    }

    pub fn workspace_dir(&self, id: Uuid) -> PathBuf {
        workspace_directory(&self.data_dir, id)
    }

    pub fn workspace_path(&self, id: Uuid) -> PathBuf {
        workspace_file_path(&self.data_dir, id)
    }

    pub fn active_workspace_path(&self) -> PathBuf {
        self.workspace_path(self.registry.active_workspace)
    }

    pub fn workspace_local_dir(&self, id: Uuid) -> PathBuf {
        workspace_local_directory(&self.local_dir, id)
    }

    pub fn active_local_directory(&self) -> PathBuf {
        self.workspace_local_dir(self.registry.active_workspace)
    }

    /// Opens a background workspace saver for `id` in managed mode (settings
    /// live in `settings.json`, pre-migration backups go to `backups/`).
    pub fn open_workspace_saver(
        &self,
        id: Uuid,
        wake: impl Fn() + Send + 'static,
    ) -> Result<(Workspace, Saver)> {
        anyhow::ensure!(
            self.workspace_entry(id).is_some(),
            "Workspace {id} does not exist"
        );
        Saver::open_managed(self.workspace_path(id), self.backups_dir(), wake)
    }

    /// Marks `id` as the active workspace and persists `workspaces.json`.
    pub fn set_active_workspace(&mut self, id: Uuid) -> Result<()> {
        let now = now_unix_seconds();
        let entry = self
            .registry
            .workspaces
            .iter_mut()
            .find(|entry| entry.id == id)
            .context("Workspace does not exist")?;
        entry.last_opened_at = now;
        self.registry.active_workspace = id;
        self.save_registry()
    }

    /// Creates a new empty workspace with a validated unique name.
    pub fn create_workspace(&mut self, name: &str) -> Result<WorkspaceEntry> {
        let name = validate_workspace_name(name)?;
        anyhow::ensure!(
            !self.registry.name_taken(&name, None),
            "A workspace named \"{name}\" already exists."
        );
        let now = now_unix_seconds();
        let entry = WorkspaceEntry {
            id: Uuid::new_v4(),
            name,
            created_at: now,
            last_opened_at: now,
        };
        let path = self.workspace_path(entry.id);
        let file = WorkspaceFile::acquire_managed(path, self.backups_dir())?;
        file.save(&Workspace::default())?;
        drop(file);
        self.registry.workspaces.push(entry.clone());
        self.save_registry()?;
        Ok(entry)
    }

    /// Renames workspace `id`.
    pub fn rename_workspace(&mut self, id: Uuid, name: &str) -> Result<WorkspaceEntry> {
        let name = validate_workspace_name(name)?;
        anyhow::ensure!(
            !self.registry.name_taken(&name, Some(id)),
            "A workspace named \"{name}\" already exists."
        );
        let entry = self
            .registry
            .workspaces
            .iter_mut()
            .find(|entry| entry.id == id)
            .context("Workspace does not exist")?;
        entry.name = name;
        let updated = entry.clone();
        self.save_registry()?;
        Ok(updated)
    }

    /// Duplicates workspace `id` into a new workspace with freshly generated
    /// UUIDs so it never shares Keychain secrets or caches with the source.
    pub fn duplicate_workspace(&mut self, id: Uuid, name: Option<&str>) -> Result<WorkspaceEntry> {
        let source = self
            .workspace_entry(id)
            .context("Workspace does not exist")?
            .clone();
        let target_name = match name {
            Some(name) => {
                let validated = validate_workspace_name(name)?;
                anyhow::ensure!(
                    !self.registry.name_taken(&validated, None),
                    "A workspace named \"{validated}\" already exists."
                );
                validated
            }
            None => copied_workspace_name(&source.name, |candidate| {
                self.registry.name_taken(candidate, None)
            }),
        };

        let mut workspace = super::load(&self.workspace_path(id))?;
        workspace.remap_ids_for_copy();

        let now = now_unix_seconds();
        let entry = WorkspaceEntry {
            id: Uuid::new_v4(),
            name: target_name,
            created_at: now,
            last_opened_at: now,
        };
        let file =
            WorkspaceFile::acquire_managed(self.workspace_path(entry.id), self.backups_dir())?;
        file.save(&workspace)?;
        drop(file);
        self.registry.workspaces.push(entry.clone());
        self.save_registry()?;
        Ok(entry)
    }

    /// Deletes workspace `id`, removing its Keychain passwords and sign-in
    /// tokens, its data directory, and its local caches/transcripts.
    ///
    /// Refuses to delete the last remaining workspace. The caller must release
    /// any active `Saver` holding `id`'s lock before deleting it.
    pub fn delete_workspace(
        &mut self,
        id: Uuid,
        credentials: &dyn Credentials,
        tokens: &dyn TokenStore,
    ) -> Result<()> {
        anyhow::ensure!(
            self.registry.workspaces.len() > 1,
            "Cannot delete the only workspace."
        );
        let position = self
            .registry
            .workspaces
            .iter()
            .position(|entry| entry.id == id)
            .context("Workspace does not exist")?;

        // Best-effort read of the workspace to clean up its Keychain secrets.
        if let Ok(workspace) = super::load(&self.workspace_path(id)) {
            for profile in &workspace.profiles {
                let _ = credentials.delete_password(profile.id);
            }
            for sign_in in &workspace.sign_ins {
                let _ = tokens.delete_tokens(sign_in.id);
            }
        }

        self.registry.workspaces.remove(position);
        if self.registry.active_workspace == id {
            let next = position.min(self.registry.workspaces.len() - 1);
            self.registry.active_workspace = self.registry.workspaces[next].id;
        }
        self.save_registry()?;

        let data_ws = self.workspace_dir(id);
        if data_ws.exists() {
            let _ = fs::remove_dir_all(&data_ws);
        }
        let local_ws = self.workspace_local_dir(id);
        if local_ws.exists() {
            let _ = fs::remove_dir_all(&local_ws);
        }
        Ok(())
    }

    fn save_registry(&self) -> Result<()> {
        let mut value = serde_json::to_value(&self.registry)?;
        self.registry_unknown.restore(&mut value);
        migrations::stamp_envelope(&mut value, &REGISTRY_SPEC, self.registry_preserved_version);
        write_atomically(
            &registry_path(&self.data_dir),
            &serde_json::to_vec_pretty(&value)?,
        )
    }
}

fn acquire_lock(lock_path: &Path, label: &str) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(lock_path)
        .with_context(|| format!("Could not open {label} lock"))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(fs::TryLockError::WouldBlock) => anyhow::bail!(
            "This {label} is already open in another Qrow process. Use that process or choose a different workspace directory"
        ),
        Err(error) => Err(error).with_context(|| format!("Could not lock {label}")),
    }
}

fn initialize_fresh_store(data_dir: &Path) -> Result<()> {
    let now = now_unix_seconds();
    let entry = WorkspaceEntry {
        id: Uuid::new_v4(),
        name: DEFAULT_WORKSPACE_NAME.into(),
        created_at: now,
        last_opened_at: now,
    };
    let settings_file = SettingsFile {
        path: settings_path(data_dir),
        backup_dir: backups_directory(data_dir),
        unknown: Unknown::default(),
        preserved_version: None,
    };
    if !settings_path(data_dir).exists() {
        settings_file.save(&Settings::default())?;
    }
    let ws_file = WorkspaceFile::acquire_managed(
        workspace_file_path(data_dir, entry.id),
        backups_directory(data_dir),
    )?;
    ws_file.save(&Workspace::default())?;
    drop(ws_file);

    let registry = WorkspaceRegistry::single(entry);
    let mut value = serde_json::to_value(&registry)?;
    migrations::stamp_envelope(&mut value, &REGISTRY_SPEC, None);
    write_atomically(
        &registry_path(data_dir),
        &serde_json::to_vec_pretty(&value)?,
    )
}

/// One-time atomic layout upgrade from legacy `<data_dir>/workspace.json`
/// (`v1..=v8`) to `settings.json` + `workspaces.json` +
/// `workspaces/<default-id>/workspace.json` (`v9`).
fn migrate_legacy_layout(data_dir: &Path, local_dir: &Path, legacy_path: &Path) -> Result<()> {
    // Claim the legacy workspace lock too so a competing older Qrow process
    // cannot be writing while we migrate.
    let _legacy_lock = acquire_lock(&legacy_path.with_extension("lock"), "workspace")?;

    let raw = fs::read(legacy_path).context("Could not read saved workspace")?;
    let MigratedDocument {
        value,
        written_by,
        original_version,
        preserved_version,
        ..
    } = migrations::migrate_bytes(&raw, &WORKSPACE_SPEC)?;

    // Validate by deserializing before making any mutations on disk.
    let (mut workspace, workspace_unknown): (Workspace, Unknown) =
        Unknown::read_value(&value, &ENVELOPE_KEYS)
            .context("Saved workspace is invalid; the original file has been left untouched")?;
    workspace.normalize();
    workspace.settings.sanitize();

    // 1. Back up the original legacy file verbatim.
    let backups = backups_directory(data_dir);
    let _ = migrations::back_up_version(legacy_path, &backups, original_version, &raw)?;

    // 2. Write `settings.json`, preserving any unknown fields from the legacy
    //    `settings` object verbatim.
    let settings_json_path = settings_path(data_dir);
    if !settings_json_path.exists() {
        let mut settings_doc = serde_json::to_value(SavedSettings {
            version: SETTINGS_SPEC.current,
            settings: workspace.settings.clone(),
        })?;
        // Preserve any unknown fields that existed inside legacy `settings`.
        if let Some(legacy_settings) = value.get("settings")
            && let Ok((_, settings_unknown)) = Unknown::read_value::<Settings>(legacy_settings, &[])
            && let Some(target_settings) = settings_doc.get_mut("settings")
        {
            settings_unknown.restore(target_settings);
        }
        migrations::stamp_envelope(&mut settings_doc, &SETTINGS_SPEC, None);
        if let Some(writer) = &written_by
            && let Some(obj) = settings_doc.as_object_mut()
            && value
                .get("settings")
                .and_then(|s| Unknown::read_value::<Settings>(s, &[]).ok())
                .is_some_and(|(_, u)| !u.is_empty())
        {
            obj.insert(migrations::WRITTEN_BY.into(), Value::from(writer.clone()));
        }
        write_atomically(
            &settings_json_path,
            &serde_json::to_vec_pretty(&settings_doc)?,
        )?;
    }

    // 3. Create the Default workspace and write `workspaces/<id>/workspace.json`
    //    without embedded `settings`, preserving workspace-level unknown fields.
    let now = now_unix_seconds();
    let entry = WorkspaceEntry {
        id: Uuid::new_v4(),
        name: DEFAULT_WORKSPACE_NAME.into(),
        created_at: now,
        last_opened_at: now,
    };
    let ws_path = workspace_file_path(data_dir, entry.id);
    fs::create_dir_all(ws_path.parent().context("Invalid workspace path")?)
        .context("Could not create workspace directory")?;

    let mut ws_value = serde_json::to_value(&workspace)?;
    workspace_unknown.restore(&mut ws_value);
    if let Some(object) = ws_value.as_object_mut() {
        object.remove("settings");
    }
    migrations::stamp_envelope(&mut ws_value, &WORKSPACE_SPEC, preserved_version);
    write_atomically(&ws_path, &serde_json::to_vec_pretty(&ws_value)?)?;

    // 4. Relocate machine-local workspace folders (catalog, dbt, transcripts).
    let target_local = workspace_local_directory(local_dir, entry.id);
    relocate_local_subdirs(local_dir, &target_local)?;

    // 5. Commit point: write `workspaces.json`.
    let registry = WorkspaceRegistry::single(entry);
    let mut reg_value = serde_json::to_value(&registry)?;
    migrations::stamp_envelope(&mut reg_value, &REGISTRY_SPEC, None);
    write_atomically(
        &registry_path(data_dir),
        &serde_json::to_vec_pretty(&reg_value)?,
    )?;

    // 6. Remove legacy root `workspace.json` (original is safely in `backups/`).
    let _ = fs::remove_file(legacy_path);
    Ok(())
}

fn relocate_local_subdirs(local_dir: &Path, target_local: &Path) -> Result<()> {
    let transcripts = PathBuf::from("assistant").join("transcripts");
    for rel in [
        Path::new("catalog"),
        Path::new("dbt"),
        transcripts.as_path(),
    ] {
        let src = local_dir.join(rel);
        if !src.exists() {
            continue;
        }
        let dst = target_local.join(rel);
        if dst.exists() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::rename(&src, &dst).is_err() {
            copy_dir_recursive(&src, &dst)?;
            let _ = fs::remove_dir_all(&src);
        }
    }
    // Remove empty legacy `assistant` wrapper directory if transcripts moved out.
    let legacy_assistant = local_dir.join("assistant");
    if legacy_assistant.is_dir()
        && fs::read_dir(&legacy_assistant)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
    {
        let _ = fs::remove_dir(&legacy_assistant);
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
