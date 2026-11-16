use crate::sql::{KeywordCase, SqlStyle};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
pub mod sharing;
pub mod transfer;

pub const PREVIEW_ROWS: usize = 1_000;
pub const WORKSPACE_VERSION: u32 = 8;
pub const MAX_RESULT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RESULT_ROWS: usize = 100_000;
pub const MAX_PROFILE_NAME: usize = 60;
pub const MAX_SIGN_IN_NAME: usize = 60;
/// The scopes that each sign-in requests: the account, and its name and
/// email for display.
pub const BASE_SCOPES: [&str; 3] = ["openid", "profile", "email"];

/// The scopes of `scopes` that are not in [`BASE_SCOPES`], without repeats.
pub fn extra_scopes(scopes: &[String]) -> Vec<&str> {
    let mut extra: Vec<&str> = Vec::new();
    for scope in scopes {
        if !BASE_SCOPES.contains(&scope.as_str()) && !extra.contains(&scope.as_str()) {
            extra.push(scope);
        }
    }
    extra
}

/// The most callback ports of one sign-in.
pub const MAX_CALLBACK_PORTS: usize = 16;
pub const MAX_TAB_TITLE: usize = 60;
pub const MAX_ASSISTANT_CONVERSATION_TITLE: usize = 120;
/// Version 2 adds schema, table, and column names and comments. Version 3
/// adds the assistant notes of connections. Version 4 adds the descriptions,
/// tests, and lineage of dbt projects.
pub const ASSISTANT_DATA_SHARING_NOTICE_VERSION: u32 = 5;
/// The largest assistant notes of one connection, in bytes.
pub const MAX_ASSISTANT_NOTES_BYTES: usize = 16 * 1024;
pub const MIN_ASSISTANT_PANEL_WIDTH: f32 = 360.;
pub const DEFAULT_ASSISTANT_PANEL_WIDTH: f32 = 660.;
pub const MAX_ASSISTANT_PANEL_WIDTH: f32 = 900.;
pub const SYSTEM_THEME: &str = "System";

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
    copied_name(name, MAX_PROFILE_NAME, is_taken)
}

/// Keep the name of a pasted or imported connection when it is available,
/// otherwise append a copy suffix.
pub fn unique_profile_name<F>(name: &str, is_taken: F) -> String
where
    F: Fn(&str) -> bool,
{
    if !is_taken(name) {
        name.to_owned()
    } else {
        copied_name(name, MAX_PROFILE_NAME, is_taken)
    }
}

/// Keep the name of a pasted sign-in when it is available, otherwise append
/// a copy suffix.
pub fn unique_sign_in_name<F>(name: &str, is_taken: F) -> String
where
    F: Fn(&str) -> bool,
{
    if !is_taken(name) {
        name.to_owned()
    } else {
        copied_name(name, MAX_SIGN_IN_NAME, is_taken)
    }
}

/// Return `name` with the first free copy suffix, within `max` characters.
fn copied_name<F>(name: &str, max: usize, is_taken: F) -> String
where
    F: Fn(&str) -> bool,
{
    for copy in 1.. {
        let suffix = if copy == 1 {
            " copy".to_owned()
        } else {
            format!(" copy {copy}")
        };
        let available = max.saturating_sub(suffix.chars().count());
        let base: String = name.chars().take(available).collect();
        let candidate = format!("{base}{suffix}");
        if !is_taken(&candidate) {
            return candidate;
        }
    }
    unreachable!("copy name search must find a bounded name")
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

/// Fit a conversation title to a tab and keep it unique within its connection.
pub fn conversation_tab_title<F>(title: &str, is_taken: F) -> Option<String>
where
    F: Fn(&str) -> bool,
{
    let title: String = title.trim().chars().take(MAX_TAB_TITLE).collect();
    let title = title.trim_end();
    (!title.is_empty()).then(|| unique_tab_title(title, is_taken))
}

pub const MIN_UI_SCALE: f32 = 0.75;
pub const MAX_UI_SCALE: f32 = 1.50;
pub const UI_SCALE_STEP: f32 = 0.10;
pub const MIN_EDITOR_FONT_SIZE: f32 = 10.;
pub const MAX_EDITOR_FONT_SIZE: f32 = 32.;
pub const MIN_LINE_HEIGHT: f32 = 1.;
pub const MAX_LINE_HEIGHT: f32 = 2.;
pub const LINE_HEIGHT_STEP: f32 = 0.1;
pub const MIN_TAB_SIZE: u8 = 1;
pub const MAX_TAB_SIZE: u8 = 8;
pub const SYSTEM_FONT_FAMILY: &str = ".SystemUIFont";
/// The bundled interface font, the default for controls and assistant
/// messages.
pub const INTERFACE_FONT_FAMILY: &str = "IBM Plex Sans";
/// The bundled monospace font, the default for SQL and logs.
pub const MONOSPACE_FONT_FAMILY: &str = "JetBrains Mono";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    pub ui_scale: f32,
    pub ui_font_family: String,
    pub editor_font_family: String,
    pub editor_font_size: f32,
    pub editor_line_height: f32,
    /// Spaces for each indent level in the SQL editor.
    pub editor_tab_size: u8,
    /// Animate short horizontal caret movements in the SQL editor.
    pub editor_smooth_caret: bool,
    /// Ease discrete mouse-wheel steps in the SQL editor.
    pub editor_smooth_scrolling: bool,
    /// Suggest names from the cached metadata when Tab follows a name in
    /// the SQL editor.
    pub editor_autocomplete: bool,
    /// Fade content when switching tabs and panels.
    pub smooth_transitions: bool,
    pub logs_font_family: String,
    pub logs_font_size: f32,
    pub logs_line_height: f32,
    pub assistant_font_family: String,
    pub assistant_font_size: f32,
    pub assistant_line_height: f32,
    /// Animate short horizontal caret movements in the assistant composer.
    pub assistant_smooth_caret: bool,
    /// Ease content-driven assistant composer height changes.
    pub assistant_smooth_resize: bool,
    /// Hold reply text until its message completes, then fade it in.
    pub assistant_buffer_replies: bool,
    pub assistant: AssistantSettings,
    pub export: crate::export::Settings,
    /// Maximum retained replay spool size. Zero disables replay retention.
    pub export_replay_limit_mib: u32,
    /// Format of ⌘C and the Copy commands of the results menu.
    pub copy_format: CopyFormat,
    /// Collapse the Results and Logs panel to its header in every tab.
    pub results_minimized: bool,
    /// Check for new versions once a day. Only release builds check.
    pub check_for_updates: bool,
    /// The release channel that updates follow. `None` follows the channel
    /// of the running build.
    pub update_channel: Option<crate::update::Channel>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: SYSTEM_THEME.into(),
            ui_scale: 1.,
            ui_font_family: INTERFACE_FONT_FAMILY.into(),
            editor_font_family: MONOSPACE_FONT_FAMILY.into(),
            editor_font_size: 13.,
            editor_line_height: 1.2,
            editor_tab_size: 2,
            editor_smooth_caret: false,
            editor_smooth_scrolling: false,
            editor_autocomplete: true,
            smooth_transitions: false,
            logs_font_family: MONOSPACE_FONT_FAMILY.into(),
            logs_font_size: 13.,
            logs_line_height: 1.2,
            assistant_font_family: INTERFACE_FONT_FAMILY.into(),
            assistant_font_size: 14.,
            assistant_line_height: 1.6,
            assistant_smooth_caret: false,
            assistant_smooth_resize: false,
            assistant_buffer_replies: false,
            assistant: AssistantSettings::default(),
            export: crate::export::Settings::default(),
            export_replay_limit_mib: 2048,
            copy_format: CopyFormat::default(),
            results_minimized: false,
            check_for_updates: true,
            update_channel: None,
        }
    }
}

/// The text that ⌘C and the Copy commands of the results menu put on the
/// clipboard. **Copy as** chooses a format for one copy.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyFormat {
    /// An aligned Markdown table with column names.
    #[default]
    Markdown,
    /// Tab-separated values without column names. One cell copies its value.
    PlainText,
    Csv,
    Tsv,
    Json,
}

impl CopyFormat {
    pub const ALL: [Self; 5] = [
        Self::Markdown,
        Self::PlainText,
        Self::Csv,
        Self::Tsv,
        Self::Json,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Markdown => "Markdown",
            Self::PlainText => "Plain text",
            Self::Csv => "CSV",
            Self::Tsv => "TSV",
            Self::Json => "JSON",
        }
    }
}

impl Settings {
    pub fn sanitize(&mut self) {
        self.export_replay_limit_mib = self.export_replay_limit_mib.min(1024 * 1024);
        self.theme = self.theme.trim().into();
        if self.theme.is_empty() {
            self.theme = Self::default().theme;
        }

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

        if self.editor_tab_size == 0 {
            self.editor_tab_size = Self::default().editor_tab_size;
        }
        self.editor_tab_size = self.editor_tab_size.clamp(MIN_TAB_SIZE, MAX_TAB_SIZE);

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

    /// The layout for SQL that the assistant writes and Qrow formats.
    pub fn sql_style(&self) -> SqlStyle {
        SqlStyle {
            keyword_case: self.assistant.sql_keyword_case,
            indent_spaces: self.editor_tab_size,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantExecutionMode {
    #[default]
    AskBeforeRunning,
    RunAutomatically,
}

/// The command-line program that runs the assistant. Each conversation
/// belongs to the harness that created it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantHarness {
    #[default]
    Codex,
    Claude,
}

impl AssistantHarness {
    pub const ALL: [Self; 2] = [Self::Codex, Self::Claude];

    /// The name of the program, for interface text.
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
        }
    }
}

/// The model choices of one harness. Each harness offers its own models.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(default)]
pub struct AssistantModelChoice {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub service_tier: Option<String>,
}

impl AssistantModelChoice {
    fn sanitize(&mut self) {
        sanitize_optional_string(&mut self.model);
        sanitize_optional_string(&mut self.reasoning_effort);
        sanitize_optional_string(&mut self.service_tier);
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(default)]
pub struct AssistantSettings {
    pub enabled: bool,
    pub data_sharing_notice_version: u32,
    pub default_execution_mode: AssistantExecutionMode,
    /// The harness of new conversations, and the one that Qrow starts.
    pub harness: AssistantHarness,
    pub codex_executable: Option<String>,
    pub claude_executable: Option<String>,
    pub panel_width: f32,
    /// The Codex choices keep the field names of earlier workspaces.
    #[serde(flatten)]
    pub codex: AssistantModelChoice,
    pub claude: AssistantModelChoice,
    pub sql_keyword_case: KeywordCase,
    /// Lets the harness search the web. Codex uses its cached index, and
    /// Claude Code uses its search tool. Neither can open other URLs.
    pub web_search: bool,
}

impl Default for AssistantSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            data_sharing_notice_version: 0,
            default_execution_mode: AssistantExecutionMode::AskBeforeRunning,
            harness: AssistantHarness::default(),
            codex_executable: None,
            claude_executable: None,
            panel_width: DEFAULT_ASSISTANT_PANEL_WIDTH,
            codex: AssistantModelChoice::default(),
            claude: AssistantModelChoice::default(),
            sql_keyword_case: KeywordCase::default(),
            web_search: false,
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
        sanitize_optional_string(&mut self.claude_executable);
        self.codex.sanitize();
        self.claude.sanitize();
    }

    /// The model choices of the selected harness.
    pub fn choice(&self) -> &AssistantModelChoice {
        match self.harness {
            AssistantHarness::Codex => &self.codex,
            AssistantHarness::Claude => &self.claude,
        }
    }

    pub fn choice_mut(&mut self) -> &mut AssistantModelChoice {
        match self.harness {
            AssistantHarness::Codex => &mut self.codex,
            AssistantHarness::Claude => &mut self.claude,
        }
    }

    /// The configured executable of the selected harness.
    pub fn executable(&self) -> Option<&str> {
        match self.harness {
            AssistantHarness::Codex => self.codex_executable.as_deref(),
            AssistantHarness::Claude => self.claude_executable.as_deref(),
        }
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
    #[serde(default)]
    pub database_type: DatabaseType,
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub database: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub trino_schema: String,
    pub parameters: BTreeMap<String, String>,
    #[serde(default)]
    pub transfer: transfer::Transfer,
    #[serde(default)]
    pub lifecycle: ConnectionLifecycle,
    #[serde(default)]
    pub catalog: CatalogSettings,
    /// The column-read scope of this connection, also for a shared catalog.
    #[serde(default)]
    pub catalog_column_reads: CatalogColumnReads,
    /// The shared catalog that the connection uses, or `None` for a catalog
    /// of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_catalog: Option<Uuid>,
    /// Encrypts the transport with TLS. Profiles before version 6 use plain TCP.
    #[serde(default)]
    pub tls: bool,
    /// The Postgres TLS policy. None preserves the earlier TLS checkbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgres_ssl_mode: Option<PostgresSslMode>,
    #[serde(default)]
    pub authentication: Authentication,
    /// Facts about the connection that the user writes for the assistant.
    /// Qrow adds them to the workspace context of a conversation.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub assistant_notes: String,
    /// The dbt project that describes the tables of the connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dbt: Option<DbtProject>,
}

/// The most schema mapping rules of a dbt project.
pub const MAX_SCHEMA_RULES: usize = 50;

/// The dbt project of a connection. Qrow reads its `manifest.json`. It does
/// not run dbt or read the other files of the project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DbtProject {
    /// The absolute path of the manifest, usually
    /// `<project>/target/manifest.json`.
    pub manifest: String,
    #[serde(default)]
    pub refresh: DbtRefresh,
    /// The rules that change a dbt schema into a catalog schema. The first
    /// rule that matches applies. Without a match, the schema stays.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schema_mapping: Vec<SchemaRule>,
}

/// When Qrow reads the manifest of a dbt project again.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DbtRefresh {
    /// When the manifest changes.
    #[default]
    Automatic,
    /// Only on Refresh, and at launch when the saved index does not agree
    /// with the manifest.
    Manual,
}

/// A rule of the schema mapping of a dbt project. Letter case does not
/// matter, like in Spark.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SchemaRule {
    pub kind: SchemaRuleKind,
    pub from: String,
    #[serde(default)]
    pub to: String,
}

impl SchemaRule {
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.from.trim().is_empty()
                && (self.kind == SchemaRuleKind::Prefix || !self.to.trim().is_empty()),
            "Each schema mapping rule needs a dbt schema and a database schema."
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaRuleKind {
    /// Replaces the start `from` of a schema with `to`.
    Prefix,
    /// Replaces the schema `from` with `to`.
    Exact,
}

impl DbtProject {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            std::path::Path::new(&self.manifest).is_absolute(),
            "Choose the manifest.json file of the dbt project."
        );
        anyhow::ensure!(
            self.schema_mapping.len() <= MAX_SCHEMA_RULES,
            "Schema mapping can have {MAX_SCHEMA_RULES} rules or fewer."
        );
        for rule in &self.schema_mapping {
            rule.validate()?;
        }
        Ok(())
    }

    /// The catalog schema of the dbt schema `schema`.
    pub fn map_schema<'a>(&self, schema: &'a str) -> std::borrow::Cow<'a, str> {
        for rule in &self.schema_mapping {
            match rule.kind {
                SchemaRuleKind::Exact if schema.eq_ignore_ascii_case(&rule.from) => {
                    return rule.to.clone().into();
                }
                SchemaRuleKind::Prefix
                    if schema
                        .get(..rule.from.len())
                        .is_some_and(|start| start.eq_ignore_ascii_case(&rule.from)) =>
                {
                    return format!("{}{}", rule.to, &schema[rule.from.len()..]).into();
                }
                _ => {}
            }
        }
        schema.into()
    }
}

/// Read schema mapping rules from text with one rule on each line, like
/// `dbt_dev_* = *` for a prefix or `analytics = prod_analytics` for one
/// schema. Empty lines do not count.
pub fn parse_schema_rules(text: &str) -> anyhow::Result<Vec<SchemaRule>> {
    let mut rules = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let invalid = || {
            anyhow::anyhow!(
                "Schema mapping line {} must be like dbt_dev_* = * or analytics = prod.",
                number + 1
            )
        };
        let (from, to) = line.split_once('=').ok_or_else(invalid)?;
        let (from, to) = (from.trim(), to.trim());
        let rule = match (from.strip_suffix('*'), to.strip_suffix('*')) {
            (Some(from), Some(to)) => SchemaRule {
                kind: SchemaRuleKind::Prefix,
                from: from.to_owned(),
                to: to.to_owned(),
            },
            (None, None) => SchemaRule {
                kind: SchemaRuleKind::Exact,
                from: from.to_owned(),
                to: to.to_owned(),
            },
            _ => return Err(invalid()),
        };
        let name = |text: &str| {
            text.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        };
        if rule.from.is_empty()
            || (rule.kind == SchemaRuleKind::Exact && rule.to.is_empty())
            || !name(&rule.from)
            || !name(&rule.to)
        {
            return Err(invalid());
        }
        rules.push(rule);
    }
    Ok(rules)
}

/// The text of schema mapping rules, one rule on each line.
pub fn format_schema_rules(rules: &[SchemaRule]) -> String {
    rules
        .iter()
        .map(|rule| match rule.kind {
            SchemaRuleKind::Prefix => format!("{}* = {}*", rule.from, rule.to),
            SchemaRuleKind::Exact => format!("{} = {}", rule.from, rule.to),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The encryption and server identity checks of a Postgres connection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostgresSslMode {
    Disable,
    Require,
    VerifyFull,
}

/// The SQL server and dialect of a connection.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseType {
    #[default]
    Kyuubi,
    Postgres,
    Trino,
}

impl DatabaseType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Kyuubi => "Spark (HiveServer2)",
            Self::Postgres => "Postgres",
            Self::Trino => "Trino",
        }
    }

    pub fn connector_name(self) -> &'static str {
        match self {
            Self::Kyuubi => "spark_kyuubi",
            Self::Postgres => "postgres",
            Self::Trino => "trino",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Self::Kyuubi => 10009,
            Self::Postgres => 5432,
            Self::Trino => 8080,
        }
    }

    pub fn quote_identifier(self, name: &str) -> String {
        let quote = match self {
            Self::Kyuubi => '`',
            Self::Postgres | Self::Trino => '"',
        };
        format!(
            "{quote}{}{quote}",
            name.replace(quote, &format!("{quote}{quote}"))
        )
    }
}

/// How a refresh reads columns. Table reads reduce the result held by the driver.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogColumnReads {
    #[default]
    Table,
    Schema,
}

/// How a connection proves the identity of its database user.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Authentication {
    /// The password of the database user, from macOS Keychain.
    #[default]
    Password,
    /// An access token of a reusable sign-in, in the SASL password field.
    Oidc { sign_in: Uuid },
    /// Browser authentication delegated to the Trino coordinator.
    TrinoExternal,
}

impl Authentication {
    pub fn sign_in(self) -> Option<Uuid> {
        match self {
            Self::Password | Self::TrinoExternal => None,
            Self::Oidc { sign_in } => Some(sign_in),
        }
    }
}

/// One schema catalog for the connections that read the same metastore,
/// for example through different users or compute clusters.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SharedCatalog {
    pub id: Uuid,
    pub name: String,
    /// The settings that decide what a refresh reads.
    pub settings: CatalogSettings,
    /// The member that automatic refreshes use while it has a live session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred: Option<Uuid>,
}

/// The maximum length of a shared catalog name, in characters.
pub const MAX_SHARED_CATALOG_NAME: usize = 60;

impl SharedCatalog {
    /// Repair a shared catalog that an edited file made invalid. A shared
    /// catalog always browses schemas: each member can turn browsing off.
    fn sanitize(&mut self) {
        if !self.settings.browses() {
            self.settings.refresh = CatalogRefresh::Manual;
        }
        if self.settings.validate().is_err() {
            self.settings = CatalogSettings {
                refresh: self.settings.refresh,
                ..CatalogSettings::default()
            };
        }
        if self.name.trim().is_empty() {
            self.name = "Shared metadata".into();
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.name.trim().is_empty(),
            "Give the shared metadata a name."
        );
        anyhow::ensure!(
            self.name.chars().count() <= MAX_SHARED_CATALOG_NAME,
            "Shared metadata name must be {MAX_SHARED_CATALOG_NAME} characters or fewer."
        );
        self.settings.validate()
    }
}

/// The catalog settings that Connection Settings shows for `profile`. A
/// member of a shared catalog shows the shared settings, also while it does
/// not browse schemas.
pub fn shown_catalog(profile: &Profile, shared: &[SharedCatalog]) -> CatalogSettings {
    match profile
        .shared_catalog
        .and_then(|id| shared.iter().find(|catalog| catalog.id == id))
    {
        Some(shared) => CatalogSettings {
            refresh: if profile.catalog.browses() {
                shared.settings.refresh
            } else {
                CatalogRefresh::Disabled
            },
            ..shared.settings.clone()
        },
        None => profile.catalog.clone(),
    }
}

/// The catalog settings that apply to `profile`. A member of a shared
/// catalog uses the shared settings. A connection with schema browsing off
/// stays off.
pub fn effective_catalog(profile: &Profile, shared: &[SharedCatalog]) -> CatalogSettings {
    match profile
        .shared_catalog
        .and_then(|id| shared.iter().find(|catalog| catalog.id == id))
    {
        Some(catalog) if profile.catalog.browses() => catalog.settings.clone(),
        _ => profile.catalog.clone(),
    }
}

/// The maximum number of include or exclude patterns of one connection.
pub const MAX_SCHEMA_PATTERNS: usize = 100;
/// The maximum length of one schema pattern, in characters.
pub const MAX_SCHEMA_PATTERN: usize = 256;

/// The valid automatic refresh periods, in minutes (5 minutes to 7 days).
pub const CATALOG_REFRESH_MINUTES: std::ops::RangeInclusive<u32> = 5..=10_080;
/// The valid refresh timeouts, in minutes (1 minute to 1 day).
pub const CATALOG_TIMEOUT_MINUTES: std::ops::RangeInclusive<u32> = 1..=1_440;

/// Whether Qrow reads the catalog of a connection, and when.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CatalogRefresh {
    /// Qrow does not read or show the schemas of the connection. New
    /// connections use it.
    #[default]
    Disabled,
    /// Only an explicit Refresh, or the expansion of an unread row while
    /// connected, reads the catalog.
    Manual,
    /// As `Manual`, and read the connection again after each refresh
    /// period, but only while a tab of the connection has a live session.
    WhileConnected,
}

impl CatalogRefresh {
    /// Settings saved before this choice existed keep their behavior: the
    /// schema tree with refreshes on request.
    fn saved_before_choice() -> Self {
        Self::Manual
    }
}

/// Which schemas the schema tree and the assistant show for a connection,
/// and when Qrow reads them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CatalogSettings {
    /// Glob patterns of the schemas to show. Empty shows all schemas.
    pub include: Vec<String>,
    /// Glob patterns of the schemas to hide, also when an include matches.
    pub exclude: Vec<String>,
    #[serde(default = "CatalogRefresh::saved_before_choice")]
    pub refresh: CatalogRefresh,
    /// The period of [`CatalogRefresh::WhileConnected`]. The other choices
    /// keep it for a later change back.
    pub refresh_minutes: u32,
    /// The longest time of one refresh, manual or automatic.
    pub timeout_minutes: u32,
}

impl Default for CatalogSettings {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            exclude: Vec::new(),
            refresh: CatalogRefresh::default(),
            refresh_minutes: 60,
            timeout_minutes: 30,
        }
    }
}

impl CatalogSettings {
    /// Whether Qrow reads and shows the schemas of the connection.
    pub fn browses(&self) -> bool {
        self.refresh != CatalogRefresh::Disabled
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            CATALOG_REFRESH_MINUTES.contains(&self.refresh_minutes),
            "Refresh period must be between {} and {} minutes.",
            CATALOG_REFRESH_MINUTES.start(),
            CATALOG_REFRESH_MINUTES.end()
        );
        anyhow::ensure!(
            CATALOG_TIMEOUT_MINUTES.contains(&self.timeout_minutes),
            "Refresh timeout must be between {} and {} minutes.",
            CATALOG_TIMEOUT_MINUTES.start(),
            CATALOG_TIMEOUT_MINUTES.end()
        );
        for patterns in [&self.include, &self.exclude] {
            anyhow::ensure!(
                patterns.len() <= MAX_SCHEMA_PATTERNS,
                "Enter {MAX_SCHEMA_PATTERNS} schema patterns or fewer."
            );
            for pattern in patterns {
                anyhow::ensure!(
                    !pattern.trim().is_empty(),
                    "Schema patterns cannot be empty."
                );
                anyhow::ensure!(
                    pattern.chars().count() <= MAX_SCHEMA_PATTERN,
                    "Each schema pattern must be {MAX_SCHEMA_PATTERN} characters or fewer."
                );
            }
        }
        Ok(())
    }

    /// Return whether the schema tree shows `schema`. `*` matches any text and
    /// `?` matches one character. Letter case does not matter.
    pub fn shows(&self, schema: &str) -> bool {
        (self.include.is_empty() || self.include.iter().any(|p| glob_match(p, schema)))
            && !self.exclude.iter().any(|p| glob_match(p, schema))
    }
}

pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.trim().to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();
    // Iterative matching with one backtrack point keeps the cost linear in practice.
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&c) if c == '?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((star_p, star_t)) => {
                    p = star_p + 1;
                    t = star_t + 1;
                    star = Some((star_p, star_t + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConnectionLifecycle {
    pub idle_seconds: u64,
    /// Zero disables keep-alive and enables idle disconnection.
    pub keep_alive_seconds: u64,
    pub keep_alive_sql: String,
    /// The longest wait for one answer from Kyuubi. The first statement of
    /// a session can wait while Kyuubi starts an engine.
    pub response_timeout_seconds: u64,
}

/// The valid response timeouts, in seconds (10 seconds to 1 hour).
pub const RESPONSE_TIMEOUT_SECONDS: std::ops::RangeInclusive<u64> = 10..=3_600;

impl Default for ConnectionLifecycle {
    fn default() -> Self {
        Self {
            idle_seconds: 900,
            keep_alive_seconds: 0,
            keep_alive_sql: "SELECT 1".into(),
            response_timeout_seconds: 300,
        }
    }
}

impl ConnectionLifecycle {
    pub fn response_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.response_timeout_seconds)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=604_800).contains(&self.idle_seconds),
            "Idle timeout must be between 1 and 604800 seconds."
        );
        anyhow::ensure!(
            self.keep_alive_seconds <= 604_800,
            "Keep-alive interval must be between 0 and 604800 seconds."
        );
        anyhow::ensure!(
            RESPONSE_TIMEOUT_SECONDS.contains(&self.response_timeout_seconds),
            "Response timeout must be between {} and {} seconds.",
            RESPONSE_TIMEOUT_SECONDS.start(),
            RESPONSE_TIMEOUT_SECONDS.end()
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
            database_type: DatabaseType::default(),
            id: Uuid::new_v4(),
            name: "Spark".into(),
            host: String::new(),
            port: 10009,
            username: String::new(),
            database: "avia".into(),
            trino_schema: String::new(),
            parameters: BTreeMap::new(),
            transfer: transfer::Transfer::default(),
            lifecycle: ConnectionLifecycle::default(),
            catalog: CatalogSettings::default(),
            catalog_column_reads: CatalogColumnReads::default(),
            shared_catalog: None,
            tls: false,
            postgres_ssl_mode: None,
            authentication: Authentication::Password,
            assistant_notes: String::new(),
            dbt: None,
        }
    }
}

impl Profile {
    /// The schema of names without a schema in the SQL of the connection.
    pub fn default_schema(&self) -> &str {
        match self.database_type {
            DatabaseType::Postgres => "public",
            DatabaseType::Trino => &self.trino_schema,
            DatabaseType::Kyuubi => &self.database,
        }
    }

    /// Resolve saved profiles that predate explicit Postgres TLS modes.
    pub fn postgres_ssl_mode(&self) -> PostgresSslMode {
        self.postgres_ssl_mode.unwrap_or(if self.tls {
            PostgresSslMode::VerifyFull
        } else {
            PostgresSslMode::Disable
        })
    }

    /// Return whether two profiles open the same authenticated session.
    ///
    /// The display name and idle policy can change while a session remains
    /// usable. The profile id stays part of the identity because it selects
    /// the stored password.
    pub fn connection_identity_eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.database_type == other.database_type
            && self.host == other.host
            && self.port == other.port
            && self.username == other.username
            && self.database == other.database
            && (self.database_type != DatabaseType::Trino
                || self.trino_schema == other.trino_schema)
            && self.parameters == other.parameters
            && if self.database_type == DatabaseType::Postgres {
                self.postgres_ssl_mode() == other.postgres_ssl_mode()
            } else {
                self.tls == other.tls
            }
            && self.authentication == other.authentication
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.name.trim().is_empty(), "Give this connection a name.");
        self.transfer.validate()?;
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
        anyhow::ensure!(
            self.assistant_notes.len() <= MAX_ASSISTANT_NOTES_BYTES,
            "Assistant notes must be {} KB or less.",
            MAX_ASSISTANT_NOTES_BYTES / 1024
        );
        anyhow::ensure!(
            self.database_type != DatabaseType::Postgres
                || self.authentication == Authentication::Password,
            "Postgres connections use password authentication."
        );
        anyhow::ensure!(
            self.database_type != DatabaseType::Trino
                || self.authentication == Authentication::Password
                || self.tls,
            "Trino sign-ins require TLS."
        );
        anyhow::ensure!(
            !matches!(self.authentication, Authentication::TrinoExternal)
                || self.database_type == DatabaseType::Trino,
            "Browser sign-in through Trino requires a Trino connection."
        );
        self.lifecycle.validate()?;
        if let Some(dbt) = &self.dbt {
            dbt.validate()?;
        }
        self.catalog.validate()
    }

    /// Repair a profile that an edited file made invalid: notes above the
    /// limit lose their end, and the schema mapping loses its invalid rules.
    fn sanitize(&mut self) {
        if let Some(dbt) = &mut self.dbt {
            dbt.schema_mapping.retain(|rule| rule.validate().is_ok());
            dbt.schema_mapping.truncate(MAX_SCHEMA_RULES);
        }
        if self.assistant_notes.len() > MAX_ASSISTANT_NOTES_BYTES {
            let end = self
                .assistant_notes
                .floor_char_boundary(MAX_ASSISTANT_NOTES_BYTES);
            self.assistant_notes.truncate(end);
        }
    }
}

/// The protocol used by a reusable sign-in.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInProvider {
    #[default]
    Oidc,
    /// Legacy workspace records, migrated to connection authentication.
    TrinoExternal,
}
impl SignInProvider {
    fn is_oidc(&self) -> bool {
        *self == Self::Oidc
    }
}

/// A reusable sign-in. Each connection keeps its database username. Direct
/// OIDC tokens use Keychain. Tokens are never stored in the workspace.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SignIn {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub provider: SignInProvider,
    /// The issuer identifier of the provider, for example a Keycloak realm URL.
    #[serde(default)]
    pub issuer: String,
    /// The public client registered for Qrow. Qrow uses no client secret.
    #[serde(default)]
    pub client_id: String,
    /// Scopes in addition to [`BASE_SCOPES`].
    #[serde(default)]
    pub scopes: Vec<String>,
    /// An optional RFC 8707 resource indicator for the access tokens.
    #[serde(default)]
    pub resource: Option<String>,
    /// The loopback ports of the callback, in the order to try them. An
    /// empty list selects an available port.
    #[serde(default, alias = "callback_port", deserialize_with = "callback_ports")]
    pub callback_ports: Vec<u16>,
    /// The signed-in account. `None` until the first sign-in.
    #[serde(default)]
    pub identity: Option<Identity>,
}

/// An account at a provider. The issuer and the subject identify it. The
/// name and email are for display only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Identity {
    pub issuer: String,
    pub subject: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

impl Identity {
    /// The name, the email, or the subject, in that order.
    pub fn display(&self) -> &str {
        self.email
            .as_deref()
            .or(self.name.as_deref())
            .unwrap_or(&self.subject)
    }
}

impl Default for SignIn {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            provider: SignInProvider::Oidc,
            issuer: String::new(),
            client_id: String::new(),
            scopes: vec![],
            resource: None,
            callback_ports: vec![],
            identity: None,
        }
    }
}

impl SignIn {
    /// Return whether two configurations need the same tokens. A change to
    /// any of these fields makes the stored tokens unusable.
    pub fn token_requirements_eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.provider == other.provider
            && self.issuer == other.issuer
            && self.client_id == other.client_id
            && extra_scopes(&self.scopes) == extra_scopes(&other.scopes)
            && self.resource == other.resource
    }

    /// The scopes of an authorization request: [`BASE_SCOPES`], then the
    /// other scopes of the sign-in.
    pub fn requested_scopes(&self) -> Vec<&str> {
        let mut scopes = BASE_SCOPES.to_vec();
        scopes.extend(extra_scopes(&self.scopes));
        scopes
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.name.trim().is_empty(), "Give this sign-in a name.");
        anyhow::ensure!(
            self.name.chars().count() <= MAX_SIGN_IN_NAME,
            "Sign-in name must be {MAX_SIGN_IN_NAME} characters or fewer."
        );
        if self.provider == SignInProvider::Oidc {
            let issuer = url::Url::parse(&self.issuer)
                .map_err(|_| anyhow::anyhow!("Enter the issuer as an HTTPS URL."))?;
            anyhow::ensure!(
                issuer.scheme() == "https"
                    && issuer.host_str().is_some()
                    && issuer.query().is_none()
                    && issuer.fragment().is_none()
                    && issuer.username().is_empty()
                    && issuer.password().is_none(),
                "Enter the issuer as an HTTPS URL without a query or fragment."
            );
            anyhow::ensure!(!self.client_id.trim().is_empty(), "Enter a client ID.");
            anyhow::ensure!(
                self.client_id.chars().all(|c| (' '..='~').contains(&c)),
                "The client ID can contain only printable ASCII characters."
            );
            for scope in &self.scopes {
                anyhow::ensure!(
                    !scope.is_empty()
                        && scope.chars().all(|c| c == '!'
                            || ('#'..='[').contains(&c)
                            || (']'..='~').contains(&c)),
                    "Scope \"{scope}\" contains a character that a scope cannot contain."
                );
            }
            if let Some(resource) = &self.resource {
                let parsed = url::Url::parse(resource)
                    .map_err(|_| anyhow::anyhow!("Enter the resource as an absolute URI."))?;
                anyhow::ensure!(
                    parsed.fragment().is_none(),
                    "The resource cannot contain a fragment."
                );
            }
        }
        anyhow::ensure!(
            self.callback_ports.len() <= MAX_CALLBACK_PORTS,
            "Enter {MAX_CALLBACK_PORTS} callback ports or fewer."
        );
        anyhow::ensure!(
            !self.callback_ports.contains(&0),
            "Callback ports must be numbers from 1 to 65535."
        );
        Ok(())
    }
}

/// Reads the callback ports of a sign-in. Workspaces before this list kept
/// one port, where zero selected an available port.
fn callback_ports<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<u16>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Ports {
        One(u16),
        Many(Vec<u16>),
    }
    Ok(match Ports::deserialize(deserializer)? {
        Ports::One(0) => vec![],
        Ports::One(port) => vec![port],
        Ports::Many(ports) => ports,
    })
}

/// The format version of shared sign-in settings.
pub const SHARED_SIGN_IN_VERSION: u32 = 2;
/// The longest text that Qrow reads as shared sign-in settings.
const MAX_SHARED_SIGN_IN_BYTES: usize = 64 * 1024;

/// The settings of a sign-in that one user gives to another as text. It has
/// no identifier, account, or tokens.
#[derive(Deserialize, Serialize)]
struct SharedSignIn {
    /// Marks the text as sign-in settings and gives the format version.
    qrow_sign_in: u32,
    name: String,
    #[serde(default, skip_serializing_if = "SignInProvider::is_oidc")]
    provider: SignInProvider,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    issuer: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    client_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    callback_ports: Vec<u16>,
}

impl SharedSignIn {
    /// The shared settings of `sign_in`.
    fn of(sign_in: &SignIn) -> Self {
        Self {
            qrow_sign_in: if sign_in.provider == SignInProvider::Oidc {
                1
            } else {
                SHARED_SIGN_IN_VERSION
            },
            name: sign_in.name.clone(),
            provider: sign_in.provider,
            issuer: sign_in.issuer.clone(),
            client_id: sign_in.client_id.clone(),
            scopes: extra_scopes(&sign_in.scopes)
                .into_iter()
                .map(str::to_owned)
                .collect(),
            resource: sign_in.resource.clone(),
            callback_ports: sign_in.callback_ports.clone(),
        }
    }

    /// A new sign-in with no account from the shared settings in `value`.
    fn read(value: serde_json::Value) -> anyhow::Result<SignIn> {
        let version = value
            .get("qrow_sign_in")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("The sign-in settings have no format version."))?;
        anyhow::ensure!(
            version <= u64::from(SHARED_SIGN_IN_VERSION),
            "These sign-in settings come from a newer version of Qrow. Update Qrow, then try again."
        );
        let shared: Self = serde_json::from_value(value)
            .map_err(|error| anyhow::anyhow!("The sign-in settings are not complete: {error}."))?;
        let sign_in = SignIn {
            name: shared.name.trim().to_owned(),
            provider: shared.provider,
            issuer: shared.issuer.trim().to_owned(),
            client_id: shared.client_id.trim().to_owned(),
            scopes: shared.scopes,
            resource: shared
                .resource
                .filter(|resource| !resource.trim().is_empty()),
            callback_ports: shared.callback_ports,
            ..SignIn::default()
        };
        sign_in.validate()?;
        Ok(sign_in)
    }
}

impl SignIn {
    /// The settings of this sign-in as text that another user can paste.
    pub fn to_shared_text(&self) -> String {
        serde_json::to_string_pretty(&SharedSignIn::of(self)).expect("sign-in settings serialize")
    }

    /// Reads settings from [`Self::to_shared_text`] into a new sign-in with
    /// no account. Text around the settings, like the rest of a chat
    /// message, and typographic quotes are accepted.
    pub fn from_shared_text(text: &str) -> anyhow::Result<Self> {
        const NOT_A_SIGN_IN: &str = "The clipboard does not contain sign-in settings. Use Copy settings on a sign-in, then try again.";
        anyhow::ensure!(text.len() <= MAX_SHARED_SIGN_IN_BYTES, NOT_A_SIGN_IN);
        let value = shared_object(text, "qrow_sign_in")
            .filter(|value| {
                value
                    .get("qrow_sign_in")
                    .is_some_and(serde_json::Value::is_u64)
            })
            .ok_or_else(|| anyhow::anyhow!(NOT_A_SIGN_IN))?;
        SharedSignIn::read(value)
    }

    /// Return whether `other` has the same provider settings as this
    /// sign-in, so that the two can share tokens. Names and callback ports
    /// do not matter.
    pub fn same_provider_settings(&self, other: &Self) -> bool {
        self.token_requirements_eq(&Self {
            id: self.id,
            ..other.clone()
        })
    }
}

/// The first JSON object in `text` that has the key `marker`, also after
/// typographic quotes change to straight quotes.
fn shared_object(text: &str, marker: &str) -> Option<serde_json::Value> {
    shared_object_in(text, marker)
        .or_else(|| shared_object_in(&text.replace(['\u{201C}', '\u{201D}'], "\""), marker))
}

/// The first JSON object in `text` that has the key `marker`. Other text,
/// also text with braces, can come before and after it.
fn shared_object_in(text: &str, marker: &str) -> Option<serde_json::Value> {
    // The object starts at one of the nearest braces before a marker. A few
    // tries for each marker keep the work linear in the length of the text.
    const STARTS_PER_MARKER: usize = 4;
    let quoted = format!("\"{marker}\"");
    text.match_indices(&quoted).find_map(|(at, _)| {
        text[..at]
            .rmatch_indices('{')
            .take(STARTS_PER_MARKER)
            .find_map(|(start, _)| {
                serde_json::Deserializer::from_str(&text[start..])
                    .into_iter::<serde_json::Value>()
                    .next()?
                    .ok()
                    .filter(|value| value.get(marker).is_some())
            })
    })
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
    /// The harness generated the title. Earlier workspaces call it `codex`.
    #[serde(rename = "codex", alias = "generated")]
    Generated,
    User,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct AssistantConversation {
    pub thread_id: String,
    /// The harness that keeps the conversation. Earlier workspaces have only
    /// Codex conversations.
    #[serde(default)]
    pub harness: AssistantHarness,
    pub title: String,
    pub title_source: AssistantTitleSource,
    /// A tab opened by New Conversation follows its conversation title.
    #[serde(default)]
    pub title_follows_conversation: bool,
    pub last_activity: u64,
    pub execution_mode: AssistantExecutionMode,
    /// The query tab of the conversation. The tab supplies the connection.
    #[serde(default)]
    pub tab_id: Option<Uuid>,
    /// The connection for the next tab of a conversation whose tab closed.
    #[serde(default)]
    pub detached_profile: Option<Uuid>,
    /// The connection notes that the conversation has, so a message sends
    /// them again only after a change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_notes: Option<SentNotes>,
}

/// The connection notes that Qrow last sent in a conversation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SentNotes {
    /// The connection of the conversation tab when Qrow sent the notes.
    pub connection: Option<Uuid>,
    /// The SHA-256 digest of the notes, in lowercase hexadecimal.
    pub digest: String,
}

impl AssistantConversation {
    pub fn new(thread_id: impl Into<String>, execution_mode: AssistantExecutionMode) -> Self {
        Self {
            thread_id: thread_id.into(),
            harness: AssistantHarness::default(),
            title: "New conversation".into(),
            title_source: AssistantTitleSource::Temporary,
            title_follows_conversation: false,
            last_activity: 0,
            execution_mode,
            tab_id: None,
            detached_profile: None,
            sent_notes: None,
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
    /// The conversation that a version 3 workspace showed. Loading moves it
    /// to the active tab. Qrow does not save it.
    #[serde(rename = "selected_thread", skip_serializing)]
    legacy_selected_thread: Option<String>,
}

impl AssistantWorkspace {
    pub fn sanitize(&mut self) {
        let mut thread_ids = BTreeSet::new();
        self.conversations.retain_mut(|conversation| {
            conversation.sanitize();
            !conversation.thread_id.is_empty() && thread_ids.insert(conversation.thread_id.clone())
        });
    }

    pub fn conversation(&self, thread_id: &str) -> Option<&AssistantConversation> {
        self.conversations
            .iter()
            .find(|conversation| conversation.thread_id == thread_id)
    }

    pub fn conversation_mut(&mut self, thread_id: &str) -> Option<&mut AssistantConversation> {
        self.conversations
            .iter_mut()
            .find(|conversation| conversation.thread_id == thread_id)
    }

    /// The conversation that owns a query tab.
    pub fn conversation_for_tab(&self, tab_id: Uuid) -> Option<&AssistantConversation> {
        self.conversations
            .iter()
            .find(|conversation| conversation.tab_id == Some(tab_id))
    }

    /// Detach the conversation of a closed tab. It keeps the connection for
    /// its next tab.
    pub fn detach_tab(&mut self, tab_id: Uuid, profile: Option<Uuid>) {
        for conversation in &mut self.conversations {
            if conversation.tab_id == Some(tab_id) {
                conversation.tab_id = None;
                conversation.detached_profile = profile;
            }
        }
    }

    /// Give each tab at most one conversation, and detach conversations whose
    /// tab is missing. The most recent conversation keeps a shared tab. A
    /// detached conversation keeps the connection of its last tab.
    fn link_tabs(&mut self, tabs: &[SavedTab], profiles: &[Profile]) {
        let mut order: Vec<usize> = (0..self.conversations.len()).collect();
        order.sort_by_key(|&index| std::cmp::Reverse(self.conversations[index].last_activity));
        let mut claimed = BTreeSet::new();
        for index in order {
            let conversation = &mut self.conversations[index];
            let Some(tab_id) = conversation.tab_id else {
                continue;
            };
            let tab = tabs.iter().find(|tab| tab.id == tab_id);
            if tab.is_some() && claimed.insert(tab_id) {
                conversation.detached_profile = None;
                continue;
            }
            conversation.tab_id = None;
            if let Some(tab) = tab {
                conversation.detached_profile = tab.profile;
            }
        }
        for conversation in &mut self.conversations {
            if conversation
                .detached_profile
                .is_some_and(|id| !profiles.iter().any(|profile| profile.id == id))
            {
                conversation.detached_profile = None;
            }
        }
    }

    /// Remove conversations without a turn. Codex keeps them only in the
    /// memory of its current process, so another process cannot resume them.
    pub fn remove_unstarted(&mut self, unstarted: &BTreeSet<String>) {
        self.conversations
            .retain(|conversation| !unstarted.contains(&conversation.thread_id));
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_catalogs: Vec<SharedCatalog>,
    #[serde(default)]
    pub sign_ins: Vec<SignIn>,
    /// The connections that the Connections search looks in.
    #[serde(default, skip_serializing_if = "SearchScope::is_all")]
    pub search_scope: SearchScope,
}

/// The connections that the Connections search looks in.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    #[default]
    All,
    /// The connection of the active tab.
    Current,
    /// The selected connections. Never empty after `sanitize`.
    Connections(BTreeSet<Uuid>),
}

impl SearchScope {
    pub fn is_all(&self) -> bool {
        *self == Self::All
    }

    /// Whether the search looks in `profile`. `current` is the connection
    /// of the active tab.
    pub fn includes(&self, profile: Uuid, current: Option<Uuid>) -> bool {
        match self {
            Self::All => true,
            Self::Current => current == Some(profile),
            Self::Connections(ids) => ids.contains(&profile),
        }
    }

    /// Add `profile` to the selected connections, or remove it. Other
    /// scopes start a new selection with only `profile`.
    pub fn toggle(&mut self, profile: Uuid) {
        match self {
            Self::Connections(ids) => {
                if !ids.remove(&profile) {
                    ids.insert(profile);
                }
                if ids.is_empty() {
                    *self = Self::All;
                }
            }
            _ => *self = Self::Connections(BTreeSet::from([profile])),
        }
    }

    /// Remove connections that do not exist. An empty selection becomes All.
    pub fn sanitize(&mut self, profiles: &[Profile]) {
        if let Self::Connections(ids) = self {
            ids.retain(|id| profiles.iter().any(|profile| profile.id == *id));
            if ids.is_empty() {
                *self = Self::All;
            }
        }
    }
}

/// Remove the shared catalogs without members, and the references to
/// catalogs or members that do not exist. Returns the IDs of the removed
/// shared catalogs, whose cache files can go.
pub fn prune_shared_catalogs(
    profiles: &mut [Profile],
    shared: &mut Vec<SharedCatalog>,
) -> Vec<Uuid> {
    for profile in profiles.iter_mut() {
        if profile
            .shared_catalog
            .is_some_and(|id| !shared.iter().any(|catalog| catalog.id == id))
        {
            profile.shared_catalog = None;
        }
    }
    let mut removed = Vec::new();
    shared.retain(|catalog| {
        let used = profiles
            .iter()
            .any(|profile| profile.shared_catalog == Some(catalog.id));
        if !used {
            removed.push(catalog.id);
        }
        used
    });
    for catalog in shared.iter_mut() {
        if catalog.preferred.is_some_and(|preferred| {
            !profiles
                .iter()
                .any(|p| p.id == preferred && p.shared_catalog == Some(catalog.id))
        }) {
            catalog.preferred = None;
        }
    }
    removed
}

impl Workspace {
    /// Upgrade the flat tab list used by version 1 workspaces. New workspaces
    /// still use one list in storage, but every tab is normalized to an owned
    /// connection and each connection gets an active tab.
    pub fn normalize(&mut self) {
        // Older Trino profiles referred to a reusable sign-in. The coordinator
        // now owns authentication directly; legacy records have no credentials.
        let legacy_external: BTreeSet<_> = self
            .sign_ins
            .iter()
            .filter(|sign_in| sign_in.provider == SignInProvider::TrinoExternal)
            .map(|sign_in| sign_in.id)
            .collect();
        for profile in &mut self.profiles {
            if profile.database_type == DatabaseType::Trino
                && profile
                    .authentication
                    .sign_in()
                    .is_some_and(|id| legacy_external.contains(&id))
            {
                profile.authentication = Authentication::TrinoExternal;
            }
        }
        self.sign_ins
            .retain(|sign_in| sign_in.provider != SignInProvider::TrinoExternal);
        let conversations_without_tabs = self.version < 4;
        for profile in &mut self.profiles {
            profile.sanitize();
        }
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
        let selected = self
            .assistant
            .legacy_selected_thread
            .take()
            .map(|thread| thread.trim().to_owned());
        if conversations_without_tabs {
            // Earlier conversations used the selected tab. The conversation
            // that was open keeps the active tab. The others are detached
            // under the connection that was active.
            let tab = self.tabs.get(self.active_tab);
            let profile = tab.and_then(|tab| tab.profile);
            for conversation in &mut self.assistant.conversations {
                conversation.detached_profile = profile;
                conversation.tab_id = None;
                if selected.as_deref() == Some(conversation.thread_id.as_str()) {
                    conversation.tab_id = tab.map(|tab| tab.id);
                }
            }
        }
        self.assistant.link_tabs(&self.tabs, &self.profiles);
        for catalog in &mut self.shared_catalogs {
            catalog.sanitize();
        }
        prune_shared_catalogs(&mut self.profiles, &mut self.shared_catalogs);
        self.search_scope.sanitize(&self.profiles);
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
            shared_catalogs: Vec::new(),
            sign_ins: vec![],
            search_scope: SearchScope::All,
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Column {
    pub name: String,
    pub data_type: String,
}

pub type Row = Vec<Option<String>>;

#[derive(Default, Debug)]
pub struct Batch {
    pub rows: Vec<Row>,
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod database_type_tests;

#[cfg(test)]
mod trino_tests;

#[cfg(test)]
mod external_auth_tests;
