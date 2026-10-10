//! Connection export policy. Execution overrides do not change session identity.
use serde::{Deserialize, Serialize};

pub const INCREMENTAL_KEY: &str = "kyuubi.engine.spark.operation.incremental.collect";
pub const LEGACY_INCREMENTAL_KEY: &str = "kyuubi.operation.incremental.collect";
pub const SAVE_TO_FILE_KEY: &str = "kyuubi.operation.result.saveToFile.enabled";
pub const MAX_FETCH_ROWS: usize = 100_000;

/// Settings statements and variable substitution retain their session meaning.
pub fn incremental_override_applies(sql: &str) -> bool {
    if sql.contains("${") {
        return false;
    }
    let first = crate::sql::tokens(sql).into_iter().find(|(range, kind)| {
        *kind != crate::sql::Kind::Comment && !sql[range.clone()].trim().is_empty()
    });
    !first.is_some_and(|(range, _)| {
        sql[range.clone()].eq_ignore_ascii_case("SET") || sql[range].eq_ignore_ascii_case("RESET")
    })
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPreset {
    Conservative,
    #[default]
    Balanced,
    Fast,
    Custom,
}

impl TransferPreset {
    pub const ALL: [Self; 4] = [Self::Conservative, Self::Balanced, Self::Fast, Self::Custom];
    pub fn label(self) -> &'static str {
        match self {
            Self::Conservative => "Conservative",
            Self::Balanced => "Balanced",
            Self::Fast => "Fast",
            Self::Custom => "Custom",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncrementalCollect {
    #[default]
    Inherit,
    On,
    Off,
}

impl IncrementalCollect {
    pub const ALL: [Self; 3] = [Self::Inherit, Self::On, Self::Off];
    pub fn label(self) -> &'static str {
        match self {
            Self::Inherit => "Inherit",
            Self::On => "On",
            Self::Off => "Off",
        }
    }
    pub fn value(self) -> Option<&'static str> {
        match self {
            Self::Inherit => None,
            Self::On => Some("true"),
            Self::Off => Some("false"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct TransferSettings {
    pub incremental_collect: IncrementalCollect,
    pub request_mib: u32,
    /// Estimated spool bytes per second, in decimal MB. Zero has no limit.
    pub speed_limit_mb: u32,
    pub concurrent_exports: u8,
}

impl Default for TransferSettings {
    fn default() -> Self {
        Self {
            incremental_collect: IncrementalCollect::Inherit,
            request_mib: 16,
            speed_limit_mb: 0,
            concurrent_exports: 1,
        }
    }
}

impl TransferSettings {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=32).contains(&self.request_mib),
            "Data per request must be from 1 to 32 MiB."
        );
        anyhow::ensure!(
            self.speed_limit_mb <= 1000,
            "Speed limit must be from 0 to 1000 MB/s."
        );
        anyhow::ensure!(
            (1..=2).contains(&self.concurrent_exports),
            "Exports at the same time must be 1 or 2."
        );
        Ok(())
    }

    /// An estimate with a 25% margin. Response and allocation caps still apply.
    pub fn request_rows(&self, widest_owned_row: usize) -> usize {
        let width = widest_owned_row.max(1024);
        let estimate = width.saturating_add(width / 4);
        (self.request_mib as usize * 1024 * 1024 / estimate).clamp(1, MAX_FETCH_ROWS)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Transfer {
    pub preset: TransferPreset,
    pub custom: TransferSettings,
}

impl Transfer {
    pub fn settings(&self) -> TransferSettings {
        match self.preset {
            TransferPreset::Conservative => TransferSettings {
                incremental_collect: IncrementalCollect::On,
                request_mib: 4,
                speed_limit_mb: 20,
                concurrent_exports: 1,
            },
            TransferPreset::Balanced => TransferSettings::default(),
            TransferPreset::Fast => TransferSettings {
                request_mib: 32,
                concurrent_exports: 2,
                ..TransferSettings::default()
            },
            TransferPreset::Custom => self.custom.clone(),
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        self.custom.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_statements_and_substitution_keep_the_session_configuration() {
        for sql in [
            "SET key=true",
            "/* note */ reset key",
            "SELECT '${hiveconf:key}'",
            "${command}",
        ] {
            assert!(!incremental_override_applies(sql), "{sql}");
        }
        for sql in [
            "-- note\nSELECT 1",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "SHOW TABLES",
        ] {
            assert!(incremental_override_applies(sql), "{sql}");
        }
    }

    #[test]
    fn preset_values_and_custom_round_trip_are_stable() {
        let expected = [
            (IncrementalCollect::On, 4, 20, 1),
            (IncrementalCollect::Inherit, 16, 0, 1),
            (IncrementalCollect::Inherit, 32, 0, 2),
        ];
        for (preset, expected) in TransferPreset::ALL[..3].iter().zip(expected) {
            let settings = Transfer {
                preset: *preset,
                ..Default::default()
            }
            .settings();
            assert_eq!(
                (
                    settings.incremental_collect,
                    settings.request_mib,
                    settings.speed_limit_mb,
                    settings.concurrent_exports
                ),
                expected
            );
        }
        let transfer = Transfer {
            preset: TransferPreset::Custom,
            custom: TransferSettings {
                incremental_collect: IncrementalCollect::Off,
                request_mib: 7,
                speed_limit_mb: 12,
                concurrent_exports: 2,
            },
        };
        assert_eq!(
            serde_json::from_str::<Transfer>(&serde_json::to_string(&transfer).unwrap()).unwrap(),
            transfer
        );
        assert_eq!(
            serde_json::from_str::<Transfer>("{}").unwrap(),
            Transfer::default()
        );
    }

    #[test]
    fn adaptive_requests_shrink_for_wide_rows_and_stay_bounded() {
        let settings = TransferSettings::default();
        assert_eq!(settings.request_rows(0), 13_107);
        assert_eq!(settings.request_rows(4096), 3276);
        assert_eq!(settings.request_rows(16 * 1024 * 1024), 1);
        assert_eq!(settings.request_rows(usize::MAX), 1);
        let mut invalid = settings;
        invalid.request_mib = 33;
        assert!(invalid.validate().is_err());
        invalid.request_mib = 16;
        invalid.concurrent_exports = 0;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn transfer_edits_preserve_session_identity_and_legacy_defaults() {
        let profile = super::super::Profile::default();
        let mut json = serde_json::to_value(&profile).unwrap();
        json.as_object_mut().unwrap().remove("transfer");
        assert_eq!(
            serde_json::from_value::<super::super::Profile>(json)
                .unwrap()
                .transfer,
            Transfer::default()
        );
        let mut edited = profile.clone();
        edited.transfer.preset = TransferPreset::Fast;
        assert!(profile.connection_identity_eq(&edited));
    }
}
