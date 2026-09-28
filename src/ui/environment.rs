use crate::storage::{self, Credentials, Keychain};
use std::{path::PathBuf, sync::Arc};

/// Where a Qrow window keeps its workspace and connection passwords.
#[derive(Clone)]
pub struct Environment {
    /// `None` shows the built-in demo data and saves nothing.
    workspace: Option<PathBuf>,
    credentials: Arc<dyn Credentials>,
}

impl Environment {
    /// The user's workspace and the macOS Keychain.
    pub fn user() -> Self {
        Self {
            workspace: Some(storage::workspace_path()),
            credentials: Arc::new(Keychain),
        }
    }

    /// Built-in demo data. The demo does not read, save, or delete passwords.
    pub fn demo() -> Self {
        Self {
            workspace: None,
            credentials: Arc::new(Keychain),
        }
    }

    /// A workspace file and a password store chosen by the caller, for
    /// example a temporary directory and synthetic passwords in tests.
    pub fn isolated(workspace: PathBuf, credentials: Arc<dyn Credentials>) -> Self {
        Self {
            workspace: Some(workspace),
            credentials,
        }
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
}
