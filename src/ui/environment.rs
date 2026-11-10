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
    /// `None` shows the built-in demo data and saves nothing.
    workspace: Option<PathBuf>,
    /// The folder of the caches and transcripts of the workspace. See
    /// [`storage::local_data_directory`].
    local: Option<PathBuf>,
    credentials: Arc<dyn Credentials>,
    tokens: Arc<dyn TokenStore>,
    trust: Trust,
    /// `None` opens the default browser of the system.
    browser: Option<Browser>,
}

impl Environment {
    /// The user's workspace, system secret store, and system trust store.
    pub fn user() -> Self {
        Self {
            workspace: Some(storage::workspace_path()),
            local: Some(storage::local_data_directory()),
            credentials: Arc::new(Keychain),
            tokens: Arc::new(Keychain),
            trust: Trust::system(),
            browser: None,
        }
    }

    /// Built-in demo data. The demo does not read, save, or delete passwords
    /// or tokens.
    pub fn demo() -> Self {
        Self {
            workspace: None,
            local: None,
            credentials: Arc::new(Keychain),
            tokens: Arc::new(MemoryTokenStore::default()),
            trust: Trust::system(),
            browser: None,
        }
    }

    /// A workspace file and a password store chosen by the caller, for
    /// example a temporary directory and synthetic passwords in tests.
    /// Caches and transcripts are next to the workspace file. Sign-in tokens
    /// stay in memory.
    pub fn isolated(workspace: PathBuf, credentials: Arc<dyn Credentials>) -> Self {
        Self {
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
        }
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
        self.workspace.is_none()
    }

    pub(crate) fn workspace(&self) -> Option<&PathBuf> {
        self.workspace.as_ref()
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
}
