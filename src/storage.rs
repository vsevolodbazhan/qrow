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
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Workspace::default());
        }
        Err(error) => return Err(error).context("Could not read saved workspace"),
    };
    let mut workspace: Workspace = serde_json::from_slice(&data)
        .context("Saved workspace is invalid; the original file has been left untouched")?;
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
    Ok(workspace)
}

/// Exclusive write access. The OS releases the lock even if the process crashes.
pub struct WorkspaceFile {
    path: PathBuf,
    _lock: fs::File,
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
        Ok(Self { path, _lock: lock })
    }

    pub fn load(&self) -> Result<Workspace> {
        load(&self.path)
    }

    pub fn save(&self, workspace: &Workspace) -> Result<()> {
        write_atomically(&self.path, &serde_json::to_vec_pretty(workspace)?)
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
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The schema cache of a profile, next to the workspace file. The workspace
/// lock also protects this directory.
pub fn catalog_path(workspace: &Path, profile: Uuid) -> PathBuf {
    workspace
        .parent()
        .unwrap_or(Path::new("."))
        .join("catalog")
        .join(format!("{profile}.json"))
}

/// The folder of the saved dbt indexes, next to the workspace file. The
/// workspace lock also protects this folder.
pub fn dbt_directory(workspace: &Path) -> PathBuf {
    workspace.parent().unwrap_or(Path::new(".")).join("dbt")
}

/// The folder of the saved assistant transcripts, next to the workspace
/// file. The workspace lock also protects this folder.
pub fn assistant_transcripts_directory(workspace: &Path) -> PathBuf {
    workspace
        .parent()
        .unwrap_or(Path::new("."))
        .join("assistant")
        .join("transcripts")
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
    handle: Option<thread::JoinHandle<()>>,
}

impl Saver {
    /// Lock before reading so the initial snapshot cannot come from another writer.
    pub fn open(path: PathBuf, wake: impl Fn() + Send + 'static) -> Result<(Workspace, Self)> {
        let file = WorkspaceFile::acquire(path)?;
        let workspace = file.load()?;
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

    fn entry(service: &str, id: Uuid) -> Result<keyring::Entry> {
        Ok(keyring::Entry::new_with_target(
            &credential_target(service, id),
            service,
            &id.to_string(),
        )?)
    }

    fn load(service: &str, id: Uuid) -> Result<Option<Zeroizing<String>>> {
        match entry(service, id)?.get_password() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error).context("Could not read Windows Credential Manager"),
        }
    }

    fn save(service: &str, id: Uuid, secret: &str) -> Result<()> {
        entry(service, id)?
            .set_password(secret)
            .context("Could not write Windows Credential Manager")
    }

    fn delete(service: &str, id: Uuid) -> Result<()> {
        match entry(service, id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error).context("Could not delete Windows credential"),
        }
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
        store.delete_tokens(id).unwrap();
        store.delete_tokens(id).unwrap();
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
