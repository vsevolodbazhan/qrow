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
    credentials: Arc<dyn Credentials>,
    tokens: Arc<dyn TokenStore>,
    trust: Trust,
    /// `None` opens the default browser of the system.
    browser: Option<Browser>,
}

impl Environment {
    /// The user's workspace, the macOS Keychain, and the system trust store.
    pub fn user() -> Self {
        Self {
            workspace: Some(storage::workspace_path()),
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
            credentials: Arc::new(Keychain),
            tokens: Arc::new(MemoryTokenStore::default()),
            trust: Trust::system(),
            browser: None,
        }
    }

    /// A workspace file and a password store chosen by the caller, for
    /// example a temporary directory and synthetic passwords in tests.
    /// Sign-in tokens stay in memory.
    pub fn isolated(workspace: PathBuf, credentials: Arc<dyn Credentials>) -> Self {
        Self {
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
