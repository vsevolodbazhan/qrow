//! Captured transfer settings and pre-execution collection information.
use super::*;
use crate::model::transfer::{
    INCREMENTAL_KEY, IncrementalCollect, LEGACY_INCREMENTAL_KEY, SAVE_TO_FILE_KEY, Transfer,
    TransferPreset, incremental_override_applies,
};

pub(super) fn choices(profile: Option<&Profile>) -> Vec<String> {
    let preset = profile.map_or(TransferPreset::Balanced, |profile| profile.transfer.preset);
    std::iter::once(format!("Connection preset ({})", preset.label()))
        .chain(
            TransferPreset::ALL
                .into_iter()
                .map(|preset| preset.label().into()),
        )
        .collect()
}

impl ExportDialog {
    pub(super) fn transfer_profile(&self) -> Option<&Profile> {
        if self.scope.executes() {
            self.run.as_ref().map(|intent| &intent.profile)
        } else {
            self.profile.as_ref()
        }
    }

    pub(super) fn transfer_override(&self, cx: &App) -> Option<Transfer> {
        let profile = self.profile.as_ref()?;
        let choice = self.transfer.read(cx).selected_value()?;
        let Some(preset) = TransferPreset::ALL
            .into_iter()
            .find(|preset| preset.label() == choice.as_str())
        else {
            return Some(profile.transfer.clone());
        };
        let selected = Transfer {
            preset,
            ..profile.transfer.clone()
        };
        let mut settings = selected.settings();
        settings.concurrent_exports = profile.transfer.settings().concurrent_exports;
        Some(Transfer {
            preset: TransferPreset::Custom,
            custom: settings,
        })
    }
}

pub(super) fn notice(intent: &run::Intent) -> Option<String> {
    if intent.profile.database_type == crate::model::DatabaseType::Trino {
        return Some(format!(
            "Trino spooling: {}. The server can return classic results.",
            intent.profile.transfer.settings().trino_spooling.label()
        ));
    }
    if intent.profile.database_type != crate::model::DatabaseType::Kyuubi {
        return None;
    }
    let configured = intent.profile.transfer.settings().incremental_collect;
    let value = if !incremental_override_applies(&intent.sql) {
        "Inherit for settings or variable-substitution SQL".to_owned()
    } else if configured != IncrementalCollect::Inherit {
        format!(
            "{} from {}",
            configured.label(),
            intent.profile.transfer.preset.label()
        )
    } else {
        match intent
            .profile
            .parameters
            .get(INCREMENTAL_KEY)
            .or_else(|| intent.profile.parameters.get(LEGACY_INCREMENTAL_KEY))
        {
            Some(value) if value.eq_ignore_ascii_case("true") => {
                "On from connection parameters".into()
            }
            Some(value) if value.eq_ignore_ascii_case("false") => {
                "Off from connection parameters".into()
            }
            Some(_) => "Inherit; the connection parameter is invalid".into(),
            None => "Inherit from the session or server".into(),
        }
    };
    Some(format!(
        "Incremental collect: {value}. Session or server settings can override this value."
    ))
}

pub(super) fn save_to_file(intent: &run::Intent) -> bool {
    intent.profile.database_type == crate::model::DatabaseType::Kyuubi
        && intent
            .profile
            .parameters
            .get(SAVE_TO_FILE_KEY)
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn collection_notice_uses_the_captured_policy_and_keeps_settings_inherited() {
        let mut intent = run::Intent {
            profile: Profile::default(),
            sql: "SELECT 1".into(),
            expected: None,
            warning: false,
        };
        assert!(
            notice(&intent)
                .unwrap()
                .contains("Inherit from the session or server")
        );
        intent
            .profile
            .parameters
            .insert(INCREMENTAL_KEY.into(), "TRUE".into());
        assert!(
            notice(&intent)
                .unwrap()
                .contains("On from connection parameters")
        );
        intent.profile.transfer.preset = TransferPreset::Conservative;
        assert!(notice(&intent).unwrap().contains("On from Conservative"));
        intent.sql = "/* comment */ SET spark.sql.shuffle.partitions=2".into();
        assert!(notice(&intent).unwrap().contains("Inherit for settings"));
        intent.sql = "SELECT '${x}'".into();
        assert!(notice(&intent).unwrap().contains("Inherit for settings"));
        intent
            .profile
            .parameters
            .insert(SAVE_TO_FILE_KEY.into(), "true".into());
        assert!(save_to_file(&intent));
        intent.profile.database_type = crate::model::DatabaseType::Postgres;
        assert!(notice(&intent).is_none());
        assert!(!save_to_file(&intent));
    }
}
