//! Sequential JSON-AST schema migrations and version compatibility checks.
//!
//! Versioned files (`workspace.json`, `settings.json`, `workspaces.json`) share
//! one migration pipeline:
//!
//! 1. Parse raw JSON once into [`serde_json::Value`].
//! 2. Inspect `version` and optional `min_reader_version`.
//! 3. If the file is older than the current schema, run sequential JSON-level
//!    migrations (`from -> to`) before typed deserialization.
//! 4. If the file is newer than the current schema, allow reading when
//!    `min_reader_version <= current` and preserve the higher version metadata
//!    on save so newer builds do not re-run migrations.
//! 5. Deserialize the migrated value through `Unknown::read_value`
//!    so genuinely unknown fields from newer builds survive round-trips.

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Top-level metadata key naming the Qrow release version that last saved the
/// file. It belongs to the file envelope, not to the typed model.
pub const WRITTEN_BY: &str = "written_by";

/// Top-level metadata key declaring the oldest schema version that can safely
/// read this file (with unknown-field preservation). When absent, it defaults
/// to `version`.
pub const MIN_READER_VERSION: &str = "min_reader_version";

/// Top-level metadata keys excluded from unknown-field tracking.
pub const ENVELOPE_KEYS: [&str; 2] = [WRITTEN_BY, MIN_READER_VERSION];

/// One sequential schema migration operating directly on the JSON document.
#[derive(Clone, Copy)]
pub struct Migration {
    pub from: u32,
    pub to: u32,
    pub apply: fn(&mut Map<String, Value>) -> Result<()>,
}

/// Version rules and migration steps for one versioned JSON document type.
#[derive(Clone, Copy)]
pub struct VersionSpec {
    /// Human-readable file kind for error messages, e.g. `"workspace"`.
    pub label: &'static str,
    /// Current schema version written by this build.
    pub current: u32,
    /// Smallest legacy version accepted directly without JSON migrations.
    pub min_legacy: u32,
    /// Largest legacy version handled by existing deserialization/normalization
    /// before the sequential JSON migration chain begins.
    pub legacy_ceiling: u32,
    /// Oldest reader version compatible with files written by this build.
    pub min_reader: u32,
    /// Ordered sequential migrations starting at `legacy_ceiling`.
    pub migrations: &'static [Migration],
}

/// Version metadata preserved when opening a forward-compatible newer file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreservedVersion {
    pub version: u32,
    pub min_reader_version: u32,
}

/// Outcome of inspecting and migrating a raw JSON document.
#[derive(Debug)]
pub struct MigratedDocument {
    pub value: Value,
    /// The Qrow release version recorded in `written_by`, if present.
    pub written_by: Option<String>,
    /// The original schema version on disk.
    pub original_version: u32,
    /// Set when the document was upgraded from an older schema version.
    pub migrated_from: Option<u32>,
    /// Set when the document came from a forward-compatible newer schema.
    pub preserved_version: Option<PreservedVersion>,
}

/// Parses `data` and migrates it according to `spec` without mutating disk.
pub fn migrate_bytes(data: &[u8], spec: &VersionSpec) -> Result<MigratedDocument> {
    let value: Value = serde_json::from_slice(data).with_context(|| {
        format!(
            "Saved {} is invalid; the original file has been left untouched",
            spec.label
        )
    })?;
    migrate_value(value, spec)
}

/// Inspects and migrates an already-parsed JSON document according to `spec`.
pub fn migrate_value(mut value: Value, spec: &VersionSpec) -> Result<MigratedDocument> {
    let object = value.as_object_mut().with_context(|| {
        format!(
            "Saved {} is invalid; the original file has been left untouched",
            spec.label
        )
    })?;
    let written_by = object
        .get(WRITTEN_BY)
        .and_then(Value::as_str)
        .map(str::to_owned);
    let version_u64 = object
        .get("version")
        .and_then(Value::as_u64)
        .with_context(|| {
            format!(
                "Saved {} has no valid version; the original file has been left untouched",
                spec.label
            )
        })?;
    let original_version = u32::try_from(version_u64).with_context(|| {
        format!(
            "Unsupported {} version {version_u64}; the file has been left untouched",
            spec.label
        )
    })?;

    anyhow::ensure!(
        original_version >= spec.min_legacy,
        "Unsupported {} version {original_version}; the file has been left untouched",
        spec.label
    );

    let min_reader_version = match object.get(MIN_READER_VERSION) {
        Some(raw) => {
            let parsed = raw.as_u64().with_context(|| {
                format!(
                    "Saved {} has an invalid min_reader_version; the original file has been left untouched",
                    spec.label
                )
            })?;
            u32::try_from(parsed).with_context(|| {
                format!(
                    "Unsupported {} version {original_version}; the file has been left untouched",
                    spec.label
                )
            })?
        }
        None => original_version,
    };

    // Forward-compatible newer document.
    if original_version > spec.current {
        anyhow::ensure!(
            min_reader_version <= spec.current,
            "Unsupported {} version {original_version}; the file has been left untouched",
            spec.label
        );
        return Ok(MigratedDocument {
            value,
            written_by,
            original_version,
            migrated_from: None,
            preserved_version: Some(PreservedVersion {
                version: original_version,
                min_reader_version,
            }),
        });
    }

    // Legacy versions below `legacy_ceiling` are normalized by existing
    // deserialization logic, then enter the sequential migration chain at
    // `legacy_ceiling`.
    let mut current = original_version.max(spec.legacy_ceiling);
    let mut ran_migration = false;
    while current < spec.current {
        let step = spec
            .migrations
            .iter()
            .find(|migration| migration.from == current)
            .with_context(|| {
                format!(
                    "Missing {} migration from version {current} toward {}",
                    spec.label, spec.current
                )
            })?;
        anyhow::ensure!(
            step.to > step.from && step.to <= spec.current,
            "Invalid {} migration step {} -> {}",
            spec.label,
            step.from,
            step.to
        );
        (step.apply)(object).with_context(|| {
            format!(
                "Could not migrate {} from version {} to {}",
                spec.label, step.from, step.to
            )
        })?;
        current = step.to;
        ran_migration = true;
    }

    // Only update the JSON `version` field when the original version was already
    // at or above `legacy_ceiling`. Legacy versions (e.g. workspace v1..v7) keep
    // their original version in JSON so `Workspace::normalize()` can inspect
    // `self.version < 4` before setting `WORKSPACE_VERSION`.
    if ran_migration && original_version >= spec.legacy_ceiling {
        object.insert("version".into(), Value::from(spec.current));
    }

    Ok(MigratedDocument {
        value,
        written_by,
        original_version,
        migrated_from: (original_version < spec.current).then_some(original_version),
        preserved_version: None,
    })
}

/// Applies file envelope metadata (`version`, `min_reader_version`,
/// `written_by`) to a serialized JSON value before writing it to disk.
pub fn stamp_envelope(value: &mut Value, spec: &VersionSpec, preserved: Option<PreservedVersion>) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if let Some(preserved) = preserved {
        object.insert("version".into(), Value::from(preserved.version));
        object.insert(
            MIN_READER_VERSION.into(),
            Value::from(preserved.min_reader_version),
        );
    } else {
        object.insert("version".into(), Value::from(spec.current));
        object.insert(MIN_READER_VERSION.into(), Value::from(spec.min_reader));
    }
    object.insert(
        WRITTEN_BY.into(),
        Value::from(crate::build_info::RELEASE_VERSION),
    );
}

/// Writes a pre-migration backup of `data` into `backup_dir` as
/// `<stem>.v<from_version>.backup.json`, readable only by the owner.
pub fn back_up_version(
    source_path: &Path,
    backup_dir: &Path,
    from_version: u32,
    data: &[u8],
) -> Result<PathBuf> {
    std::fs::create_dir_all(backup_dir).context("Could not create backup directory")?;
    let stem = source_path
        .file_stem()
        .context("Invalid file path")?
        .to_string_lossy();
    let backup = backup_dir.join(format!("{stem}.v{from_version}.backup.json"));
    super::write_atomically(&backup, data).context("Could not write pre-migration backup")?;
    Ok(backup)
}

/// Migration from workspace v8 to v9.
///
/// v9 introduces the multi-workspace directory layout with global
/// `settings.json` and the sequential migration pipeline. Standalone v8 files
/// (for example in isolated single-file tests) upgrade cleanly to v9 without
/// dropping fields.
fn migrate_workspace_v8_to_v9(_doc: &mut Map<String, Value>) -> Result<()> {
    Ok(())
}

/// Migration specification for `workspace.json`.
///
/// Legacy versions `1..=8` continue to use existing serde defaults and
/// [`crate::model::Workspace::normalize`]. All new schema transitions starting
/// from `8 -> 9` live in `migrations`.
pub const WORKSPACE_SPEC: VersionSpec = VersionSpec {
    label: "workspace",
    current: crate::model::WORKSPACE_VERSION,
    min_legacy: 1,
    legacy_ceiling: 8,
    min_reader: crate::model::WORKSPACE_MIN_READER_VERSION,
    migrations: &[Migration {
        from: 8,
        to: 9,
        apply: migrate_workspace_v8_to_v9,
    }],
};

/// Migration specification for `settings.json`.
pub const SETTINGS_SPEC: VersionSpec = VersionSpec {
    label: "settings",
    current: crate::model::SETTINGS_VERSION,
    min_legacy: 1,
    legacy_ceiling: 1,
    min_reader: crate::model::SETTINGS_MIN_READER_VERSION,
    migrations: &[],
};

/// Migration specification for `workspaces.json`.
pub const REGISTRY_SPEC: VersionSpec = VersionSpec {
    label: "workspace registry",
    current: crate::model::REGISTRY_VERSION,
    min_legacy: 1,
    legacy_ceiling: 1,
    min_reader: crate::model::REGISTRY_MIN_READER_VERSION,
    migrations: &[],
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bump_1_to_2(doc: &mut Map<String, Value>) -> Result<()> {
        if let Some(old) = doc.remove("old_name") {
            doc.insert("new_name".into(), old);
        }
        Ok(())
    }

    fn bump_2_to_3(doc: &mut Map<String, Value>) -> Result<()> {
        doc.insert("added_in_v3".into(), json!(true));
        Ok(())
    }

    const TEST_SPEC: VersionSpec = VersionSpec {
        label: "test document",
        current: 3,
        min_legacy: 1,
        legacy_ceiling: 1,
        min_reader: 2,
        migrations: &[
            Migration {
                from: 1,
                to: 2,
                apply: bump_1_to_2,
            },
            Migration {
                from: 2,
                to: 3,
                apply: bump_2_to_3,
            },
        ],
    };

    #[test]
    fn runs_sequential_multi_hop_migrations() {
        let raw = br#"{"version": 1, "old_name": "hello", "written_by": "0.6.0"}"#;
        let migrated = migrate_bytes(raw, &TEST_SPEC).unwrap();
        assert_eq!(migrated.original_version, 1);
        assert_eq!(migrated.migrated_from, Some(1));
        assert_eq!(migrated.preserved_version, None);
        assert_eq!(migrated.written_by.as_deref(), Some("0.6.0"));
        assert_eq!(migrated.value["version"], 3);
        assert!(migrated.value.get("old_name").is_none());
        assert_eq!(migrated.value["new_name"], "hello");
        assert_eq!(migrated.value["added_in_v3"], true);
    }

    #[test]
    fn allows_forward_compatible_newer_version_and_preserves_version_metadata() {
        let raw = br#"{"version": 5, "min_reader_version": 3, "future_field": 42}"#;
        let migrated = migrate_bytes(raw, &TEST_SPEC).unwrap();
        assert_eq!(migrated.original_version, 5);
        assert_eq!(migrated.migrated_from, None);
        assert_eq!(
            migrated.preserved_version,
            Some(PreservedVersion {
                version: 5,
                min_reader_version: 3,
            })
        );
        let mut out = json!({"version": 3});
        stamp_envelope(&mut out, &TEST_SPEC, migrated.preserved_version);
        assert_eq!(out["version"], 5);
        assert_eq!(out["min_reader_version"], 3);
        assert_eq!(out["written_by"], crate::build_info::RELEASE_VERSION);
    }

    #[test]
    fn rejects_incompatible_newer_version() {
        let raw = br#"{"version": 5, "min_reader_version": 4}"#;
        let error = migrate_bytes(raw, &TEST_SPEC).unwrap_err().to_string();
        assert!(
            error.contains("Unsupported test document version 5"),
            "{error}"
        );

        let raw_no_min = br#"{"version": 4}"#;
        let error = migrate_bytes(raw_no_min, &TEST_SPEC)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Unsupported test document version 4"),
            "{error}"
        );
    }

    #[test]
    fn rejects_version_below_min_legacy() {
        let raw = br#"{"version": 0}"#;
        let error = migrate_bytes(raw, &TEST_SPEC).unwrap_err().to_string();
        assert!(
            error.contains("Unsupported test document version 0"),
            "{error}"
        );
    }

    #[test]
    fn writes_pre_migration_backup_with_version_in_filename() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("workspace.json");
        let backups = dir.path().join("backups");
        let raw = br#"{"version": 8}"#;
        let backup = back_up_version(&source, &backups, 8, raw).unwrap();
        assert_eq!(backup, backups.join("workspace.v8.backup.json"));
        assert_eq!(std::fs::read(&backup).unwrap(), raw);
    }
}
