//! Portable workspace export and import (`.qrow-workspace.json`).
//!
//! Uses an explicit field whitelist — mirroring [`super::sharing`] — so secrets
//! (passwords, OIDC tokens, signed-in identities), machine-local paths (dbt
//! manifests, cache directories), app-wide settings, custom keep-alive SQL, and
//! assistant transcripts are never exported by accident. Sensitive optional
//! fields (session parameters, assistant notes) are excluded by default and
//! require explicit opt-in via [`ExportOptions`]. Every imported object
//! receives fresh UUIDs so importing never collides with existing Keychain
//! items or local caches.

use super::{
    Authentication, CatalogColumnReads, CatalogSettings, ConnectionEnvironment, ConnectionGroup,
    ConnectionLifecycle, DatabaseType, MAX_WORKSPACE_NAME, PostgresSslMode, Profile, SavedTab,
    SharedCatalog, SharedSignIn, SignIn, SignInProvider, Workspace, WorkspaceLayout,
    transfer::Transfer, validate_group_name,
};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

/// Format version of `.qrow-workspace.json` bundles.
pub const EXPORTED_WORKSPACE_VERSION: u32 = 1;
/// Largest portable workspace bundle Qrow will export or read (16 MiB).
pub const MAX_EXPORTED_WORKSPACE_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_IMPORTED_NAME: &str = "Imported workspace";

/// Explicit content selection for portable workspace export.
///
/// Safe defaults exclude saved SQL, session parameters, and assistant notes
/// because they can contain secrets or local paths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    pub include_sql: bool,
    pub include_parameters: bool,
    pub include_assistant_notes: bool,
}

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

fn sanitized_portable_lifecycle(lifecycle: &ConnectionLifecycle) -> ConnectionLifecycle {
    ConnectionLifecycle {
        idle_seconds: lifecycle.idle_seconds,
        // Never export or import background keep-alive SQL or automatic
        // keep-alive intervals; recipients must opt in explicitly.
        keep_alive_seconds: 0,
        keep_alive_sql: ConnectionLifecycle::default().keep_alive_sql,
        response_timeout_seconds: lifecycle.response_timeout_seconds,
    }
}

/// Serializes `workspace` (with display `name`) into a portable
/// `.qrow-workspace.json` document using safe default [`ExportOptions`].
pub fn export_workspace(name: &str, workspace: &Workspace) -> Result<String> {
    export_workspace_with_options(name, workspace, ExportOptions::default())
}

/// Serializes `workspace` (with display `name`) into a portable
/// `.qrow-workspace.json` document using explicit `options`.
pub fn export_workspace_with_options(
    name: &str,
    workspace: &Workspace,
    options: ExportOptions,
) -> Result<String> {
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
                parameters: if options.include_parameters {
                    profile.parameters.clone()
                } else {
                    BTreeMap::new()
                },
                tls: profile.tls,
                postgres_ssl_mode: (profile.database_type == DatabaseType::Postgres)
                    .then(|| profile.postgres_ssl_mode()),
                authentication,
                transfer: profile.transfer.clone(),
                lifecycle: sanitized_portable_lifecycle(&profile.lifecycle),
                catalog: profile.catalog.clone(),
                catalog_column_reads: profile.catalog_column_reads,
                assistant_notes: if options.include_assistant_notes {
                    profile.assistant_notes.clone()
                } else {
                    String::new()
                },
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
            sql: if options.include_sql {
                tab.sql.clone()
            } else {
                String::new()
            },
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
    let json = serde_json::to_string_pretty(&bundle).context("Could not serialize workspace.")?;
    anyhow::ensure!(
        json.len() <= MAX_EXPORTED_WORKSPACE_BYTES,
        "The exported workspace exceeds the {} MiB portable bundle limit.",
        MAX_EXPORTED_WORKSPACE_BYTES / (1024 * 1024)
    );
    Ok(json)
}

/// Parses and validates a portable `.qrow-workspace.json` document into a fresh
/// [`Workspace`] with newly minted UUIDs.
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

    let mut groups: Vec<ConnectionGroup> = Vec::with_capacity(exported.groups.len());
    let mut seen_groups: HashSet<String> = HashSet::new();
    for raw_group in exported.groups {
        let name = validate_group_name(&raw_group.name)
            .context("A connection group in the workspace file is not valid.")?;
        let key = name.to_lowercase();
        anyhow::ensure!(
            seen_groups.insert(key),
            "The workspace file contains duplicate connection group \"{name}\"."
        );
        groups.push(ConnectionGroup {
            id: Uuid::new_v4(),
            name,
            collapsed: raw_group.collapsed,
        });
    }

    let mut sign_ins: Vec<SignIn> = Vec::with_capacity(exported.sign_ins.len());
    let mut seen_sign_ins: HashSet<String> = HashSet::new();
    for raw_sign_in in exported.sign_ins {
        let sign_in = SharedSignIn::read(raw_sign_in)
            .context("A sign-in in the workspace file is not valid.")?;
        anyhow::ensure!(
            sign_in.provider == SignInProvider::Oidc,
            "Sign-ins in an exported workspace must use OpenID Connect."
        );
        sign_in
            .validate()
            .context("A sign-in in the workspace file is not valid.")?;
        let key = sign_in.name.trim().to_owned();
        anyhow::ensure!(
            seen_sign_ins.insert(key),
            "The workspace file contains duplicate sign-in \"{}\".",
            sign_in.name
        );
        sign_ins.push(sign_in);
    }

    let mut shared_catalogs: Vec<SharedCatalog> =
        Vec::with_capacity(exported.shared_catalogs.len());
    let mut seen_catalogs: HashSet<String> = HashSet::new();
    for raw_catalog in &exported.shared_catalogs {
        let catalog = SharedCatalog {
            id: Uuid::new_v4(),
            name: raw_catalog.name.trim().to_owned(),
            settings: raw_catalog.settings.clone(),
            preferred: None,
        };
        catalog
            .validate()
            .context("A shared metadata definition in the workspace file is not valid.")?;
        anyhow::ensure!(
            catalog.name != "Not shared" && catalog.name != "New shared metadata",
            "Shared metadata name \"{}\" is reserved.",
            catalog.name
        );
        let key = catalog.name.clone();
        anyhow::ensure!(
            seen_catalogs.insert(key),
            "The workspace file contains duplicate shared metadata \"{}\".",
            catalog.name
        );
        shared_catalogs.push(catalog);
    }

    let mut profiles: Vec<Profile> = Vec::with_capacity(exported.profiles.len());
    let mut seen_profiles: HashSet<String> = HashSet::new();
    for raw_profile in exported.profiles {
        let authentication = match raw_profile.authentication {
            ExportedAuthentication::Password => Authentication::Password,
            ExportedAuthentication::TrinoExternal => Authentication::TrinoExternal,
            ExportedAuthentication::Oidc { sign_in } => {
                let si = sign_ins.get(sign_in).with_context(|| {
                    format!(
                        "Connection \"{}\" references missing sign-in index {sign_in}.",
                        raw_profile.name
                    )
                })?;
                Authentication::Oidc { sign_in: si.id }
            }
        };
        let shared_catalog = match raw_profile.shared_catalog {
            Some(ix) => {
                let catalog = shared_catalogs.get(ix).with_context(|| {
                    format!(
                        "Connection \"{}\" references missing shared metadata index {ix}.",
                        raw_profile.name
                    )
                })?;
                Some(catalog.id)
            }
            None => None,
        };
        let group_id = match raw_profile.group {
            Some(ix) => {
                let group = groups.get(ix).with_context(|| {
                    format!(
                        "Connection \"{}\" references missing group index {ix}.",
                        raw_profile.name
                    )
                })?;
                Some(group.id)
            }
            None => None,
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
            // Never run bundle-supplied keep-alive SQL in the background.
            lifecycle: sanitized_portable_lifecycle(&raw_profile.lifecycle),
            catalog: raw_profile.catalog,
            catalog_column_reads: raw_profile.catalog_column_reads,
            shared_catalog,
            tls: raw_profile.tls,
            postgres_ssl_mode: raw_profile.postgres_ssl_mode,
            authentication,
            assistant_notes: raw_profile.assistant_notes.trim().to_owned(),
            dbt: None,
            group_id,
            environment: raw_profile.environment,
            ..Profile::default()
        };
        if profile.database_type == DatabaseType::Postgres {
            profile.postgres_ssl_mode = Some(profile.postgres_ssl_mode());
        }
        profile
            .validate()
            .with_context(|| format!("Connection \"{}\" is not valid.", profile.name))?;
        let key = profile.name.clone();
        anyhow::ensure!(
            seen_profiles.insert(key),
            "The workspace file contains duplicate connection \"{}\".",
            profile.name
        );
        profiles.push(profile);
    }

    for (raw_catalog, catalog) in exported
        .shared_catalogs
        .iter()
        .zip(shared_catalogs.iter_mut())
    {
        catalog.preferred = match raw_catalog.preferred_profile {
            Some(ix) => {
                let profile = profiles.get(ix).with_context(|| {
                    format!(
                        "Shared metadata \"{}\" references missing preferred connection index {ix}.",
                        catalog.name
                    )
                })?;
                anyhow::ensure!(
                    profile.shared_catalog == Some(catalog.id),
                    "Shared metadata \"{}\" names preferred connection \"{}\", which does not use that shared metadata.",
                    catalog.name,
                    profile.name
                );
                Some(profile.id)
            }
            None => None,
        };
    }

    let mut tabs: Vec<SavedTab> = Vec::with_capacity(exported.tabs.len());
    for raw_tab in exported.tabs {
        let profile = match raw_tab.profile {
            Some(ix) => {
                let p = profiles.get(ix).with_context(|| {
                    format!(
                        "Tab \"{}\" references missing connection index {ix}.",
                        raw_tab.title
                    )
                })?;
                Some(p.id)
            }
            None => None,
        };
        tabs.push(SavedTab {
            id: Uuid::new_v4(),
            title: raw_tab.title,
            sql: raw_tab.sql,
            profile,
        });
    }
    if tabs.is_empty() {
        anyhow::ensure!(
            exported.active_tab == 0,
            "Active tab index {} is out of range for an empty tab list.",
            exported.active_tab
        );
    } else {
        anyhow::ensure!(
            exported.active_tab < tabs.len(),
            "Active tab index {} is out of range for {} tabs.",
            exported.active_tab,
            tabs.len()
        );
    }

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

    fn sample_workspace() -> (Workspace, Uuid, Uuid, Uuid) {
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
            parameters: BTreeMap::from([("spark.secret.token".into(), "top-secret".into())]),
            authentication: Authentication::Oidc {
                sign_in: sign_in.id,
            },
            assistant_notes: "Internal secret note".into(),
            lifecycle: ConnectionLifecycle {
                idle_seconds: 900,
                keep_alive_seconds: 60,
                keep_alive_sql: "CALL dangerous_proc()".into(),
                response_timeout_seconds: 300,
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
        (workspace, orig_profile_id, orig_group_id, orig_sign_in_id)
    }

    #[test]
    fn export_safe_defaults_omit_secrets_parameters_notes_keep_alive_and_remaps_ids() {
        let (workspace, orig_profile_id, orig_group_id, orig_sign_in_id) = sample_workspace();

        let exported_json = export_workspace("Analytics", &workspace).unwrap();
        // Safe defaults must omit secrets, saved SQL, parameters, assistant notes, custom keep-alive SQL,
        // local paths, assistant threads, settings, and original UUIDs.
        assert!(!exported_json.contains("SELECT sum(amount)"));
        assert!(!exported_json.contains("sub-secret-user"));
        assert!(!exported_json.contains("alice@example.com"));
        assert!(!exported_json.contains("top-secret"));
        assert!(!exported_json.contains("Internal secret note"));
        assert!(!exported_json.contains("dangerous_proc"));
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
        assert!(p.parameters.is_empty());
        assert!(p.assistant_notes.is_empty());
        assert_eq!(p.lifecycle.keep_alive_seconds, 0);
        assert_eq!(p.lifecycle.keep_alive_sql, "SELECT 1");
        assert_eq!(
            p.authentication,
            Authentication::Oidc {
                sign_in: imported.workspace.sign_ins[0].id
            }
        );
        assert!(p.dbt.is_none());

        assert_eq!(imported.workspace.tabs[0].title, "Revenue");
        assert_eq!(imported.workspace.tabs[0].sql, "");
        assert_eq!(imported.workspace.tabs[0].profile, Some(p.id));
        assert_eq!(
            imported.workspace.layout.as_ref().map(|l| l.sidebar_open),
            Some(false)
        );
    }

    #[test]
    fn export_with_explicit_options_controls_sql_parameters_and_notes() {
        let (workspace, ..) = sample_workspace();
        let exported = export_workspace_with_options(
            "Analytics",
            &workspace,
            ExportOptions {
                include_sql: true,
                include_parameters: true,
                include_assistant_notes: true,
            },
        )
        .unwrap();
        assert!(exported.contains("SELECT sum(amount)"));
        assert!(exported.contains("top-secret"));
        assert!(exported.contains("Internal secret note"));
        assert!(!exported.contains("dangerous_proc"));
    }

    #[test]
    fn import_disables_crafted_keep_alive_sql_and_rejects_bad_references_or_profiles() {
        // Crafted bundle with background keep-alive SQL must be neutralized.
        let crafted_keep_alive = r#"{
            "qrow_workspace": 1,
            "name": "Crafted",
            "profiles": [{
                "name": "P1",
                "host": "db.internal",
                "username": "alice",
                "database": "main",
                "lifecycle": {
                    "idle_seconds": 900,
                    "keep_alive_seconds": 30,
                    "keep_alive_sql": "DROP TABLE audit_log",
                    "response_timeout_seconds": 300
                }
            }]
        }"#;
        let imported = import_workspace(crafted_keep_alive).unwrap();
        assert_eq!(
            imported.workspace.profiles[0].lifecycle.keep_alive_seconds,
            0
        );
        assert_eq!(
            imported.workspace.profiles[0].lifecycle.keep_alive_sql,
            "SELECT 1"
        );

        // Invalid OIDC sign-in index must be rejected (never silently downgraded to Password).
        let bad_oidc = r#"{
            "qrow_workspace": 1,
            "profiles": [{
                "name": "P1",
                "host": "db.internal",
                "username": "alice",
                "database": "main",
                "authentication": { "method": "oidc", "sign_in": 4 }
            }]
        }"#;
        assert!(import_workspace(bad_oidc).is_err());

        // Invalid tab profile index must be rejected (never redirected to another connection).
        let bad_tab = r#"{
            "qrow_workspace": 1,
            "profiles": [{
                "name": "P1",
                "host": "db.internal",
                "username": "alice",
                "database": "main"
            }],
            "tabs": [{ "title": "Q1", "sql": "SELECT 1", "profile": 9 }]
        }"#;
        assert!(import_workspace(bad_tab).is_err());

        // Invalid profile (missing host) must fail Profile::validate.
        let bad_profile = r#"{
            "qrow_workspace": 1,
            "profiles": [{
                "name": "P1",
                "host": "",
                "username": "alice",
                "database": "main"
            }]
        }"#;
        assert!(import_workspace(bad_profile).is_err());
    }

    #[test]
    fn import_rejects_newer_versions_and_non_workspace_json() {
        let err = import_workspace(r#"{"qrow_workspace": 99}"#).unwrap_err();
        assert!(err.to_string().contains("newer version"));

        let err = import_workspace(r#"{"version": 9}"#).unwrap_err();
        assert!(err.to_string().contains(".qrow-workspace.json"));
    }
}
