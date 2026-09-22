//! Named workspaces and the last selected workspace within one data directory.
use crate::{
    model::Workspace,
    storage::{Saver, WorkspaceFile},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Entry {
    pub id: Uuid,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Deletion {
    id: Uuid,
    profiles: Vec<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    version: u32,
    active: Option<Uuid>,
    entries: Vec<Entry>,
    #[serde(default)]
    pending_deletions: Vec<Deletion>,
    #[serde(default)]
    recent: Vec<Uuid>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: 2,
            active: None,
            entries: vec![],
            pending_deletions: vec![],
            recent: vec![],
        }
    }
}
impl Catalog {
    pub fn load(root: &Path) -> Result<Self> {
        let mut catalog: Self = match fs::read(root.join("workspaces.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Could not read workspace list")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut catalog = Self::default();
                if root.join("workspace.json").exists() {
                    catalog.entries.push(Entry {
                        id: Uuid::nil(),
                        name: "Default".into(),
                    });
                    catalog.opened(Uuid::nil());
                }
                catalog
            }
            Err(e) => return Err(e).context("Could not read workspace list"),
        };
        ensure!(
            (1..=2).contains(&catalog.version),
            "Unsupported workspace list version"
        );
        // Older catalogs know only the last selection. Use reverse creation order
        // for their remaining entries until those workspaces have been opened.
        if catalog.version == 1 {
            catalog.recent = catalog
                .active
                .into_iter()
                .chain(
                    catalog
                        .entries
                        .iter()
                        .rev()
                        .map(|e| e.id)
                        .filter(|id| Some(*id) != catalog.active),
                )
                .collect();
        }
        catalog.version = 2;
        ensure!(
            match catalog.active {
                Some(id) => catalog.entries.iter().any(|e| e.id == id),
                None => catalog.entries.is_empty(),
            },
            "Last opened workspace is missing"
        );
        for (ix, entry) in catalog.entries.iter().enumerate() {
            validate_name(&entry.name)?;
            ensure!(!catalog.entries[..ix].iter().any(|e| e.id == entry.id || e.name.to_lowercase() == entry.name.to_lowercase()), "Duplicate workspace in list");
        }
        for (ix, deletion) in catalog.pending_deletions.iter().enumerate() {
            ensure!(
                !catalog.entries.iter().any(|e| e.id == deletion.id)
                    && !catalog.pending_deletions[..ix]
                        .iter()
                        .any(|e| e.id == deletion.id),
                "Invalid pending workspace deletion"
            );
        }
        ensure!(
            catalog.recent.len() == catalog.entries.len()
                && catalog.recent.first().copied() == catalog.active
                && catalog.recent.iter().enumerate().all(|(ix, id)| catalog
                    .entries
                    .iter()
                    .any(|e| e.id == *id)
                    && !catalog.recent[..ix].contains(id)),
            "Invalid workspace history"
        );
        Ok(catalog)
    }
    fn opened(&mut self, id: Uuid) {
        self.active = Some(id);
        self.recent.retain(|previous| *previous != id);
        self.recent.insert(0, id);
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn active(&self) -> Option<&Entry> {
        self.entries.iter().find(|e| Some(e.id) == self.active)
    }
    pub fn path(&self, root: &Path, id: Uuid) -> Result<PathBuf> {
        ensure!(
            self.entries.iter().any(|e| e.id == id),
            "Workspace no longer exists"
        );
        Ok(workspace_file_path(root, id))
    }
}

fn workspace_file_path(root: &Path, id: Uuid) -> PathBuf {
    if id.is_nil() {
        root.join("workspace.json")
    } else {
        root.join("workspaces")
            .join(id.to_string())
            .join("workspace.json")
    }
}

fn validate_name(name: &str) -> Result<()> {
    ensure!(!name.trim().is_empty(), "Enter a workspace name.");
    ensure!(
        name.chars().count() <= 80,
        "Workspace name must be 80 characters or fewer."
    );
    ensure!(
        !name.chars().any(char::is_control),
        "Workspace name cannot contain control characters."
    );
    Ok(())
}

/// Open a target before publishing the selection. Call only after saving the source.
/// The catalog lock prevents competing creators from losing each other's entries.
pub fn open(
    root: &Path,
    target: Option<Uuid>,
    name: Option<&str>,
    wake: impl Fn() + Send + 'static,
) -> Result<(Catalog, Workspace, Saver)> {
    let file = WorkspaceFile::acquire(root.join("workspaces.json"))?;
    let mut catalog = Catalog::load(root)?;
    let id = if let Some(name) = name {
        let name = name.trim();
        validate_name(name)?;
        ensure!(
            !catalog
                .entries
                .iter()
                .any(|e| e.name.to_lowercase() == name.to_lowercase()),
            "A workspace with this name already exists."
        );
        let id = Uuid::new_v4();
        catalog.entries.push(Entry {
            id,
            name: name.into(),
        });
        id
    } else {
        target
            .or(catalog.active)
            .context("Create a workspace to begin.")?
    };
    let path = catalog.path(root, id)?;
    // A listed workspace must never silently become a blank workspace after deletion.
    if name.is_none() {
        ensure!(
            path.is_file(),
            "Workspace file is missing. Restore it before opening this workspace."
        );
    }
    let (workspace, saver) = Saver::open(path, wake)?;
    if name.is_some() {
        saver
            .flush(workspace.clone())?
            .recv()
            .context("Workspace saver stopped")?
            .map_err(anyhow::Error::msg)?;
    }
    catalog.opened(id);
    file.save_json(&catalog)?;
    Ok((catalog, workspace, saver))
}

/// Restore the last opened workspace, or return no workspace on a fresh install.
pub fn restore(
    root: &Path,
    wake: impl Fn() + Send + 'static,
) -> Result<Option<(Catalog, Workspace, Saver)>> {
    if Catalog::load(root)?.active().is_none() {
        return Ok(None);
    }
    open(root, None, None, wake).map(Some)
}

/// Change a display name without changing workspace files or the last selection.
pub fn rename(root: &Path, id: Uuid, name: &str) -> Result<Catalog> {
    let file = WorkspaceFile::acquire(root.join("workspaces.json"))?;
    let mut catalog = Catalog::load(root)?;
    let name = name.trim();
    validate_name(name)?;
    ensure!(
        !catalog
            .entries
            .iter()
            .any(|e| e.id != id && e.name.to_lowercase() == name.to_lowercase()),
        "A workspace with this name already exists."
    );
    let entry = catalog
        .entries
        .iter_mut()
        .find(|e| e.id == id)
        .context("Workspace no longer exists")?;
    entry.name = name.into();
    file.save_json(&catalog)?;
    Ok(catalog)
}

/// Result of a committed deletion. Cleanup failures do not reopen the deleted workspace.
#[non_exhaustive]
pub struct DeletedWorkspace {
    pub catalog: Catalog,
    pub next: Option<(Workspace, Saver)>,
    pub warning: Option<String>,
}

/// Permanently delete the current workspace after the caller has flushed its saver.
/// Keep the source lock, issue no more saves, and stop its saver after success.
/// A durable deletion record lets startup finish interrupted file/password removal.
pub fn delete(
    root: &Path,
    id: Uuid,
    mut delete_password: impl FnMut(Uuid) -> Result<()>,
    wake: impl Fn() + Send + 'static,
) -> Result<DeletedWorkspace> {
    let file = WorkspaceFile::acquire(root.join("workspaces.json"))?;
    let mut catalog = Catalog::load(root)?;
    ensure!(
        catalog.active == Some(id),
        "The workspace selection changed. Reopen the workspace before deleting it."
    );
    let path = catalog.path(root, id)?;
    ensure!(path.is_file(), "Workspace file is missing.");
    let source = crate::storage::load(&path)?;
    let mut profiles: Vec<_> = source.profiles.iter().map(|profile| profile.id).collect();
    // Imported or manually copied files can refer to the same Keychain entry.
    for entry in catalog.entries.iter().filter(|entry| entry.id != id) {
        let path = catalog.path(root, entry.id)?;
        ensure!(
            path.is_file(),
            "Another workspace file is missing. Recover it before deleting this workspace."
        );
        let workspace = crate::storage::load(&path)?;
        profiles.retain(|id| !workspace.profiles.iter().any(|profile| profile.id == *id));
    }
    catalog.entries.retain(|entry| entry.id != id);
    catalog.recent.retain(|entry| *entry != id);
    catalog.active = catalog.recent.first().copied();
    let next = if let Some(next) = catalog.active {
        Some(
            Saver::open(catalog.path(root, next)?, wake)
                .context("Could not open the next workspace")?,
        )
    } else {
        None
    };
    let deletion = Deletion { id, profiles };
    catalog.pending_deletions.push(deletion.clone());
    file.save_json(&catalog)?;
    let warning = match remove_deleted(root, &deletion, &mut delete_password) {
        Ok(()) => {
            catalog
                .pending_deletions
                .retain(|deletion| deletion.id != id);
            file.save_json(&catalog).err().map(|error| format!("Workspace deleted, but cleanup confirmation could not be saved: {error:#}. Qrow will retry at startup."))
        }
        Err(error) => Some(format!(
            "Workspace deleted, but cleanup is incomplete: {error:#}. Qrow will retry at startup."
        )),
    };
    Ok(DeletedWorkspace {
        catalog,
        next,
        warning,
    })
}

fn remove_deleted(
    root: &Path,
    deletion: &Deletion,
    delete_password: &mut impl FnMut(Uuid) -> Result<()>,
) -> Result<()> {
    match fs::remove_file(workspace_file_path(root, deletion.id)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Could not remove the workspace file"),
    }
    fs::File::open(
        workspace_file_path(root, deletion.id)
            .parent()
            .expect("workspace directory"),
    )?
    .sync_all()?;
    for id in &deletion.profiles {
        delete_password(*id).context("Could not remove a saved password")?;
    }
    Ok(())
}

/// Finish confirmed deletions after an interrupted run. Missing passwords are success.
pub fn finish_deletions(
    root: &Path,
    mut delete_password: impl FnMut(Uuid) -> Result<()>,
) -> Result<()> {
    if Catalog::load(root)?.pending_deletions.is_empty() {
        return Ok(());
    }
    let file = WorkspaceFile::acquire(root.join("workspaces.json"))?;
    let mut catalog = Catalog::load(root)?;
    while let Some(deletion) = catalog.pending_deletions.first() {
        let _source = WorkspaceFile::acquire(workspace_file_path(root, deletion.id))?;
        remove_deleted(root, deletion, &mut delete_password)?;
        catalog.pending_deletions.remove(0);
        file.save_json(&catalog)?;
    }
    Ok(())
}
