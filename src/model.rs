use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const PREVIEW_ROWS: usize = 1_000;
pub const MAX_RESULT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RESULT_ROWS: usize = 100_000;
pub const MAX_PROFILE_NAME: usize = 60;

pub const MIN_UI_SCALE: f32 = 0.75;
pub const MAX_UI_SCALE: f32 = 1.50;
pub const UI_SCALE_STEP: f32 = 0.10;
pub const MIN_EDITOR_FONT_SIZE: f32 = 10.;
pub const MAX_EDITOR_FONT_SIZE: f32 = 32.;
pub const MIN_LINE_HEIGHT: f32 = 1.;
pub const MAX_LINE_HEIGHT: f32 = 2.;
pub const LINE_HEIGHT_STEP: f32 = 0.1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub ui_scale: f32,
    pub ui_font_family: String,
    pub editor_font_family: String,
    pub editor_font_size: f32,
    pub editor_line_height: f32,
    pub logs_font_family: String,
    pub logs_font_size: f32,
    pub logs_line_height: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_scale: 1.,
            ui_font_family: ".SystemUIFont".into(),
            editor_font_family: "Menlo".into(),
            editor_font_size: 13.,
            editor_line_height: 1.2,
            logs_font_family: "Menlo".into(),
            logs_font_size: 13.,
            logs_line_height: 1.2,
        }
    }
}

impl Settings {
    pub fn sanitize(&mut self) {
        if !self.ui_scale.is_finite() {
            self.ui_scale = Self::default().ui_scale;
        }
        self.ui_scale = (self.ui_scale * 100.).round() / 100.;
        self.ui_scale = self.ui_scale.clamp(MIN_UI_SCALE, MAX_UI_SCALE);

        self.ui_font_family = self.ui_font_family.trim().into();
        if self.ui_font_family.is_empty() {
            self.ui_font_family = Self::default().ui_font_family;
        }

        if !self.editor_font_size.is_finite() {
            self.editor_font_size = Self::default().editor_font_size;
        }
        self.editor_font_size = self
            .editor_font_size
            .round()
            .clamp(MIN_EDITOR_FONT_SIZE, MAX_EDITOR_FONT_SIZE);

        if !self.editor_line_height.is_finite() {
            self.editor_line_height = Self::default().editor_line_height;
        }
        self.editor_line_height = (self.editor_line_height * 10.).round() / 10.;
        self.editor_line_height = self
            .editor_line_height
            .clamp(MIN_LINE_HEIGHT, MAX_LINE_HEIGHT);

        self.editor_font_family = self.editor_font_family.trim().into();
        if self.editor_font_family.is_empty() {
            self.editor_font_family = Self::default().editor_font_family;
        }

        self.logs_font_family = self.logs_font_family.trim().into();
        if self.logs_font_family.is_empty() {
            self.logs_font_family = Self::default().logs_font_family;
        }

        if !self.logs_font_size.is_finite() {
            self.logs_font_size = Self::default().logs_font_size;
        }
        self.logs_font_size = self
            .logs_font_size
            .round()
            .clamp(MIN_EDITOR_FONT_SIZE, MAX_EDITOR_FONT_SIZE);

        if !self.logs_line_height.is_finite() {
            self.logs_line_height = Self::default().logs_line_height;
        }
        self.logs_line_height = (self.logs_line_height * 10.).round() / 10.;
        self.logs_line_height = self
            .logs_line_height
            .clamp(MIN_LINE_HEIGHT, MAX_LINE_HEIGHT);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub database: String,
    pub parameters: BTreeMap<String, String>,
    #[serde(default)]
    pub lifecycle: ConnectionLifecycle,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConnectionLifecycle {
    pub idle_seconds: u64,
    /// Zero disables keep-alive and enables idle disconnection.
    pub keep_alive_seconds: u64,
    pub keep_alive_sql: String,
}

impl Default for ConnectionLifecycle {
    fn default() -> Self {
        Self {
            idle_seconds: 900,
            keep_alive_seconds: 0,
            keep_alive_sql: "SELECT 1".into(),
        }
    }
}

impl ConnectionLifecycle {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=604_800).contains(&self.idle_seconds),
            "Idle timeout must be between 1 and 604800 seconds."
        );
        anyhow::ensure!(
            self.keep_alive_seconds <= 604_800,
            "Keep-alive interval must be between 0 and 604800 seconds."
        );
        if self.keep_alive_seconds > 0 {
            crate::sql::validate_single(&self.keep_alive_sql)?;
        }
        Ok(())
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "Spark".into(),
            host: String::new(),
            port: 10009,
            username: String::new(),
            database: "avia".into(),
            parameters: BTreeMap::new(),
            lifecycle: ConnectionLifecycle::default(),
        }
    }
}

impl Profile {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.name.trim().is_empty(), "Give this connection a name.");
        anyhow::ensure!(
            self.name.chars().count() <= MAX_PROFILE_NAME,
            "Connection name must be {MAX_PROFILE_NAME} characters or fewer."
        );
        anyhow::ensure!(!self.host.trim().is_empty(), "Enter a host.");
        anyhow::ensure!(self.port != 0, "Port must be between 1 and 65535.");
        anyhow::ensure!(!self.username.trim().is_empty(), "Enter a username.");
        anyhow::ensure!(
            !self.username.contains('\0'),
            "Username contains a null character."
        );
        anyhow::ensure!(
            !self.database.trim().is_empty(),
            "Enter an initial database."
        );
        self.lifecycle.validate()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedTab {
    pub id: Uuid,
    pub title: String,
    pub sql: String,
    pub profile: Option<Uuid>,
}

impl SavedTab {
    pub fn new(number: usize, profile: Option<Uuid>) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: format!("Query {number}"),
            sql: String::new(),
            profile,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub version: u32,
    #[serde(default)]
    pub settings: Settings,
    pub profiles: Vec<Profile>,
    pub tabs: Vec<SavedTab>,
    pub active_tab: usize,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            version: 1,
            settings: Settings::default(),
            profiles: vec![],
            tabs: vec![SavedTab::new(1, None)],
            active_tab: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub data_type: String,
}

pub type Row = Vec<Option<String>>;

#[derive(Default, Debug)]
pub struct Batch {
    pub rows: Vec<Row>,
    pub more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_uses_a_neutral_spark_name() {
        assert_eq!(Profile::default().name, "Spark");
    }

    #[test]
    fn profile_name_is_limited_to_sixty_characters() {
        let mut profile = Profile {
            name: "n".repeat(MAX_PROFILE_NAME),
            host: "localhost".into(),
            username: "user".into(),
            ..Profile::default()
        };
        assert!(profile.validate().is_ok());
        profile.name.push('n');
        assert!(profile.validate().is_err());
    }

    #[test]
    fn old_profiles_restore_with_idle_disconnect_and_no_heartbeat() {
        let original = Profile::default();
        let mut json = serde_json::to_value(&original).unwrap();
        json.as_object_mut().unwrap().remove("lifecycle");
        let restored: Profile = serde_json::from_value(json).unwrap();
        assert_eq!(restored.id, original.id);
        assert_eq!(restored.lifecycle, ConnectionLifecycle::default());
        assert_eq!(restored.lifecycle.keep_alive_seconds, 0);
    }

    #[test]
    fn lifecycle_rejects_invalid_timers_and_multiple_heartbeat_statements() {
        let mut policy = ConnectionLifecycle {
            idle_seconds: 0,
            ..Default::default()
        };
        assert!(policy.validate().is_err());
        policy.idle_seconds = 900;
        policy.keep_alive_seconds = 604_801;
        assert!(policy.validate().is_err());
        policy.keep_alive_seconds = 300;
        policy.keep_alive_sql = "SELECT 1; SELECT 2".into();
        assert!(policy.validate().is_err());
        policy.keep_alive_sql = "".into();
        assert!(policy.validate().is_err());
        policy.keep_alive_sql = "SELECT 42".into();
        assert!(policy.validate().is_ok());
        let restored: ConnectionLifecycle =
            serde_json::from_str(&serde_json::to_string(&policy).unwrap()).unwrap();
        assert_eq!(restored, policy);
    }

    #[test]
    fn settings_restore_defaults_and_stay_within_supported_bounds() {
        let restored: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(restored, Settings::default());

        let mut settings = Settings {
            ui_scale: f32::INFINITY,
            ui_font_family: "   ".into(),
            editor_font_family: "   ".into(),
            editor_font_size: f32::NAN,
            editor_line_height: f32::NAN,
            logs_font_family: "   ".into(),
            logs_font_size: f32::NAN,
            logs_line_height: f32::NAN,
        };
        settings.sanitize();
        assert_eq!(settings, Settings::default());

        settings.ui_scale = 1.46;
        settings.editor_font_size = 33.;
        settings.editor_line_height = 2.1;
        settings.logs_font_size = 33.;
        settings.logs_line_height = 2.1;
        settings.sanitize();
        assert_eq!(settings.ui_scale, 1.46);
        assert_eq!(settings.editor_font_size, MAX_EDITOR_FONT_SIZE);
        assert_eq!(settings.editor_line_height, MAX_LINE_HEIGHT);
        assert_eq!(settings.logs_font_size, MAX_EDITOR_FONT_SIZE);
        assert_eq!(settings.logs_line_height, MAX_LINE_HEIGHT);
        let encoded = serde_json::to_string(&settings).unwrap();
        let mut restored: Settings = serde_json::from_str(&encoded).unwrap();
        restored.sanitize();
        assert_eq!(restored.ui_scale, 1.46);
        settings.ui_scale = 9.;
        settings.sanitize();
        assert_eq!(settings.ui_scale, MAX_UI_SCALE);
    }

    #[test]
    fn legacy_settings_keep_editor_preferences_and_default_ui_font() {
        let mut settings: Settings = serde_json::from_str(
            r#"{"ui_scale":1.2,"editor_font_family":"Monaco","editor_font_size":16}"#,
        )
        .unwrap();
        settings.sanitize();
        assert_eq!(settings.ui_font_family, Settings::default().ui_font_family);
        assert_eq!(settings.editor_font_family, "Monaco");
        assert_eq!(settings.editor_font_size, 16.);
        assert_eq!(
            settings.editor_line_height,
            Settings::default().editor_line_height
        );
        assert_eq!(
            settings.logs_font_family,
            Settings::default().logs_font_family
        );
        assert_eq!(settings.logs_font_size, Settings::default().logs_font_size);
        assert_eq!(
            settings.logs_line_height,
            Settings::default().logs_line_height
        );
        assert_eq!(settings.ui_scale, 1.2);

        settings.ui_font_family = " Helvetica ".into();
        settings.sanitize();
        assert_eq!(settings.ui_font_family, "Helvetica");
        assert_eq!(settings.editor_font_size, 16.);
    }

    #[test]
    fn removed_ui_font_size_does_not_prevent_restoring_settings() {
        let settings: Settings = serde_json::from_str(
            r#"{"ui_scale":1.2,"ui_font_family":"Helvetica","ui_font_size":20,"editor_font_family":"Monaco","editor_font_size":16}"#,
        )
        .unwrap();
        assert_eq!(settings.ui_scale, 1.2);
        assert_eq!(settings.ui_font_family, "Helvetica");
        assert_eq!(settings.editor_font_family, "Monaco");
        assert_eq!(settings.editor_font_size, 16.);
        let saved = serde_json::to_value(&settings).unwrap();
        assert!(saved.get("ui_font_size").is_none());
    }
}
