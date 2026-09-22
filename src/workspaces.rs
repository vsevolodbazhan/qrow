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
pub struct Catalog {
    version: u32,
    active: Option<Uuid>,
    entries: Vec<Entry>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: 1,
            active: None,
            entries: vec![],
        }
    }
}
impl Catalog {
    pub fn load(root: &Path) -> Result<Self> {
        let catalog: Self = match fs::read(root.join("workspaces.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Could not read workspace list")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut catalog = Self::default();
                if root.join("workspace.json").exists() {
                    catalog.active = Some(Uuid::nil());
                    catalog.entries.push(Entry {
                        id: Uuid::nil(),
                        name: "Default".into(),
                    });
                }
                catalog
            }
            Err(e) => return Err(e).context("Could not read workspace list"),
        };
        ensure!(catalog.version == 1, "Unsupported workspace list version");
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
        Ok(catalog)
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
        Ok(if id.is_nil() {
            root.join("workspace.json")
        } else {
            root.join("workspaces")
                .join(id.to_string())
                .join("workspace.json")
        })
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
    catalog.active = Some(id);
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
