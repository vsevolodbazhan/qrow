use crate::{
    storage::{self, Credentials, Keychain, MemoryTokenStore, TokenStore},
    tls::Trust,
};
use std::{path::PathBuf, sync::Arc};

/// Opens the authorization URL of a browser sign-in. It runs on a background
/// thread.
pub type Browser = Arc<dyn Fn(&str) -> anyhow::Result<()> + Send + Sync>;

/// Where a Qrow window keeps its workspace, connection passwords, and
/// sign-in tokens, and which certificate authorities it trusts.
#[derive(Clone)]
pub struct Environment {
    /// Managed multi-workspace root directory (`settings.json`, `workspaces.json`,
    /// `workspaces/<id>/`). When `Some`, takes precedence over `workspace`.
    data_dir: Option<PathBuf>,
    /// Standalone single workspace file path (for isolated tests). `None` with
    /// `data_dir == None` shows the built-in demo data and saves nothing.
    workspace: Option<PathBuf>,
    /// The folder of the caches and transcripts. See
    /// [`storage::local_data_directory`].
    local: Option<PathBuf>,
    credentials: Arc<dyn Credentials>,
    tokens: Arc<dyn TokenStore>,
    trust: Trust,
    /// `None` opens the default browser of the system.
    browser: Option<Browser>,
    /// How this copy updates. `None` never checks for updates.
    updates: Option<Arc<crate::update::Setup>>,
}

impl Environment {
    /// The user's multi-workspace data directory, system secret store, and
    /// system trust store.
    pub fn user() -> Self {
        Self {
            data_dir: Some(storage::data_directory()),
            workspace: None,
            local: Some(storage::local_data_directory()),
            credentials: Arc::new(Keychain),
            tokens: Arc::new(Keychain),
            trust: Trust::system(),
            browser: None,
            // Only release builds have an update setup.
            updates: crate::update::Setup::for_this_build(
                storage::local_data_directory().join("updates"),
            )
            .map(Arc::new),
        }
    }

    /// Built-in demo data. The demo does not read, save, or delete passwords
    /// or tokens.
    pub fn demo() -> Self {
        Self {
            data_dir: None,
            workspace: None,
            local: None,
            credentials: Arc::new(Keychain),
            tokens: Arc::new(MemoryTokenStore::default()),
            trust: Trust::system(),
            browser: None,
            updates: None,
        }
    }

    /// A single workspace file and a password store chosen by the caller, for
    /// example a temporary directory and synthetic passwords in tests.
    /// Caches and transcripts are next to the workspace file. Sign-in tokens
    /// stay in memory.
    pub fn isolated(workspace: PathBuf, credentials: Arc<dyn Credentials>) -> Self {
        Self {
            data_dir: None,
            local: Some(
                workspace
                    .parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(".")),
            ),
            workspace: Some(workspace),
            credentials,
            tokens: Arc::new(MemoryTokenStore::default()),
            trust: Trust::system(),
            browser: None,
            updates: None,
        }
    }

    /// A multi-workspace data directory and a password store chosen by the
    /// caller, for workspace-management tests.
    pub fn isolated_store(
        data_dir: PathBuf,
        local: PathBuf,
        credentials: Arc<dyn Credentials>,
    ) -> Self {
        Self {
            data_dir: Some(data_dir),
            workspace: None,
            local: Some(local),
            credentials,
            tokens: Arc::new(MemoryTokenStore::default()),
            trust: Trust::system(),
            browser: None,
            updates: None,
        }
    }

    /// Checks for updates with this setup, for example a local release
    /// server in tests.
    pub fn with_updates(mut self, setup: crate::update::Setup) -> Self {
        self.updates = Some(Arc::new(setup));
        self
    }

    /// Uses these sign-in tokens, certificate authorities, and browser, for
    /// example a mock provider in tests.
    pub fn with_sign_ins(
        mut self,
        tokens: Arc<dyn TokenStore>,
        trust: Trust,
        browser: Option<Browser>,
    ) -> Self {
        self.tokens = tokens;
        self.trust = trust;
        self.browser = browser;
        self
    }

    pub fn is_demo(&self) -> bool {
        self.data_dir.is_none() && self.workspace.is_none()
    }

    pub(crate) fn data_directory(&self) -> Option<&PathBuf> {
        self.data_dir.as_ref()
    }

    pub(crate) fn workspace(&self) -> Option<&PathBuf> {
        self.workspace.as_ref()
    }

    /// The folder of the caches and transcripts, or `None` for the demo.
    pub fn local_directory_path(&self) -> Option<&std::path::Path> {
        self.local.as_deref()
    }

    /// The folder of the caches and transcripts, or `None` for the demo.
    pub(crate) fn local_directory(&self) -> Option<&PathBuf> {
        self.local.as_ref()
    }

    pub(crate) fn credentials(&self) -> Arc<dyn Credentials> {
        self.credentials.clone()
    }

    pub(crate) fn tokens(&self) -> Arc<dyn TokenStore> {
        self.tokens.clone()
    }

    pub(crate) fn trust(&self) -> Trust {
        self.trust.clone()
    }

    pub(crate) fn browser(&self) -> Option<Browser> {
        self.browser.clone()
    }

    pub(crate) fn updates(&self) -> Option<Arc<crate::update::Setup>> {
        self.updates.clone()
    }
}
