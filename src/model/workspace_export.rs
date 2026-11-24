//! Portable workspace export and import (`.qrow-workspace.json`).
//!
//! Uses an explicit field whitelist — mirroring [`super::sharing`] — so secrets
//! (passwords, OIDC tokens, signed-in identities), machine-local paths (dbt
//! manifests, cache directories), app-wide settings, and assistant transcripts
//! are never exported by accident. Every imported object receives fresh UUIDs
//! so importing never collides with existing Keychain items or local caches.

use super::{
    Authentication, CatalogColumnReads, CatalogSettings, ConnectionEnvironment, ConnectionGroup,
    ConnectionLifecycle, DatabaseType, MAX_WORKSPACE_NAME, PostgresSslMode, Profile, SavedTab,
    SharedCatalog, SharedSignIn, SignIn, SignInProvider, Workspace, WorkspaceLayout,
    transfer::Transfer,
};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

/// Format version of `.qrow-workspace.json` bundles.
pub const EXPORTED_WORKSPACE_VERSION: u32 = 1;
/// Largest portable workspace bundle Qrow will read (16 MiB).
const MAX_EXPORTED_WORKSPACE_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_IMPORTED_NAME: &str = "Imported workspace";

/// Result of parsing a portable `.qrow-workspace.json` bundle.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedWorkspace {
    pub name: String,
    pub workspace: Workspace,
}

#[derive(Deserialize, Serialize)]
struct ExportedWorkspace<S = SharedSignIn> {
    qrow_workspace: u32,
    #[serde(default)]
    name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    groups: Vec<ExportedGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sign_ins: Vec<S>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    shared_catalogs: Vec<ExportedSharedCatalog>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    profiles: Vec<ExportedProfile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tabs: Vec<ExportedTab>,
    #[serde(default)]
    active_tab: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    layout: Option<WorkspaceLayout>,
}

#[derive(Deserialize, Serialize)]
struct ExportedGroup {
    name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    collapsed: bool,
}

#[derive(Deserialize, Serialize)]
struct ExportedSharedCatalog {
    name: String,
    #[serde(default)]
    settings: CatalogSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preferred_profile: Option<usize>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(tag = "method", rename_all = "snake_case")]
enum ExportedAuthentication {
    #[default]
    Password,
    Oidc {
        sign_in: usize,
    },
    TrinoExternal,
}

impl ExportedAuthentication {
    fn is_password(&self) -> bool {
        matches!(self, Self::Password)
    }
}

#[derive(Deserialize, Serialize)]
struct ExportedProfile {
    #[serde(default)]
    name: String,
    #[serde(default)]
    database_type: DatabaseType,
    #[serde(default)]
    host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(default)]
    username: String,
    #[serde(default)]
    database: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    trino_schema: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    parameters: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "is_default")]
    tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    postgres_ssl_mode: Option<PostgresSslMode>,
    #[serde(default, skip_serializing_if = "ExportedAuthentication::is_password")]
    authentication: ExportedAuthentication,
    #[serde(default, skip_serializing_if = "is_default")]
    transfer: Transfer,
    #[serde(default, skip_serializing_if = "is_default")]
    lifecycle: ConnectionLifecycle,
    #[serde(default, skip_serializing_if = "is_default")]
    catalog: CatalogSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    catalog_column_reads: CatalogColumnReads,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    assistant_notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    environment: Option<ConnectionEnvironment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared_catalog: Option<usize>,
}

#[derive(Deserialize, Serialize)]
struct ExportedTab {
    #[serde(default)]
    title: String,
    #[serde(default)]
    sql: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<usize>,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Serializes `workspace` (with display `name`) into a portable
/// `.qrow-workspace.json` document containing no credentials, local paths,
/// app-wide settings, or assistant transcripts.
pub fn export_workspace(name: &str, workspace: &Workspace) -> String {
    let group_index: HashMap<Uuid, usize> = workspace
        .groups
        .iter()
        .enumerate()
        .map(|(ix, group)| (group.id, ix))
        .collect();
    let sign_in_index: HashMap<Uuid, usize> = workspace
        .sign_ins
        .iter()
        .enumerate()
        .map(|(ix, sign_in)| (sign_in.id, ix))
        .collect();
    let catalog_index: HashMap<Uuid, usize> = workspace
        .shared_catalogs
        .iter()
        .enumerate()
        .map(|(ix, catalog)| (catalog.id, ix))
        .collect();
    let profile_index: HashMap<Uuid, usize> = workspace
        .profiles
        .iter()
        .enumerate()
        .map(|(ix, profile)| (profile.id, ix))
        .collect();

    let groups = workspace
        .groups
        .iter()
        .map(|group| ExportedGroup {
            name: group.name.clone(),
            collapsed: group.collapsed,
        })
        .collect();
    let sign_ins = workspace.sign_ins.iter().map(SharedSignIn::of).collect();
    let shared_catalogs = workspace
        .shared_catalogs
        .iter()
        .map(|catalog| ExportedSharedCatalog {
            name: catalog.name.clone(),
            settings: catalog.settings.clone(),
            preferred_profile: catalog
                .preferred
                .and_then(|id| profile_index.get(&id).copied()),
        })
        .collect();
    let profiles = workspace
        .profiles
        .iter()
        .map(|profile| {
            let authentication = match profile.authentication {
                Authentication::Oidc { sign_in } => match sign_in_index.get(&sign_in).copied() {
                    Some(index) => ExportedAuthentication::Oidc { sign_in: index },
                    None => ExportedAuthentication::Password,
                },
                Authentication::TrinoExternal => ExportedAuthentication::TrinoExternal,
                Authentication::Password => ExportedAuthentication::Password,
            };
            ExportedProfile {
                name: profile.name.clone(),
                database_type: profile.database_type,
                host: profile.host.clone(),
                port: Some(profile.port),
                username: profile.username.clone(),
                database: profile.database.clone(),
                trino_schema: profile.trino_schema.clone(),
                parameters: profile.parameters.clone(),
                tls: profile.tls,
                postgres_ssl_mode: (profile.database_type == DatabaseType::Postgres)
                    .then(|| profile.postgres_ssl_mode()),
                authentication,
                transfer: profile.transfer.clone(),
                lifecycle: profile.lifecycle.clone(),
                catalog: profile.catalog.clone(),
                catalog_column_reads: profile.catalog_column_reads,
                assistant_notes: profile.assistant_notes.clone(),
                environment: profile.environment,
                group: profile
                    .group_id
                    .and_then(|id| group_index.get(&id).copied()),
                shared_catalog: profile
                    .shared_catalog
                    .and_then(|id| catalog_index.get(&id).copied()),
            }
        })
        .collect();
    let tabs = workspace
        .tabs
        .iter()
        .map(|tab| ExportedTab {
            title: tab.title.clone(),
            sql: tab.sql.clone(),
            profile: tab.profile.and_then(|id| profile_index.get(&id).copied()),
        })
        .collect();

    let bundle = ExportedWorkspace {
        qrow_workspace: EXPORTED_WORKSPACE_VERSION,
        name: name.trim().to_owned(),
        groups,
        sign_ins,
        shared_catalogs,
        profiles,
        tabs,
        active_tab: workspace.active_tab,
        layout: workspace.layout.clone(),
    };
    serde_json::to_string_pretty(&bundle).expect("exported workspace serializes")
}

/// Parses a portable `.qrow-workspace.json` document into a fresh [`Workspace`]
/// with newly minted UUIDs.
pub fn import_workspace(text: &str) -> Result<ImportedWorkspace> {
    anyhow::ensure!(
        text.len() <= MAX_EXPORTED_WORKSPACE_BYTES,
        "The workspace file is too large to import."
    );
    let value: serde_json::Value =
        serde_json::from_str(text).context("The selected file is not valid JSON.")?;
    let version = value
        .get("qrow_workspace")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "The selected file is not an exported workspace (.qrow-workspace.json)."
            )
        })?;
    anyhow::ensure!(
        version >= 1 && version <= u64::from(EXPORTED_WORKSPACE_VERSION),
        "This workspace file comes from a newer version of Qrow. Update Qrow, then try again."
    );
    let exported: ExportedWorkspace<serde_json::Value> = serde_json::from_value(value)
        .map_err(|error| anyhow::Error::new(error).context("The workspace file is not valid."))?;

    let groups: Vec<ConnectionGroup> = exported
        .groups
        .into_iter()
        .map(|group| ConnectionGroup {
            id: Uuid::new_v4(),
            name: group.name,
            collapsed: group.collapsed,
        })
        .collect();

    let mut sign_ins: Vec<SignIn> = Vec::with_capacity(exported.sign_ins.len());
    for raw_sign_in in exported.sign_ins {
        let sign_in = SharedSignIn::read(raw_sign_in)
            .context("A sign-in in the workspace file is not valid.")?;
        anyhow::ensure!(
            sign_in.provider == SignInProvider::Oidc,
            "Sign-ins in an exported workspace must use OpenID Connect."
        );
        sign_ins.push(sign_in);
    }

    let mut shared_catalogs: Vec<SharedCatalog> = exported
        .shared_catalogs
        .iter()
        .map(|catalog| SharedCatalog {
            id: Uuid::new_v4(),
            name: catalog.name.clone(),
            settings: catalog.settings.clone(),
            preferred: None,
        })
        .collect();

    let mut profiles: Vec<Profile> = Vec::with_capacity(exported.profiles.len());
    for raw_profile in exported.profiles {
        let authentication = match raw_profile.authentication {
            ExportedAuthentication::Password => Authentication::Password,
            ExportedAuthentication::TrinoExternal => Authentication::TrinoExternal,
            ExportedAuthentication::Oidc { sign_in } => match sign_ins.get(sign_in) {
                Some(si) => Authentication::Oidc { sign_in: si.id },
                None => Authentication::Password,
            },
        };
        let mut profile = Profile {
            database_type: raw_profile.database_type,
            name: raw_profile.name.trim().to_owned(),
            host: raw_profile.host.trim().to_owned(),
            port: raw_profile
                .port
                .unwrap_or_else(|| raw_profile.database_type.default_port()),
            username: raw_profile.username.trim().to_owned(),
            database: raw_profile.database.trim().to_owned(),
            trino_schema: raw_profile.trino_schema.trim().to_owned(),
            parameters: raw_profile.parameters,
            transfer: raw_profile.transfer,
            lifecycle: raw_profile.lifecycle,
            catalog: raw_profile.catalog,
            catalog_column_reads: raw_profile.catalog_column_reads,
            shared_catalog: raw_profile
                .shared_catalog
                .and_then(|ix| shared_catalogs.get(ix).map(|c| c.id)),
            tls: raw_profile.tls,
            postgres_ssl_mode: raw_profile.postgres_ssl_mode,
            authentication,
            assistant_notes: raw_profile.assistant_notes.trim().to_owned(),
            dbt: None,
            group_id: raw_profile
                .group
                .and_then(|ix| groups.get(ix).map(|g| g.id)),
            environment: raw_profile.environment,
            ..Profile::default()
        };
        if profile.database_type == DatabaseType::Postgres {
            profile.postgres_ssl_mode = Some(profile.postgres_ssl_mode());
        }
        profiles.push(profile);
    }

    for (raw_catalog, catalog) in exported
        .shared_catalogs
        .iter()
        .zip(shared_catalogs.iter_mut())
    {
        catalog.preferred = raw_catalog
            .preferred_profile
            .and_then(|ix| profiles.get(ix).map(|p| p.id));
    }

    let tabs: Vec<SavedTab> = exported
        .tabs
        .into_iter()
        .map(|tab| SavedTab {
            id: Uuid::new_v4(),
            title: tab.title,
            sql: tab.sql,
            profile: tab.profile.and_then(|ix| profiles.get(ix).map(|p| p.id)),
        })
        .collect();

    let mut workspace = Workspace {
        groups,
        profiles,
        tabs: if tabs.is_empty() {
            Workspace::default().tabs
        } else {
            tabs
        },
        active_tab: exported.active_tab,
        shared_catalogs,
        sign_ins,
        layout: exported.layout,
        ..Workspace::default()
    };
    workspace.normalize();

    let trimmed = exported.name.trim();
    let name: String = if trimmed.is_empty() {
        DEFAULT_IMPORTED_NAME.to_owned()
    } else {
        trimmed.chars().take(MAX_WORKSPACE_NAME).collect()
    };
    Ok(ImportedWorkspace { name, workspace })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AssistantConversation, AssistantExecutionMode, DbtProject, Identity};

    #[test]
    fn export_omits_secrets_local_paths_settings_and_assistant_and_remaps_ids() {
        let group = ConnectionGroup::new("Production");
        let sign_in = SignIn {
            name: "Corp SSO".into(),
            issuer: "https://auth.example.com/realms/corp".into(),
            client_id: "qrow-desktop".into(),
            identity: Some(Identity {
                issuer: "https://auth.example.com/realms/corp".into(),
                subject: "sub-secret-user".into(),
                name: Some("Alice".into()),
                email: Some("alice@example.com".into()),
            }),
            ..SignIn::default()
        };
        let mut profile = Profile {
            name: "Prod Spark".into(),
            host: "spark.internal".into(),
            username: "analyst".into(),
            database: "warehouse".into(),
            authentication: Authentication::Oidc {
                sign_in: sign_in.id,
            },
            group_id: Some(group.id),
            environment: Some(ConnectionEnvironment::Production),
            dbt: Some(DbtProject {
                manifest: "/Users/secret/repo/target/manifest.json".into(),
                refresh: Default::default(),
                schema_mapping: vec![],
            }),
            ..Profile::default()
        };
        profile.catalog.refresh = crate::model::CatalogRefresh::Manual;
        let orig_profile_id = profile.id;
        let orig_group_id = group.id;
        let orig_sign_in_id = sign_in.id;

        let mut tab = SavedTab::new(1, Some(profile.id));
        tab.title = "Revenue".into();
        tab.sql = "SELECT sum(amount) FROM sales".into();

        let mut workspace = Workspace {
            groups: vec![group],
            profiles: vec![profile],
            tabs: vec![tab],
            sign_ins: vec![sign_in],
            layout: Some(WorkspaceLayout {
                sidebar_open: false,
                sidebar_width: 280.,
                ..WorkspaceLayout::default()
            }),
            ..Workspace::default()
        };
        workspace
            .assistant
            .conversations
            .push(AssistantConversation::new(
                "thread-private",
                AssistantExecutionMode::AskBeforeRunning,
            ));

        let exported_json = export_workspace("Analytics", &workspace);
        // Must not contain any secrets, local paths, assistant threads, or settings.
        assert!(!exported_json.contains("sub-secret-user"));
        assert!(!exported_json.contains("alice@example.com"));
        assert!(!exported_json.contains("/Users/secret"));
        assert!(!exported_json.contains("thread-private"));
        assert!(!exported_json.contains("\"settings\""));
        assert!(!exported_json.contains(&orig_profile_id.to_string()));
        assert!(!exported_json.contains(&orig_group_id.to_string()));
        assert!(!exported_json.contains(&orig_sign_in_id.to_string()));

        let imported = import_workspace(&exported_json).unwrap();
        assert_eq!(imported.name, "Analytics");
        assert_eq!(imported.workspace.groups.len(), 1);
        assert_eq!(imported.workspace.groups[0].name, "Production");
        assert_ne!(imported.workspace.groups[0].id, orig_group_id);

        assert_eq!(imported.workspace.sign_ins.len(), 1);
        assert_ne!(imported.workspace.sign_ins[0].id, orig_sign_in_id);
        assert!(imported.workspace.sign_ins[0].identity.is_none());

        assert_eq!(imported.workspace.profiles.len(), 1);
        let p = &imported.workspace.profiles[0];
        assert_ne!(p.id, orig_profile_id);
        assert_eq!(p.group_id, Some(imported.workspace.groups[0].id));
        assert_eq!(p.environment, Some(ConnectionEnvironment::Production));
        assert_eq!(
            p.authentication,
            Authentication::Oidc {
                sign_in: imported.workspace.sign_ins[0].id
            }
        );
        assert!(p.dbt.is_none());

        assert_eq!(imported.workspace.tabs[0].title, "Revenue");
        assert_eq!(
            imported.workspace.tabs[0].sql,
            "SELECT sum(amount) FROM sales"
        );
        assert_eq!(imported.workspace.tabs[0].profile, Some(p.id));
        assert_eq!(
            imported.workspace.layout.as_ref().map(|l| l.sidebar_open),
            Some(false)
        );
    }

    #[test]
    fn import_rejects_newer_versions_and_non_workspace_json() {
        let err = import_workspace(r#"{"qrow_workspace": 99}"#).unwrap_err();
        assert!(err.to_string().contains("newer version"));

        let err = import_workspace(r#"{"version": 9}"#).unwrap_err();
        assert!(err.to_string().contains(".qrow-workspace.json"));
    }
}
