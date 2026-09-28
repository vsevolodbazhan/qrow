//! Launches the real Qrow window headlessly with an isolated workspace and
//! synthetic passwords. Real worker threads do real I/O, so waits use wall time.
use anyhow::Result;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AnyWindowHandle, App, AppContext, TestAppContext, Window, px, size};
use qrow::{
    model::{WORKSPACE_VERSION, Workspace},
    storage::{self, Credentials},
    ui::{self, Environment, Qrow},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tempfile::TempDir;
use uuid::Uuid;
use zeroize::Zeroizing;

/// Synthetic passwords. Records reads so tests can prove Keychain was not used.
#[derive(Default)]
pub struct MemoryCredentials {
    passwords: Mutex<HashMap<Uuid, String>>,
    reads: AtomicUsize,
}

impl MemoryCredentials {
    pub fn get(&self, id: Uuid) -> Option<String> {
        self.passwords.lock().unwrap().get(&id).cloned()
    }
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

impl Credentials for MemoryCredentials {
    fn password(&self, id: Uuid) -> Result<Zeroizing<String>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.get(id)
            .map(Zeroizing::new)
            .ok_or_else(|| anyhow::anyhow!("No synthetic password for {id}"))
    }
    fn set_password(&self, id: Uuid, password: &str) -> Result<()> {
        self.passwords.lock().unwrap().insert(id, password.into());
        Ok(())
    }
    fn delete_password(&self, id: Uuid) -> Result<()> {
        self.passwords.lock().unwrap().remove(&id);
        Ok(())
    }
}

pub struct TestApp {
    pub window: AnyWindowHandle,
    pub credentials: Arc<MemoryCredentials>,
    workspace: PathBuf,
    _directory: TempDir,
}

impl TestApp {
    /// Opens Qrow on `workspace`, saved to a new temporary directory.
    pub fn launch(cx: &mut TestAppContext, workspace: Workspace) -> Self {
        Self::launch_with(cx, workspace, MemoryCredentials::default())
    }

    pub fn launch_with(
        cx: &mut TestAppContext,
        workspace: Workspace,
        credentials: MemoryCredentials,
    ) -> Self {
        // Worker and saver threads wake the UI. GPUI's deterministic
        // scheduler rejects wakes from other threads unless parking is allowed.
        cx.executor().allow_parking();
        cx.update(|cx| {
            ui::init(cx);
            // GPUI animations use wall time, which the test clock does not
            // control. Reduced motion settles dialogs on their first frame.
            cx.set_reduce_motion(true);
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspace.json");
        let workspace = Workspace {
            version: WORKSPACE_VERSION,
            ..workspace
        };
        std::fs::write(&path, serde_json::to_vec(&workspace).unwrap()).unwrap();
        let credentials = Arc::new(credentials);
        let environment = Environment::isolated(path.clone(), credentials.clone());
        let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
            let view = cx.new(|cx| Qrow::new(environment, Instant::now(), window, cx));
            ui::root(view, window, cx)
        });
        let app = Self {
            window: window.into(),
            credentials,
            workspace: path,
            _directory: directory,
        };
        app.update(cx, |window, cx| window.render_frame(cx));
        app
    }

    pub fn update<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> R {
        cx.update_window(self.window, |_, window, cx| f(window, cx))
            .expect("Qrow window closed")
    }

    /// Clicks `id`, selects its text, and types `text` in its place. Masked
    /// inputs do not publish their value, so only unmasked values are checked.
    pub fn fill(&self, cx: &mut TestAppContext, id: &'static str, text: &str) {
        self.update(cx, |window, cx| {
            window.click(id, cx);
            window.press("cmd-a", cx);
            window.input(text, cx);
            let input = window.find(id);
            assert_eq!(input.focused(), Some(true), "{id} did not take focus");
            if let Some(value) = input.value() {
                assert_eq!(value, text, "{id} did not accept the text");
            }
        });
    }

    pub fn workspace_path(&self) -> &Path {
        &self.workspace
    }

    /// The workspace as Qrow last saved it.
    pub fn saved(&self) -> Workspace {
        storage::load(&self.workspace).unwrap()
    }

    /// Polls `predicate` on fresh frames until it holds. Fails with the
    /// registered element paths after `timeout` of wall time.
    pub fn wait_until(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        timeout: Duration,
        mut predicate: impl FnMut(&mut Window, &mut App) -> bool,
    ) {
        let deadline = Instant::now() + timeout;
        loop {
            // Advance the test clock for Qrow's polling timers, then run the
            // tasks that real threads woke.
            cx.executor().advance_clock(Duration::from_millis(50));
            cx.run_until_parked();
            let ready = self.update(cx, |window, cx| {
                window.render_frame(cx);
                predicate(window, cx)
            });
            if ready {
                return;
            }
            if Instant::now() >= deadline {
                let labels = self.update(cx, |window, _| {
                    gpui_kit::base::test_support::snapshots(window)
                        .iter()
                        .filter_map(|element| element.label().map(str::to_owned))
                        .collect::<Vec<_>>()
                });
                let saved = std::fs::read_to_string(&self.workspace).unwrap_or_default();
                panic!(
                    "Timed out after {timeout:?} waiting for {what}.\nLabels: {labels:?}\nSaved workspace: {saved}"
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
