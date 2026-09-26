use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const PREVIEW_ROWS: usize = 1_000;
pub const WORKSPACE_VERSION: u32 = 3;
pub const MAX_RESULT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RESULT_ROWS: usize = 100_000;
pub const MAX_PROFILE_NAME: usize = 60;
pub const MAX_TAB_TITLE: usize = 60;
pub const MAX_ASSISTANT_CONVERSATION_TITLE: usize = 120;
pub const ASSISTANT_DATA_SHARING_NOTICE_VERSION: u32 = 1;
pub const MIN_ASSISTANT_PANEL_WIDTH: f32 = 360.;
pub const DEFAULT_ASSISTANT_PANEL_WIDTH: f32 = 660.;
pub const MAX_ASSISTANT_PANEL_WIDTH: f32 = 900.;

fn title_with_suffix(title: &str, suffix: &str) -> String {
    let available = MAX_TAB_TITLE.saturating_sub(suffix.chars().count());
    let base: String = title.chars().take(available).collect();
    format!("{base}{suffix}")
}

/// Return a unique profile name for a duplicated connection.
pub fn copied_profile_name<F>(name: &str, is_taken: F) -> String
where
    F: Fn(&str) -> bool,
{
    for copy in 1.. {
        let suffix = if copy == 1 {
            " copy".to_owned()
        } else {
            format!(" copy {copy}")
        };
        let available = MAX_PROFILE_NAME.saturating_sub(suffix.chars().count());
        let base: String = name.chars().take(available).collect();
        let candidate = format!("{base}{suffix}");
        if !is_taken(&candidate) {
            return candidate;
        }
    }
    unreachable!("profile name search must find a bounded name")
}

/// Return a copy title that is different from every title accepted by `is_taken`.
pub fn copied_tab_title<F>(title: &str, is_taken: F) -> String
where
    F: Fn(&str) -> bool,
{
    for copy in 1.. {
        let suffix = if copy == 1 {
            " (Copy)".to_owned()
        } else {
            format!(" (Copy {copy})")
        };
        let candidate = title_with_suffix(title, &suffix);
        if !is_taken(&candidate) {
            return candidate;
        }
    }
    unreachable!("copy title search must find a bounded title")
}

/// Keep a title when it is available, otherwise append a copy suffix.
pub fn unique_tab_title<F>(title: &str, is_taken: F) -> String
where
    F: Fn(&str) -> bool,
{
    if !is_taken(title) {
        title.to_owned()
    } else {
        copied_tab_title(title, is_taken)
    }
}

pub const MIN_UI_SCALE: f32 = 0.75;
pub const MAX_UI_SCALE: f32 = 1.50;
pub const UI_SCALE_STEP: f32 = 0.10;
pub const MIN_EDITOR_FONT_SIZE: f32 = 10.;
pub const MAX_EDITOR_FONT_SIZE: f32 = 32.;
pub const MIN_LINE_HEIGHT: f32 = 1.;
pub const MAX_LINE_HEIGHT: f32 = 2.;
pub const LINE_HEIGHT_STEP: f32 = 0.1;
pub const SYSTEM_FONT_FAMILY: &str = ".SystemUIFont";

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
    pub assistant_font_family: String,
    pub assistant_font_size: f32,
    pub assistant_line_height: f32,
    pub assistant: AssistantSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_scale: 1.,
            ui_font_family: SYSTEM_FONT_FAMILY.into(),
            editor_font_family: "Menlo".into(),
            editor_font_size: 13.,
            editor_line_height: 1.2,
            logs_font_family: "Menlo".into(),
            logs_font_size: 13.,
            logs_line_height: 1.2,
            assistant_font_family: SYSTEM_FONT_FAMILY.into(),
            assistant_font_size: 14.,
            assistant_line_height: 1.6,
            assistant: AssistantSettings::default(),
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

        self.assistant_font_family = self.assistant_font_family.trim().into();
        if self.assistant_font_family.is_empty() {
            self.assistant_font_family = Self::default().assistant_font_family;
        }
        if !self.assistant_font_size.is_finite() {
            self.assistant_font_size = Self::default().assistant_font_size;
        }
        self.assistant_font_size = self
            .assistant_font_size
            .round()
            .clamp(MIN_EDITOR_FONT_SIZE, MAX_EDITOR_FONT_SIZE);
        if !self.assistant_line_height.is_finite() {
            self.assistant_line_height = Self::default().assistant_line_height;
        }
        self.assistant_line_height = (self.assistant_line_height * 10.).round() / 10.;
        self.assistant_line_height = self
            .assistant_line_height
            .clamp(MIN_LINE_HEIGHT, MAX_LINE_HEIGHT);

        self.assistant.sanitize();
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantExecutionMode {
    #[default]
    AskBeforeRunning,
    RunAutomatically,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(default)]
pub struct AssistantSettings {
    pub enabled: bool,
    pub data_sharing_notice_version: u32,
    pub default_execution_mode: AssistantExecutionMode,
    pub codex_executable: Option<String>,
    pub panel_width: f32,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub service_tier: Option<String>,
}

impl Default for AssistantSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            data_sharing_notice_version: 0,
            default_execution_mode: AssistantExecutionMode::AskBeforeRunning,
            codex_executable: None,
            panel_width: DEFAULT_ASSISTANT_PANEL_WIDTH,
            model: None,
            reasoning_effort: None,
            service_tier: None,
        }
    }
}

impl AssistantSettings {
    pub fn sanitize(&mut self) {
        if self.data_sharing_notice_version != ASSISTANT_DATA_SHARING_NOTICE_VERSION {
            self.enabled = false;
            self.data_sharing_notice_version = 0;
        }
        if !self.panel_width.is_finite() {
            self.panel_width = DEFAULT_ASSISTANT_PANEL_WIDTH;
        }
        self.panel_width = self
            .panel_width
            .clamp(MIN_ASSISTANT_PANEL_WIDTH, MAX_ASSISTANT_PANEL_WIDTH);
        sanitize_optional_string(&mut self.codex_executable);
        sanitize_optional_string(&mut self.model);
        sanitize_optional_string(&mut self.reasoning_effort);
        sanitize_optional_string(&mut self.service_tier);
    }
}

fn sanitize_optional_string(value: &mut Option<String>) {
    if let Some(text) = value {
        *text = text.trim().to_owned();
        if text.is_empty() {
            *value = None;
        }
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
    /// Return whether two profiles open the same authenticated session.
    ///
    /// The display name and idle policy can change while a session remains
    /// usable. The profile id stays part of the identity because it selects
    /// the stored password.
    pub fn connection_identity_eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.host == other.host
            && self.port == other.port
            && self.username == other.username
            && self.database == other.database
            && self.parameters == other.parameters
    }

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantTitleSource {
    #[default]
    Temporary,
    Codex,
    User,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct AssistantConversation {
    pub thread_id: String,
    pub title: String,
    pub title_source: AssistantTitleSource,
    pub last_activity: u64,
    pub execution_mode: AssistantExecutionMode,
}

impl AssistantConversation {
    pub fn new(thread_id: impl Into<String>, execution_mode: AssistantExecutionMode) -> Self {
        Self {
            thread_id: thread_id.into(),
            title: "New conversation".into(),
            title_source: AssistantTitleSource::Temporary,
            last_activity: 0,
            execution_mode,
        }
    }

    fn sanitize(&mut self) {
        self.thread_id = self.thread_id.trim().to_owned();
        self.title = self.title.trim().to_owned();
        if self.title.is_empty() {
            self.title = "New conversation".into();
            self.title_source = AssistantTitleSource::Temporary;
        }
        self.title = self
            .title
            .chars()
            .take(MAX_ASSISTANT_CONVERSATION_TITLE)
            .collect();
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(default)]
pub struct AssistantWorkspace {
    pub conversations: Vec<AssistantConversation>,
    pub selected_thread: Option<String>,
}

impl AssistantWorkspace {
    pub fn sanitize(&mut self) {
        let mut thread_ids = BTreeSet::new();
        self.conversations.retain_mut(|conversation| {
            conversation.sanitize();
            !conversation.thread_id.is_empty() && thread_ids.insert(conversation.thread_id.clone())
        });
        sanitize_optional_string(&mut self.selected_thread);
        // No selection asks the assistant to start a new conversation.
        if self.selected_thread.as_ref().is_some_and(|selected| {
            !self
                .conversations
                .iter()
                .any(|conversation| conversation.thread_id == *selected)
        }) {
            self.selected_thread = self
                .conversations
                .first()
                .map(|conversation| conversation.thread_id.clone());
        }
    }

    /// Remove conversations without a turn. Codex keeps them only in the
    /// memory of its current process, so another process cannot resume them.
    pub fn remove_unstarted(&mut self, unstarted: &BTreeSet<String>) {
        self.conversations
            .retain(|conversation| !unstarted.contains(&conversation.thread_id));
        if self
            .selected_thread
            .as_ref()
            .is_some_and(|selected| unstarted.contains(selected))
        {
            self.selected_thread = None;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Workspace {
    pub version: u32,
    #[serde(default)]
    pub settings: Settings,
    pub profiles: Vec<Profile>,
    pub tabs: Vec<SavedTab>,
    pub active_tab: usize,
    #[serde(default)]
    pub active_tabs: BTreeMap<Uuid, Uuid>,
    #[serde(default)]
    pub assistant: AssistantWorkspace,
}

impl Workspace {
    /// Upgrade the flat tab list used by version 1 workspaces. New workspaces
    /// still use one list in storage, but every tab is normalized to an owned
    /// connection and each connection gets an active tab.
    pub fn normalize(&mut self) {
        let active_profile = self.tabs.get(self.active_tab).and_then(|tab| tab.profile);
        let fallback = active_profile.filter(|id| self.profiles.iter().any(|p| p.id == *id));
        let fallback = fallback.or_else(|| self.profiles.first().map(|p| p.id));
        for tab in &mut self.tabs {
            if tab.profile.is_none() && fallback.is_some() {
                tab.profile = fallback;
            }
            if tab
                .profile
                .is_some_and(|id| !self.profiles.iter().any(|p| p.id == id))
            {
                tab.profile = fallback;
            }
        }
        for profile in &self.profiles {
            if !self.tabs.iter().any(|tab| tab.profile == Some(profile.id)) {
                self.tabs.push(SavedTab::new(1, Some(profile.id)));
            }
        }
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
        self.active_tabs.retain(|profile, tab| {
            self.profiles.iter().any(|p| p.id == *profile)
                && self
                    .tabs
                    .iter()
                    .any(|candidate| candidate.id == *tab && candidate.profile == Some(*profile))
        });
        for profile in &self.profiles {
            let first = self
                .tabs
                .iter()
                .find(|tab| tab.profile == Some(profile.id))
                .map(|tab| tab.id);
            if let Some(first) = first {
                self.active_tabs.entry(profile.id).or_insert(first);
            }
        }
        if let Some(tab) = self.tabs.get(self.active_tab)
            && let Some(profile) = tab.profile
        {
            self.active_tabs.insert(profile, tab.id);
        } else if let Some(profile) = self.profiles.first().map(|p| p.id)
            && let Some(tab) = self.tabs.iter().find(|tab| tab.profile == Some(profile))
        {
            self.active_tab = self
                .tabs
                .iter()
                .position(|candidate| candidate.id == tab.id)
                .unwrap_or(0);
            self.active_tabs.insert(profile, tab.id);
        }
        let mut used_titles: BTreeMap<Option<Uuid>, BTreeSet<String>> = BTreeMap::new();
        for tab in &mut self.tabs {
            let profile = tab.profile;
            let title = if tab.title.trim().is_empty() {
                "Query 1".to_owned()
            } else {
                tab.title.trim().to_owned()
            };
            let used = used_titles.entry(profile).or_default();
            tab.title = unique_tab_title(&title, |candidate| used.contains(candidate));
            used.insert(tab.title.clone());
        }
        self.assistant.sanitize();
        self.version = WORKSPACE_VERSION;
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            version: WORKSPACE_VERSION,
            settings: Settings::default(),
            profiles: vec![],
            tabs: vec![SavedTab::new(1, None)],
            active_tab: 0,
            active_tabs: BTreeMap::new(),
            assistant: AssistantWorkspace::default(),
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
    fn connection_identity_excludes_name_and_lifecycle() {
        let original = Profile::default();
        let mut updated = original.clone();
        updated.name = "Renamed".into();
        updated.lifecycle.idle_seconds = 60;
        assert!(original.connection_identity_eq(&updated));

        updated.host = "other-host".into();
        assert!(!original.connection_identity_eq(&updated));
        updated = original.clone();
        updated.id = uuid::Uuid::new_v4();
        assert!(!original.connection_identity_eq(&updated));
    }

    #[test]
    fn normalize_assigns_legacy_tabs_and_adds_one_tab_per_connection() {
        let first = Profile::default();
        let second = Profile::default();
        let mut unassigned = SavedTab::new(1, None);
        unassigned.sql = "select λ".into();
        let mut workspace = Workspace {
            version: 1,
            profiles: vec![first.clone(), second.clone()],
            tabs: vec![unassigned],
            active_tab: 0,
            active_tabs: BTreeMap::new(),
            ..Workspace::default()
        };
        workspace.normalize();
        assert_eq!(workspace.version, WORKSPACE_VERSION);
        assert_eq!(workspace.tabs[0].profile, Some(first.id));
        assert!(
            workspace
                .tabs
                .iter()
                .any(|tab| tab.profile == Some(second.id))
        );
        assert_eq!(workspace.active_tabs[&first.id], workspace.tabs[0].id);
        assert!(workspace.active_tabs.contains_key(&second.id));
        assert_eq!(workspace.tabs[0].sql, "select λ");
    }

    #[test]
    fn copied_tab_titles_use_copy_suffixes_and_stay_within_limit() {
        let mut used = BTreeSet::new();
        let first = copied_tab_title("Query 1", |title| used.contains(title));
        used.insert(first.clone());
        let second = copied_tab_title("Query 1", |title| used.contains(title));
        assert_eq!(first, "Query 1 (Copy)");
        assert_eq!(second, "Query 1 (Copy 2)");
        assert!(second.chars().count() <= MAX_TAB_TITLE);

        let long = copied_tab_title(&"x".repeat(MAX_TAB_TITLE), |_| false);
        assert_eq!(long, format!("{} (Copy)", "x".repeat(MAX_TAB_TITLE - 7)));
    }

    #[test]
    fn copied_profile_names_are_unique_and_stay_within_limit() {
        let mut used = BTreeSet::new();
        let first = copied_profile_name("Spark", |name| used.contains(name));
        used.insert(first.clone());
        let second = copied_profile_name("Spark", |name| used.contains(name));
        assert_eq!(first, "Spark copy");
        assert_eq!(second, "Spark copy 2");
        assert!(second.chars().count() <= MAX_PROFILE_NAME);

        let long = copied_profile_name(&"x".repeat(MAX_PROFILE_NAME), |_| false);
        assert_eq!(long, format!("{} copy", "x".repeat(MAX_PROFILE_NAME - 5)));
    }

    #[test]
    fn normalize_makes_tab_titles_unique_per_connection() {
        let first = Profile::default();
        let second = Profile::default();
        let first_id = first.id;
        let second_id = second.id;
        let mut first_tab = SavedTab::new(1, Some(first_id));
        first_tab.title = "Shared".into();
        let mut duplicate = SavedTab::new(2, Some(first_id));
        duplicate.title = "Shared".into();
        let mut other_connection = SavedTab::new(1, Some(second_id));
        other_connection.title = "Shared".into();
        let mut workspace = Workspace {
            version: WORKSPACE_VERSION,
            profiles: vec![first, second],
            tabs: vec![first_tab, duplicate, other_connection],
            active_tab: 0,
            active_tabs: BTreeMap::new(),
            ..Workspace::default()
        };

        workspace.normalize();

        assert_eq!(workspace.tabs[0].title, "Shared");
        assert_eq!(workspace.tabs[1].title, "Shared (Copy)");
        assert_eq!(workspace.tabs[2].title, "Shared");
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
            assistant_font_family: "   ".into(),
            assistant_font_size: f32::NAN,
            assistant_line_height: f32::NAN,
            assistant: AssistantSettings::default(),
        };
        settings.sanitize();
        assert_eq!(settings, Settings::default());

        settings.ui_scale = 1.46;
        settings.editor_font_size = 33.;
        settings.editor_line_height = 2.1;
        settings.logs_font_size = 33.;
        settings.logs_line_height = 2.1;
        settings.assistant_font_size = 33.;
        settings.assistant_line_height = 2.1;
        settings.sanitize();
        assert_eq!(settings.ui_scale, 1.46);
        assert_eq!(settings.editor_font_size, MAX_EDITOR_FONT_SIZE);
        assert_eq!(settings.editor_line_height, MAX_LINE_HEIGHT);
        assert_eq!(settings.logs_font_size, MAX_EDITOR_FONT_SIZE);
        assert_eq!(settings.logs_line_height, MAX_LINE_HEIGHT);
        assert_eq!(settings.assistant_font_size, MAX_EDITOR_FONT_SIZE);
        assert_eq!(settings.assistant_line_height, MAX_LINE_HEIGHT);
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
        assert_eq!(
            settings.assistant_font_family,
            Settings::default().assistant_font_family
        );
        assert_eq!(
            settings.assistant_font_size,
            Settings::default().assistant_font_size
        );
        assert_eq!(
            settings.assistant_line_height,
            Settings::default().assistant_line_height
        );

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

    #[test]
    fn assistant_settings_restore_safe_defaults_and_sanitize_values() {
        let restored: AssistantSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(restored, AssistantSettings::default());
        assert!(!restored.enabled);
        assert_eq!(
            restored.default_execution_mode,
            AssistantExecutionMode::AskBeforeRunning
        );

        let mut settings = AssistantSettings {
            enabled: true,
            panel_width: f32::NAN,
            codex_executable: Some("   ".into()),
            model: Some(" gpt-test ".into()),
            reasoning_effort: Some(" high ".into()),
            service_tier: Some(" fast ".into()),
            ..AssistantSettings::default()
        };
        settings.sanitize();
        assert_eq!(settings.panel_width, DEFAULT_ASSISTANT_PANEL_WIDTH);
        assert!(!settings.enabled);
        assert_eq!(settings.data_sharing_notice_version, 0);
        assert_eq!(settings.codex_executable, None);
        assert_eq!(settings.model.as_deref(), Some("gpt-test"));
        assert_eq!(settings.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(settings.service_tier.as_deref(), Some("fast"));

        settings.panel_width = MAX_ASSISTANT_PANEL_WIDTH + 1.;
        settings.enabled = true;
        settings.data_sharing_notice_version = ASSISTANT_DATA_SHARING_NOTICE_VERSION;
        settings.sanitize();
        assert_eq!(settings.panel_width, MAX_ASSISTANT_PANEL_WIDTH);
        assert!(settings.enabled);
    }

    #[test]
    fn assistant_workspace_removes_invalid_threads_and_repairs_selection() {
        let mut first =
            AssistantConversation::new(" thread-1 ", AssistantExecutionMode::RunAutomatically);
        first.title = format!("  {}  ", "x".repeat(MAX_ASSISTANT_CONVERSATION_TITLE + 5));
        let mut duplicate =
            AssistantConversation::new("thread-1", AssistantExecutionMode::AskBeforeRunning);
        duplicate.title = "Duplicate".into();
        let blank = AssistantConversation::new("   ", AssistantExecutionMode::AskBeforeRunning);
        let mut assistant = AssistantWorkspace {
            conversations: vec![first, duplicate, blank],
            selected_thread: Some("missing".into()),
        };

        assistant.sanitize();

        assert_eq!(assistant.conversations.len(), 1);
        assert_eq!(assistant.conversations[0].thread_id, "thread-1");
        assert_eq!(
            assistant.conversations[0].title.chars().count(),
            MAX_ASSISTANT_CONVERSATION_TITLE
        );
        assert_eq!(assistant.selected_thread.as_deref(), Some("thread-1"));
        assert_eq!(
            assistant.conversations[0].execution_mode,
            AssistantExecutionMode::RunAutomatically
        );

        assistant.selected_thread = None;
        assistant.sanitize();
        assert_eq!(assistant.selected_thread, None);
    }

    #[test]
    fn assistant_workspace_removes_unstarted_threads() {
        let mut assistant = AssistantWorkspace {
            conversations: vec![
                AssistantConversation::new("used", AssistantExecutionMode::AskBeforeRunning),
                AssistantConversation::new("new", AssistantExecutionMode::AskBeforeRunning),
            ],
            selected_thread: Some("new".into()),
        };
        let mut other = assistant.clone();
        other.selected_thread = Some("used".into());
        let unstarted = BTreeSet::from(["new".to_owned()]);

        assistant.remove_unstarted(&unstarted);
        other.remove_unstarted(&unstarted);

        assert_eq!(assistant.conversations.len(), 1);
        assert_eq!(assistant.conversations[0].thread_id, "used");
        assert_eq!(assistant.selected_thread, None);
        assert_eq!(other.selected_thread.as_deref(), Some("used"));
    }
}
