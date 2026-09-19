use crate::model::Workspace;
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
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/Qrow/workspace.json")
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
        (1..=2).contains(&workspace.version),
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
        let parent = self.path.parent().context("Invalid workspace path")?;
        let temporary =
            TemporaryWorkspace(self.path.with_extension(format!("{}.tmp", Uuid::new_v4())));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary.0)?;
        serde_json::to_writer_pretty(&mut file, workspace)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary.0, &self.path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

struct TemporaryWorkspace(PathBuf);
impl Drop for TemporaryWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
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

#[cfg(target_os = "macos")]
pub fn password(id: Uuid) -> Result<Zeroizing<String>> {
    let bytes = Zeroizing::new(security_framework::passwords::get_generic_password("io.qrow.connection", &id.to_string())
        .context("Could not read the password from macOS Keychain. Edit the connection to save a password")?);
    Ok(Zeroizing::new(String::from_utf8(bytes.to_vec())?))
}

#[cfg(target_os = "macos")]
pub fn set_password(id: Uuid, password: &str) -> Result<()> {
    security_framework::passwords::set_generic_password(
        "io.qrow.connection",
        &id.to_string(),
        password.as_bytes(),
    )
    .context("Could not save the password in macOS Keychain")
}

/// Deleting a profile leaves its secret behind otherwise, keyed by a UUID that
/// nothing refers to any more.
#[cfg(target_os = "macos")]
pub fn delete_password(id: Uuid) -> Result<()> {
    security_framework::passwords::delete_generic_password("io.qrow.connection", &id.to_string())
        .context("Could not delete the password from macOS Keychain")
}

#[cfg(not(target_os = "macos"))]
pub fn password(_: Uuid) -> Result<Zeroizing<String>> {
    anyhow::bail!("Keychain requires macOS")
}
#[cfg(not(target_os = "macos"))]
pub fn set_password(_: Uuid, _: &str) -> Result<()> {
    anyhow::bail!("Keychain requires macOS")
}
#[cfg(not(target_os = "macos"))]
pub fn delete_password(_: Uuid) -> Result<()> {
    anyhow::bail!("Keychain requires macOS")
}
