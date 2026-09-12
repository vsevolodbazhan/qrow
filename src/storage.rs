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

pub fn load(path: &Path) -> Result<Workspace> {
    if !path.exists() {
        return Ok(Workspace::default());
    }
    let data = fs::read(path).context("Could not read saved workspace")?;
    let mut workspace: Workspace = serde_json::from_slice(&data)
        .context("Saved workspace is invalid; the original file has been left untouched")?;
    anyhow::ensure!(
        workspace.version == 1,
        "Unsupported workspace version {}; the file has been left untouched",
        workspace.version
    );
    if workspace.tabs.is_empty() {
        workspace.tabs = Workspace::default().tabs;
    }
    workspace.active_tab = workspace.active_tab.min(workspace.tabs.len() - 1);
    workspace.settings.sanitize();
    Ok(workspace)
}

pub fn save(path: &Path, workspace: &Workspace) -> Result<()> {
    let parent = path.parent().context("Invalid workspace path")?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(workspace)?)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}

enum SaveCommand {
    Save(Workspace),
    Finish(Workspace),
}

pub struct Saver {
    tx: mpsc::Sender<SaveCommand>,
    pub errors: mpsc::Receiver<String>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Saver {
    pub fn new(path: PathBuf) -> Self {
        Self::with_wake(path, || {})
    }
    pub fn with_wake(path: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let (errors_tx, errors) = mpsc::channel();
        let handle = thread::spawn(move || {
            while let Ok(command) = rx.recv() {
                let (mut state, mut finish) = match command {
                    SaveCommand::Save(w) => (w, false),
                    SaveCommand::Finish(w) => (w, true),
                };
                while !finish {
                    match rx.try_recv() {
                        Ok(SaveCommand::Save(w)) => state = w,
                        Ok(SaveCommand::Finish(w)) => {
                            state = w;
                            finish = true;
                        }
                        Err(_) => break,
                    }
                }
                if let Err(e) = save(&path, &state) {
                    let _ = errors_tx.send(format!("Workspace save failed: {e:#}"));
                    wake();
                }
                if finish {
                    break;
                }
            }
        });
        Self {
            tx,
            errors,
            handle: Some(handle),
        }
    }
    pub fn save(&self, state: Workspace) {
        let _ = self.tx.send(SaveCommand::Save(state));
    }
    pub fn finish(&mut self, state: Workspace) {
        let _ = self.tx.send(SaveCommand::Finish(state));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(target_os = "macos")]
pub fn password(id: Uuid) -> Result<Zeroizing<String>> {
    let bytes = Zeroizing::new(security_framework::passwords::get_generic_password("io.qrow.connection", &id.to_string())
        .context("Could not read the password from macOS Keychain. Edit the connection to save a password.")?);
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

#[cfg(not(target_os = "macos"))]
pub fn password(_: Uuid) -> Result<Zeroizing<String>> {
    anyhow::bail!("Keychain requires macOS")
}
#[cfg(not(target_os = "macos"))]
pub fn set_password(_: Uuid, _: &str) -> Result<()> {
    anyhow::bail!("Keychain requires macOS")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_save_error_wakes_the_ui() {
        let dir = tempfile::tempdir().unwrap();
        let parent_file = dir.path().join("not-a-directory");
        fs::write(&parent_file, b"preserve").unwrap();
        let (notify, wake) = mpsc::channel();
        let mut saver = Saver::with_wake(parent_file.join("workspace.json"), move || {
            let _ = notify.send(());
        });
        saver.save(Workspace::default());
        wake.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(
            saver
                .errors
                .try_recv()
                .unwrap()
                .contains("Workspace save failed")
        );
        saver.finish(Workspace::default());
        assert_eq!(fs::read(parent_file).unwrap(), b"preserve");
    }

    #[test]
    fn workspace_roundtrip_and_corruption_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let mut state = Workspace::default();
        state.tabs[0].sql = "SELECT '日本語';".into();
        state.settings.ui_scale = 1.2;
        state.settings.editor_font_family = "Monaco".into();
        state.settings.editor_font_size = 16.;
        save(&path, &state).unwrap();
        let restored = load(&path).unwrap();
        assert_eq!(restored.tabs[0].sql, state.tabs[0].sql);
        assert_eq!(restored.settings, state.settings);
        fs::write(&path, b"broken").unwrap();
        assert!(load(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"broken");
    }
}
