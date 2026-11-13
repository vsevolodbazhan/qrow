use crate::sync::LockExt as _;
use crate::{
    catalog::{CATALOG_VERSION, Catalog},
    model::{WORKSPACE_VERSION, Workspace},
};
use anyhow::{Context, Result};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};
use uuid::Uuid;
use zeroize::Zeroizing;

mod unknown;
use unknown::Unknown;

/// The top-level key that names the Qrow version that last saved the
/// workspace. It belongs to the file, not to the workspace model.
const WRITTEN_BY: &str = "written_by";

pub fn workspace_path() -> PathBuf {
    if let Some(path) = std::env::var_os("QROW_DATA_DIR") {
        return PathBuf::from(path).join("workspace.json");
    }
    #[cfg(target_os = "linux")]
    {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        linux_workspace_path(&home, std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
    }
    #[cfg(target_os = "windows")]
    {
        windows_workspace_path(
            std::env::var_os("APPDATA").map(PathBuf::from),
            home_dir().unwrap_or_default(),
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    home_dir()
        .unwrap_or_default()
        .join("Library/Application Support/Qrow/workspace.json")
}

/// The folder of the files that stay on this computer: the schema caches,
/// the dbt indexes, and the assistant transcripts. On Windows, the default
/// workspace is in the roaming profile, and these files are in the local
/// profile, so they do not roam. Elsewhere, and with `QROW_DATA_DIR`, they
/// are next to the workspace file.
pub fn local_data_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("QROW_DATA_DIR") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "windows")]
    {
        windows_local_directory(
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            home_dir().unwrap_or_default(),
        )
    }
    #[cfg(not(target_os = "windows"))]
    workspace_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// Resolve the home directory for CLI configuration and user paths.
pub fn home_dir() -> Option<PathBuf> {
    resolve_home(std::env::var_os("HOME"), std::env::var_os("USERPROFILE"))
}

fn resolve_home(
    home: Option<std::ffi::OsString>,
    profile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    home.filter(|value| !value.is_empty())
        .or_else(|| profile.filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

#[cfg(any(windows, test))]
fn windows_workspace_path(appdata: Option<PathBuf>, home: PathBuf) -> PathBuf {
    appdata
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join("AppData/Roaming"))
        .join("Qrow/workspace.json")
}

#[cfg(any(windows, test))]
fn windows_local_directory(local: Option<PathBuf>, home: PathBuf) -> PathBuf {
    local
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join("AppData/Local"))
        .join("Qrow")
}

#[cfg(test)]
#[test]
fn home_directory_fallback() {
    assert_eq!(
        resolve_home(Some("".into()), Some("profile".into())),
        Some(PathBuf::from("profile"))
    );
    assert_eq!(
        resolve_home(Some("home".into()), Some("profile".into())),
        Some(PathBuf::from("home"))
    );
    assert_eq!(resolve_home(None, None), None);
}

#[cfg(test)]
#[test]
fn roaming_workspace_fallback() {
    let home = std::env::temp_dir().join("synthetic-home");
    let data = std::env::temp_dir().join("synthetic-roaming");
    assert_eq!(
        windows_workspace_path(Some(data.clone()), home.clone()),
        data.join("Qrow/workspace.json")
    );
    assert_eq!(
        windows_workspace_path(None, home.clone()),
        home.join("AppData/Roaming/Qrow/workspace.json")
    );
}

#[cfg(test)]
#[test]
fn local_data_fallback() {
    let home = std::env::temp_dir().join("synthetic-home");
    let local = std::env::temp_dir().join("synthetic-local");
    assert_eq!(
        windows_local_directory(Some(local.clone()), home.clone()),
        local.join("Qrow")
    );
    for local in [None, Some(PathBuf::from("relative"))] {
        assert_eq!(
            windows_local_directory(local, home.clone()),
            home.join("AppData/Local/Qrow")
        );
    }
}

#[cfg(target_os = "linux")]
fn linux_workspace_path(home: &Path, data: Option<PathBuf>) -> PathBuf {
    data.filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"))
        .join("qrow/workspace.json")
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn linux_workspace_uses_absolute_xdg_data_home() {
    let home = Path::new("/home/example");
    assert_eq!(
        linux_workspace_path(home, Some(PathBuf::from("/tmp/data"))),
        PathBuf::from("/tmp/data/qrow/workspace.json")
    );
    for data in [None, Some(PathBuf::from("relative")), Some(PathBuf::new())] {
        assert_eq!(
            linux_workspace_path(home, data),
            PathBuf::from("/home/example/.local/share/qrow/workspace.json")
        );
    }
}

/// Read a snapshot without claiming write access, for the read-only probe.
pub fn load(path: &Path) -> Result<Workspace> {
    Ok(read(path)?.workspace)
}

/// A workspace file as read, with what this build does not know.
struct Read {
    workspace: Workspace,
    unknown: Unknown,
    /// The Qrow version that saved the file, if it recorded one.
    written_by: Option<String>,
    data: Vec<u8>,
}

fn read(path: &Path) -> Result<Read> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Read {
                workspace: Workspace::default(),
                unknown: Unknown::default(),
                written_by: None,
                data: Vec::new(),
            });
        }
        Err(error) => return Err(error).context("Could not read saved workspace"),
    };
    let (mut workspace, unknown): (Workspace, Unknown) = Unknown::read(&data, &[WRITTEN_BY])
        .context("Saved workspace is invalid; the original file has been left untouched")?;
    let written_by = serde_json::from_slice::<serde_json::Value>(&data)
        .ok()
        .and_then(|value| value.get(WRITTEN_BY)?.as_str().map(str::to_owned));
    anyhow::ensure!(
        (1..=WORKSPACE_VERSION).contains(&workspace.version),
        "Unsupported workspace version {}; the file has been left untouched",
        workspace.version
    );
    if workspace.tabs.is_empty() {
        workspace.tabs = Workspace::default().tabs;
    }
    workspace.active_tab = workspace.active_tab.min(workspace.tabs.len() - 1);
    workspace.normalize();
    workspace.settings.sanitize();
    Ok(Read {
        workspace,
        unknown,
        written_by,
        data,
    })
}

/// Exclusive write access. The OS releases the lock even if the process crashes.
pub struct WorkspaceFile {
    path: PathBuf,
    /// Fields of a newer Qrow that each save keeps.
    unknown: Unknown,
    _lock: fs::File,
}

/// What a workspace file held that this build does not know.
#[derive(Clone, Debug, PartialEq)]
pub struct NewerData {
    /// The Qrow version that saved the file, if it recorded one.
    pub written_by: Option<String>,
    /// The copy of the file before this build saved it.
    pub backup: PathBuf,
}

impl NewerData {
    /// A short notice for the window.
    pub fn notice(&self) -> String {
        let writer = match &self.written_by {
            Some(version) => format!("Qrow {version}"),
            None => "a newer version of Qrow".into(),
        };
        format!(
            "This workspace has settings from {writer} that this version does not use. \
             Qrow keeps them when it saves. A copy of the file is at {}.",
            self.backup.display()
        )
    }
}

impl WorkspaceFile {
    pub fn acquire(path: PathBuf) -> Result<Self> {
        let parent = path.parent().context("Invalid workspace path")?;
        fs::create_dir_all(parent).context("Could not create workspace directory")?;
        let path = parent
            .canonicalize()?
            .join(path.file_name().context("Invalid workspace path")?);
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(path.with_extension("lock"))
            .context("Could not open workspace lock")?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => anyhow::bail!(
                "This workspace is already open in another Qrow process. Use that process or choose a different workspace directory"
            ),
            Err(error) => return Err(error).context("Could not lock workspace"),
        }
        // Never unlink the lock file: another process could lock a different inode.
        Ok(Self {
            path,
            unknown: Unknown::default(),
            _lock: lock,
        })
    }

    /// Reads the workspace. When it has fields that this build does not
    /// know, saves a backup of the file and reports it.
    pub fn load(&mut self) -> Result<(Workspace, Option<NewerData>)> {
        let read = read(&self.path)?;
        let newer = if read.unknown.is_empty() {
            None
        } else {
            Some(NewerData {
                written_by: read.written_by,
                backup: unknown::back_up(&self.path, &read.data)?,
            })
        };
        self.unknown = read.unknown;
        Ok((read.workspace, newer))
    }

    /// Writes `workspace` with the kept fields and the version of this build.
    pub fn save(&self, workspace: &Workspace) -> Result<()> {
        let mut value = serde_json::to_value(workspace)?;
        self.unknown.restore(&mut value);
        if let Some(object) = value.as_object_mut() {
            object.insert(WRITTEN_BY.into(), crate::build_info::RELEASE_VERSION.into());
        }
        write_atomically(&self.path, &serde_json::to_vec_pretty(&value)?)
    }
}

/// Replace `path` with `data` so that a crash leaves the old or the new file,
/// never a partial one. Only the owner can read the file.
pub(crate) fn write_atomically(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().context("Invalid file path")?;
    let temporary = TemporaryFile(path.with_extension(format!("{}.tmp", Uuid::new_v4())));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary.0)?;
    file.write_all(data)?;
    file.sync_all()?;
    fs::rename(&temporary.0, path)?;
    // Unix records the rename durably only after a sync of the folder.
    // Windows cannot open a folder as a file, and NTFS journals the rename.
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The schema cache of a profile in the [local data directory](local_data_directory)
/// `local`. The workspace lock also protects this directory.
pub fn catalog_cache_path(local: &Path, profile: Uuid) -> PathBuf {
    local.join("catalog").join(format!("{profile}.json"))
}

/// The folder of the saved dbt indexes in the local data directory `local`.
/// The workspace lock also protects this folder.
pub fn dbt_index_directory(local: &Path) -> PathBuf {
    local.join("dbt")
}

/// The folder of the saved assistant transcripts in the local data
/// directory `local`. The workspace lock also protects this folder.
pub fn transcripts_directory(local: &Path) -> PathBuf {
    local.join("assistant").join("transcripts")
}

/// Read a schema cache. A missing, unreadable, or outdated file gives `None`,
/// because Qrow can read the schemas from the server again.
pub fn load_catalog(path: &Path) -> Option<Catalog> {
    let data = fs::read(path).ok()?;
    let catalog: Catalog = serde_json::from_slice(&data).ok()?;
    (catalog.version == CATALOG_VERSION).then_some(catalog)
}

pub fn save_catalog(path: &Path, catalog: &Catalog) -> Result<()> {
    fs::create_dir_all(path.parent().context("Invalid schema cache path")?)
        .context("Could not create the schema cache directory")?;
    write_atomically(path, &serde_json::to_vec(catalog)?).context("Could not save the schema cache")
}

pub fn delete_catalog(path: &Path) {
    let _ = fs::remove_file(path);
}

pub type SaveResult = std::result::Result<(), String>;
pub type SaveReceipt = mpsc::Receiver<SaveResult>;
enum SaveCommand {
    Save(Workspace),
    Flush(Workspace, mpsc::Sender<SaveResult>),
    Stop,
}

pub struct Saver {
    tx: mpsc::Sender<SaveCommand>,
    pub errors: mpsc::Receiver<String>,
    /// Set when the file has fields of a newer Qrow.
    pub newer: Option<NewerData>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Saver {
    /// Lock before reading so the initial snapshot cannot come from another writer.
    pub fn open(path: PathBuf, wake: impl Fn() + Send + 'static) -> Result<(Workspace, Self)> {
        let mut file = WorkspaceFile::acquire(path)?;
        let (workspace, newer) = file.load()?;
        let (tx, rx) = mpsc::channel();
        let (errors_tx, errors) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut pending = None;
            while let Some(command) = pending.take().or_else(|| rx.recv().ok()) {
                let (mut state, receipt) = match command {
                    SaveCommand::Save(state) => (state, None),
                    SaveCommand::Flush(state, receipt) => (state, Some(receipt)),
                    SaveCommand::Stop => break,
                };
                if receipt.is_none() {
                    while let Ok(next) = rx.try_recv() {
                        match next {
                            SaveCommand::Save(next) => state = next,
                            next => {
                                pending = Some(next);
                                break;
                            }
                        }
                    }
                }
                let result = file
                    .save(&state)
                    .map_err(|e| format!("Workspace save failed: {e:#}"));
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
            workspace,
            Self {
                tx,
                errors,
                newer,
                handle: Some(handle),
            },
        ))
    }

    pub fn save(&self, state: Workspace) -> Result<()> {
        self.tx
            .send(SaveCommand::Save(state))
            .context("Workspace saver stopped")
    }

    /// Acknowledge this exact snapshot without stopping the saver, so failures can be retried.
    pub fn flush(&self, state: Workspace) -> Result<SaveReceipt> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(SaveCommand::Flush(state, tx))
            .context("Workspace saver stopped")?;
        Ok(rx)
    }

    pub fn finish(&mut self, state: Workspace) -> Result<()> {
        self.flush(state)?
            .recv()
            .context("Workspace saver stopped before confirming the save")?
            .map_err(anyhow::Error::msg)?;
        self.stop()
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(handle) = self.handle.take() {
            let _ = self.tx.send(SaveCommand::Stop);
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("Workspace saver panicked"))?;
        }
        Ok(())
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        let _ = self.tx.send(SaveCommand::Stop);
    }
}

/// Connection passwords, keyed by profile ID. The application uses
/// [`Keychain`]; tests supply an in-memory store.
pub trait Credentials: Send + Sync {
    fn password(&self, id: Uuid) -> Result<Zeroizing<String>>;
    fn set_password(&self, id: Uuid, password: &str) -> Result<()>;
    /// Deleting a profile leaves its secret behind otherwise, keyed by a UUID
    /// that nothing refers to any more.
    fn delete_password(&self, id: Uuid) -> Result<()>;
}

/// The tokens of reusable sign-ins, keyed by sign-in ID. Each record is one
/// JSON document. The application uses [`Keychain`]; tests supply an
/// in-memory store.
pub trait TokenStore: Send + Sync {
    /// `None` when no record exists.
    fn load_tokens(&self, id: Uuid) -> Result<Option<Zeroizing<String>>>;
    fn save_tokens(&self, id: Uuid, record: &str) -> Result<()>;
    /// Succeeds when no record exists.
    fn delete_tokens(&self, id: Uuid) -> Result<()>;
}

/// Sign-in tokens in memory, for the demo and for tests. Nothing persists.
#[derive(Default)]
pub struct MemoryTokenStore(std::sync::Mutex<std::collections::HashMap<Uuid, Zeroizing<String>>>);

impl TokenStore for MemoryTokenStore {
    fn load_tokens(&self, id: Uuid) -> Result<Option<Zeroizing<String>>> {
        Ok(self.0.lock_unpoisoned().get(&id).cloned())
    }
    fn save_tokens(&self, id: Uuid, record: &str) -> Result<()> {
        self.0
            .lock_unpoisoned()
            .insert(id, Zeroizing::new(record.to_owned()));
        Ok(())
    }
    fn delete_tokens(&self, id: Uuid) -> Result<()> {
        self.0.lock_unpoisoned().remove(&id);
        Ok(())
    }
}

/// Connection passwords and sign-in tokens in the system secret store.
pub struct Keychain;

/// The Keychain service of connection passwords. Keep it stable: it gives
/// access to the passwords of existing profiles.
pub const PASSWORD_SERVICE: &str = "io.qrow.connection";
/// The Keychain service of sign-in tokens.
pub const TOKEN_SERVICE: &str = "io.qrow.sign-in";

#[cfg(target_os = "macos")]
const ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
impl TokenStore for Keychain {
    fn load_tokens(&self, id: Uuid) -> Result<Option<Zeroizing<String>>> {
        match security_framework::passwords::get_generic_password(TOKEN_SERVICE, &id.to_string()) {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                Ok(Some(Zeroizing::new(
                    String::from_utf8(bytes.to_vec()).context("The stored sign-in is invalid")?,
                )))
            }
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
            Err(error) => {
                Err(error).context("Could not read the sign-in tokens from macOS Keychain")
            }
        }
    }

    fn save_tokens(&self, id: Uuid, record: &str) -> Result<()> {
        security_framework::passwords::set_generic_password(
            TOKEN_SERVICE,
            &id.to_string(),
            record.as_bytes(),
        )
        .context("Could not save the sign-in tokens in macOS Keychain")
    }

    fn delete_tokens(&self, id: Uuid) -> Result<()> {
        let mut items = security_framework::item::ItemSearchOptions::new();
        items
            .class(security_framework::item::ItemClass::generic_password())
            .service(TOKEN_SERVICE)
            .account(&id.to_string());
        delete_keychain_tokens(items)
            .context("Could not delete the sign-in tokens from macOS Keychain")
    }
}

/// Recover from the legacy Keychain owner check without changing access rules.
/// The reference API discards its result, so success requires verified absence.
#[cfg(target_os = "macos")]
fn delete_keychain_tokens(
    mut items: security_framework::item::ItemSearchOptions,
) -> security_framework::base::Result<()> {
    use security_framework::item::{Limit, Reference, SearchResult};
    const INVALID_OWNER_EDIT: i32 = -25244;
    let original = match items.delete() {
        Ok(()) => return Ok(()),
        Err(error) if error.code() == ITEM_NOT_FOUND => return Ok(()),
        Err(error) if error.code() == INVALID_OWNER_EDIT => error,
        Err(error) => return Err(error),
    };
    // The caller supplies the exact generic-password service and UUID account.
    // Request references only: deletion does not need to read token data.
    items.load_refs(true).limit(Limit::All);
    match items.search() {
        Ok(found) => {
            for item in found {
                match item {
                    SearchResult::Ref(Reference::KeychainItem(item)) => item.delete(),
                    _ => return Err(original),
                }
            }
        }
        Err(error) if error.code() == ITEM_NOT_FOUND => return Ok(()),
        Err(_) => return Err(original),
    }
    match items.search() {
        Err(error) if error.code() == ITEM_NOT_FOUND => Ok(()),
        Ok(found) if found.is_empty() => Ok(()),
        _ => Err(original),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
impl TokenStore for Keychain {
    fn load_tokens(&self, _: Uuid) -> Result<Option<Zeroizing<String>>> {
        anyhow::bail!("Keychain requires macOS")
    }
    fn save_tokens(&self, _: Uuid, _: &str) -> Result<()> {
        anyhow::bail!("Keychain requires macOS")
    }
    fn delete_tokens(&self, _: Uuid) -> Result<()> {
        anyhow::bail!("Keychain requires macOS")
    }
}

#[cfg(target_os = "macos")]
impl Credentials for Keychain {
    fn password(&self, id: Uuid) -> Result<Zeroizing<String>> {
        let bytes = Zeroizing::new(security_framework::passwords::get_generic_password(PASSWORD_SERVICE, &id.to_string())
            .context("Could not read the password from macOS Keychain. Edit the connection to save a password")?);
        Ok(Zeroizing::new(String::from_utf8(bytes.to_vec())?))
    }

    fn set_password(&self, id: Uuid, password: &str) -> Result<()> {
        security_framework::passwords::set_generic_password(
            PASSWORD_SERVICE,
            &id.to_string(),
            password.as_bytes(),
        )
        .context("Could not save the password in macOS Keychain")
    }

    fn delete_password(&self, id: Uuid) -> Result<()> {
        security_framework::passwords::delete_generic_password(PASSWORD_SERVICE, &id.to_string())
            .context("Could not delete the password from macOS Keychain")
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
impl Credentials for Keychain {
    fn password(&self, _: Uuid) -> Result<Zeroizing<String>> {
        anyhow::bail!("Keychain requires macOS")
    }
    fn set_password(&self, _: Uuid, _: &str) -> Result<()> {
        anyhow::bail!("Keychain requires macOS")
    }
    fn delete_password(&self, _: Uuid) -> Result<()> {
        anyhow::bail!("Keychain requires macOS")
    }
}

#[cfg(target_os = "linux")]
mod linux_secrets {
    use super::*;

    fn attributes(service: &str, id: Uuid) -> [(String, String); 2] {
        [
            ("service".into(), service.into()),
            ("account".into(), id.to_string()),
        ]
    }

    fn load(service: &str, id: Uuid) -> Result<Option<Zeroizing<String>>> {
        futures_lite::future::block_on(async {
            let keyring = oo7::Keyring::new().await?;
            keyring.unlock().await?;
            let items = keyring.search_items(&attributes(service, id)).await?;
            match items.first() {
                Some(item) => {
                    item.unlock().await?;
                    let secret = item.secret().await?;
                    Ok::<_, anyhow::Error>(Some(Zeroizing::new(String::from_utf8(
                        secret.as_bytes().to_vec(),
                    )?)))
                }
                None => Ok(None),
            }
        })
        .context("Could not read from the system secret store")
    }

    fn save(service: &str, id: Uuid, value: &str) -> Result<()> {
        futures_lite::future::block_on(async {
            let keyring = oo7::Keyring::new().await?;
            keyring.unlock().await?;
            keyring
                .create_item("Qrow", &attributes(service, id), value, true)
                .await
        })
        .context("Could not save in the system secret store")
    }

    fn delete(service: &str, id: Uuid) -> Result<()> {
        futures_lite::future::block_on(async {
            let keyring = oo7::Keyring::new().await?;
            keyring.unlock().await?;
            keyring.delete(&attributes(service, id)).await
        })
        .context("Could not delete from the system secret store")
    }

    impl Credentials for Keychain {
        fn password(&self, id: Uuid) -> Result<Zeroizing<String>> {
            load(PASSWORD_SERVICE, id)?
                .context("Password not found. Edit the connection to save a password")
        }
        fn set_password(&self, id: Uuid, password: &str) -> Result<()> {
            save(PASSWORD_SERVICE, id, password)
        }
        fn delete_password(&self, id: Uuid) -> Result<()> {
            delete(PASSWORD_SERVICE, id)
        }
    }

    impl TokenStore for Keychain {
        fn load_tokens(&self, id: Uuid) -> Result<Option<Zeroizing<String>>> {
            load(TOKEN_SERVICE, id)
        }
        fn save_tokens(&self, id: Uuid, record: &str) -> Result<()> {
            save(TOKEN_SERVICE, id, record)
        }
        fn delete_tokens(&self, id: Uuid) -> Result<()> {
            delete(TOKEN_SERVICE, id)
        }
    }

    #[test]
    #[ignore = "requires an isolated Secret Service session"]
    fn isolated_secret_service_round_trip() {
        assert_eq!(
            std::env::var("QROW_ISOLATED_SECRET_TEST").as_deref(),
            Ok("1")
        );
        let store = Keychain;
        let id = Uuid::new_v4();
        assert!(store.load_tokens(id).unwrap().is_none());
        store.set_password(id, "synthetic-password").unwrap();
        store.save_tokens(id, "synthetic-token").unwrap();
        assert_eq!(&**store.password(id).unwrap(), "synthetic-password");
        assert_eq!(
            &**store.load_tokens(id).unwrap().unwrap(),
            "synthetic-token"
        );
        store.save_tokens(id, "replacement-token").unwrap();
        assert_eq!(
            &**store.load_tokens(id).unwrap().unwrap(),
            "replacement-token"
        );
        store.delete_tokens(id).unwrap();
        assert!(store.load_tokens(id).unwrap().is_none());
        assert_eq!(&**store.password(id).unwrap(), "synthetic-password");
        store.delete_password(id).unwrap();
        assert!(store.password(id).is_err());
        store.delete_tokens(id).unwrap();
    }
}

#[cfg(all(test, target_os = "macos"))]
mod keychain_tests {
    use super::*;
    use security_framework::{
        item::{ItemClass, ItemSearchOptions},
        os::macos::keychain::{CreateOptions, SecKeychain},
    };

    const TARGET: &str = "00000000-0000-4000-8000-000000000001";
    const OTHER: &str = "00000000-0000-4000-8000-000000000002";
    const OTHER_SERVICE: &str = "io.qrow.synthetic-other";
    const PASSWORD: &str = "qrow-synthetic-test-only";

    fn query(keychain: &SecKeychain, service: &str, account: &str) -> ItemSearchOptions {
        let mut items = ItemSearchOptions::new();
        items
            .keychains(std::slice::from_ref(keychain))
            .class(ItemClass::generic_password())
            .service(service)
            .account(account);
        items
    }

    #[test]
    #[ignore = "Child helper: the parent supplies an isolated keychain"]
    fn create_foreign_token_items() {
        let path = std::env::var_os("QROW_SYNTHETIC_KEYCHAIN").expect("isolated keychain path");
        let mut keychain = SecKeychain::open(path).unwrap();
        keychain.unlock(Some(PASSWORD)).unwrap();
        for (service, account) in [
            (TOKEN_SERVICE, TARGET),
            (TOKEN_SERVICE, OTHER),
            (OTHER_SERVICE, TARGET),
        ] {
            keychain
                .add_generic_password(service, account, b"synthetic-token")
                .unwrap();
        }
    }

    #[test]
    fn deletes_foreign_token_items_without_deleting_other_accounts_or_services() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("synthetic.keychain");
        let keychain = CreateOptions::new()
            .password(PASSWORD)
            .create(&path)
            .unwrap();
        // An item's creator basename differs from this test executable. That
        // reproduces SecItemDelete's owner-check error without user credentials.
        let creator = temporary.path().join("qrow-synthetic-keychain-creator");
        fs::copy(std::env::current_exe().unwrap(), &creator).unwrap();
        let output = std::process::Command::new(creator)
            .env("QROW_SYNTHETIC_KEYCHAIN", &path)
            .args([
                "--exact",
                "storage::keychain_tests::create_foreign_token_items",
                "--ignored",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            query(&keychain, TOKEN_SERVICE, TARGET)
                .delete()
                .unwrap_err()
                .code(),
            -25244
        );
        delete_keychain_tokens(query(&keychain, TOKEN_SERVICE, TARGET)).unwrap();
        assert_eq!(
            query(&keychain, TOKEN_SERVICE, TARGET)
                .load_refs(true)
                .search()
                .unwrap_err()
                .code(),
            ITEM_NOT_FOUND
        );
        for (service, account) in [(TOKEN_SERVICE, OTHER), (OTHER_SERVICE, TARGET)] {
            assert_eq!(
                query(&keychain, service, account)
                    .load_refs(true)
                    .search()
                    .unwrap()
                    .len(),
                1
            );
        }
        // Repeated deletion of an absent token record succeeds.
        delete_keychain_tokens(query(&keychain, TOKEN_SERVICE, TARGET)).unwrap();
    }

    #[test]
    fn deletes_token_items_created_by_the_current_executable() {
        let temporary = tempfile::tempdir().unwrap();
        let keychain = CreateOptions::new()
            .password(PASSWORD)
            .create(temporary.path().join("synthetic.keychain"))
            .unwrap();
        keychain
            .add_generic_password(TOKEN_SERVICE, TARGET, b"synthetic-token")
            .unwrap();
        delete_keychain_tokens(query(&keychain, TOKEN_SERVICE, TARGET)).unwrap();
        assert_eq!(
            query(&keychain, TOKEN_SERVICE, TARGET)
                .load_refs(true)
                .search()
                .unwrap_err()
                .code(),
            ITEM_NOT_FOUND
        );
    }
}

#[cfg(windows)]
mod windows_secrets {
    use super::*;
    use credential_chunks::Blobs;

    /// Windows generic credentials of one service.
    struct CredentialManager {
        service: &'static str,
    }

    impl CredentialManager {
        fn entry(&self, target: &str) -> Result<keyring::Entry> {
            Ok(keyring::Entry::new_with_target(
                target,
                self.service,
                target,
            )?)
        }
    }

    impl Blobs for CredentialManager {
        fn get(&self, target: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
            match self.entry(target)?.get_secret() {
                Ok(secret) => Ok(Some(Zeroizing::new(secret))),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(error) => Err(error).context("Could not read Windows Credential Manager"),
            }
        }

        fn set(&self, target: &str, bytes: &[u8]) -> Result<()> {
            self.entry(target)?
                .set_secret(bytes)
                .context("Could not write Windows Credential Manager")
        }

        fn delete(&self, target: &str) -> Result<()> {
            match self.entry(target)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(error).context("Could not delete Windows credential"),
            }
        }
    }

    fn load(service: &'static str, id: Uuid) -> Result<Option<Zeroizing<String>>> {
        credential_chunks::load(
            &CredentialManager { service },
            &credential_target(service, id),
        )
    }

    fn save(service: &'static str, id: Uuid, secret: &str) -> Result<()> {
        credential_chunks::save(
            &CredentialManager { service },
            &credential_target(service, id),
            secret,
        )
    }

    fn delete(service: &'static str, id: Uuid) -> Result<()> {
        credential_chunks::delete(
            &CredentialManager { service },
            &credential_target(service, id),
        )
    }

    impl Credentials for Keychain {
        fn password(&self, id: Uuid) -> Result<Zeroizing<String>> {
            load(PASSWORD_SERVICE, id)?
                .context("No saved password. Edit the connection to save a password")
        }
        fn set_password(&self, id: Uuid, password: &str) -> Result<()> {
            save(PASSWORD_SERVICE, id, password)
        }
        fn delete_password(&self, id: Uuid) -> Result<()> {
            delete(PASSWORD_SERVICE, id)
        }
    }

    impl TokenStore for Keychain {
        fn load_tokens(&self, id: Uuid) -> Result<Option<Zeroizing<String>>> {
            load(TOKEN_SERVICE, id)
        }
        fn save_tokens(&self, id: Uuid, record: &str) -> Result<()> {
            save(TOKEN_SERVICE, id, record)
        }
        fn delete_tokens(&self, id: Uuid) -> Result<()> {
            delete(TOKEN_SERVICE, id)
        }
    }

    #[test]
    #[ignore = "uses synthetic Windows Credential Manager entries"]
    fn credential_round_trip() {
        let id = Uuid::new_v4();
        let store = Keychain;
        store.delete_password(id).unwrap();
        assert!(store.load_tokens(id).unwrap().is_none());
        store.set_password(id, "synthetic-密碼").unwrap();
        store.save_tokens(id, "synthetic-token").unwrap();
        assert_eq!(store.password(id).unwrap().as_str(), "synthetic-密碼");
        store.delete_password(id).unwrap();
        assert_eq!(
            store.load_tokens(id).unwrap().unwrap().as_str(),
            "synthetic-token"
        );
        // A sign-in record with large tokens needs several credentials.
        let record = Zeroizing::new("synthetic-tokens-".repeat(600));
        store.save_tokens(id, &record).unwrap();
        assert_eq!(
            store.load_tokens(id).unwrap().unwrap().as_str(),
            record.as_str()
        );
        store.save_tokens(id, "synthetic-token").unwrap();
        assert_eq!(
            store.load_tokens(id).unwrap().unwrap().as_str(),
            "synthetic-token"
        );
        store.delete_tokens(id).unwrap();
        store.delete_tokens(id).unwrap();
        assert!(store.load_tokens(id).unwrap().is_none());
    }
}

/// Splits a secret over several small records. A Windows generic credential
/// holds at most 2560 bytes, and a sign-in record with its access and refresh
/// tokens can be larger.
///
/// The record at the base target holds a header and the first part. The other
/// parts have targets with a random generation. A save writes the new parts
/// first and the base record last, so an interrupted save keeps the previous
/// secret readable. Then it deletes the parts of the previous generation.
#[cfg(any(windows, test))]
mod credential_chunks {
    use super::*;

    /// Windows limits a generic credential to 2560 bytes. Keep space for the
    /// header.
    pub(super) const PART_BYTES: usize = 2048;
    const MAGIC: &str = "qrow-secret-v1";

    pub(super) trait Blobs {
        fn get(&self, target: &str) -> Result<Option<Zeroizing<Vec<u8>>>>;
        fn set(&self, target: &str, bytes: &[u8]) -> Result<()>;
        /// Succeeds when the record does not exist.
        fn delete(&self, target: &str) -> Result<()>;
    }

    struct Header {
        generation: String,
        parts: usize,
    }

    fn part_target(base: &str, generation: &str, index: usize) -> String {
        format!("{base}/{generation}/{index}")
    }

    /// Splits a base record into its header and its first part.
    fn parse(record: &[u8]) -> Result<(Header, &[u8])> {
        let end = record
            .iter()
            .position(|&byte| byte == b'\n')
            .context("The saved secret has no header")?;
        let header = std::str::from_utf8(&record[..end])?;
        let mut fields = header.split(' ');
        anyhow::ensure!(
            fields.next() == Some(MAGIC),
            "The saved secret has an unknown format"
        );
        let generation = fields
            .next()
            .context("The saved secret has no generation")?;
        let parts: usize = fields
            .next()
            .context("The saved secret has no part count")?
            .parse()?;
        anyhow::ensure!(
            parts >= 1 && fields.next().is_none(),
            "The saved secret has an invalid header"
        );
        Ok((
            Header {
                generation: generation.to_owned(),
                parts,
            },
            &record[end + 1..],
        ))
    }

    fn header(store: &impl Blobs, base: &str) -> Option<Header> {
        let record = store.get(base).ok()??;
        parse(&record).ok().map(|(header, _)| header)
    }

    pub(super) fn load(store: &impl Blobs, base: &str) -> Result<Option<Zeroizing<String>>> {
        let Some(record) = store.get(base)? else {
            return Ok(None);
        };
        let (header, first) = parse(&record)?;
        let mut bytes = Zeroizing::new(first.to_vec());
        for index in 1..header.parts {
            let part = store
                .get(&part_target(base, &header.generation, index))?
                .context("A part of the saved secret is missing. Save it again")?;
            bytes.extend_from_slice(&part);
        }
        Ok(Some(Zeroizing::new(
            std::str::from_utf8(&bytes)?.to_owned(),
        )))
    }

    pub(super) fn save(store: &impl Blobs, base: &str, secret: &str) -> Result<()> {
        let previous = header(store, base);
        let generation = Uuid::new_v4().simple().to_string();
        let bytes = secret.as_bytes();
        let parts: Vec<&[u8]> = if bytes.is_empty() {
            vec![bytes]
        } else {
            bytes.chunks(PART_BYTES).collect()
        };
        for (index, part) in parts.iter().enumerate().skip(1) {
            store.set(&part_target(base, &generation, index), part)?;
        }
        let mut record =
            Zeroizing::new(format!("{MAGIC} {generation} {}\n", parts.len()).into_bytes());
        record.extend_from_slice(parts[0]);
        store.set(base, &record)?;
        if let Some(previous) = previous {
            // The new secret is complete. A stale part is no longer readable
            // through the base record, so a failed delete does not fail the save.
            for index in 1..previous.parts {
                let _ = store.delete(&part_target(base, &previous.generation, index));
            }
        }
        Ok(())
    }

    pub(super) fn delete(store: &impl Blobs, base: &str) -> Result<()> {
        let previous = header(store, base);
        store.delete(base)?;
        let mut result = Ok(());
        if let Some(previous) = previous {
            for index in 1..previous.parts {
                if let Err(error) = store.delete(&part_target(base, &previous.generation, index))
                    && result.is_ok()
                {
                    result = Err(error);
                }
            }
        }
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{cell::RefCell, collections::BTreeMap};

        /// Windows' limit of a generic credential.
        const LIMIT: usize = 2560;

        #[derive(Default)]
        struct Memory {
            records: RefCell<BTreeMap<String, Vec<u8>>>,
            fail_set: RefCell<Option<String>>,
        }

        impl Blobs for Memory {
            fn get(&self, target: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
                Ok(self
                    .records
                    .borrow()
                    .get(target)
                    .cloned()
                    .map(Zeroizing::new))
            }
            fn set(&self, target: &str, bytes: &[u8]) -> Result<()> {
                anyhow::ensure!(bytes.len() <= LIMIT, "record of {} bytes", bytes.len());
                anyhow::ensure!(
                    self.fail_set.borrow().as_deref() != Some(target),
                    "synthetic failure"
                );
                self.records
                    .borrow_mut()
                    .insert(target.into(), bytes.to_vec());
                Ok(())
            }
            fn delete(&self, target: &str) -> Result<()> {
                self.records.borrow_mut().remove(target);
                Ok(())
            }
        }

        const BASE: &str = "io.qrow.oidc/synthetic";

        #[test]
        fn secrets_of_any_size_round_trip_within_the_record_limit() {
            let store = Memory::default();
            for size in [
                0,
                1,
                PART_BYTES - 1,
                PART_BYTES,
                PART_BYTES + 1,
                3 * PART_BYTES,
                20_000,
            ] {
                let secret = "s".repeat(size);
                save(&store, BASE, &secret).unwrap();
                assert_eq!(load(&store, BASE).unwrap().unwrap().as_str(), secret);
                assert_eq!(
                    store.records.borrow().len(),
                    size.div_ceil(PART_BYTES).max(1)
                );
            }
        }

        #[test]
        fn multibyte_characters_survive_a_split_inside_a_character() {
            let store = Memory::default();
            let secret = "é".repeat(PART_BYTES);
            save(&store, BASE, &secret).unwrap();
            assert_eq!(load(&store, BASE).unwrap().unwrap().as_str(), secret);
        }

        #[test]
        fn a_smaller_save_and_a_delete_remove_old_parts() {
            let store = Memory::default();
            save(&store, BASE, &"a".repeat(5 * PART_BYTES)).unwrap();
            save(&store, BASE, "short").unwrap();
            assert_eq!(store.records.borrow().len(), 1);
            assert_eq!(load(&store, BASE).unwrap().unwrap().as_str(), "short");
            save(&store, BASE, &"b".repeat(3 * PART_BYTES)).unwrap();
            delete(&store, BASE).unwrap();
            assert!(store.records.borrow().is_empty());
            assert!(load(&store, BASE).unwrap().is_none());
            delete(&store, BASE).unwrap();
        }

        #[test]
        fn an_interrupted_save_keeps_the_previous_secret() {
            let store = Memory::default();
            let previous = "p".repeat(3 * PART_BYTES);
            save(&store, BASE, &previous).unwrap();
            *store.fail_set.borrow_mut() = Some(BASE.into());
            assert!(save(&store, BASE, &"n".repeat(4 * PART_BYTES)).is_err());
            assert_eq!(load(&store, BASE).unwrap().unwrap().as_str(), previous);
        }

        #[test]
        fn a_missing_part_or_an_unknown_record_is_an_error() {
            let store = Memory::default();
            save(&store, BASE, &"m".repeat(2 * PART_BYTES)).unwrap();
            let part = store
                .records
                .borrow()
                .keys()
                .find(|target| *target != BASE)
                .cloned()
                .unwrap();
            store.records.borrow_mut().remove(&part);
            assert!(load(&store, BASE).is_err());
            store
                .records
                .borrow_mut()
                .insert(BASE.into(), b"raw secret".to_vec());
            assert!(load(&store, BASE).is_err());
        }
    }
}

#[cfg(any(windows, test))]
fn credential_target(service: &str, id: Uuid) -> String {
    format!("{service}/{id}")
}

#[test]
fn credential_targets_separate_services() {
    let id = Uuid::nil();
    assert_eq!(
        credential_target(PASSWORD_SERVICE, id),
        "io.qrow.connection/00000000-0000-0000-0000-000000000000"
    );
    assert_ne!(
        credential_target(PASSWORD_SERVICE, id),
        credential_target(TOKEN_SERVICE, id)
    );
}
