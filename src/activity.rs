use std::time::{Duration, SystemTime};

pub const MAX_EXECUTION_GROUPS: usize = 100;
pub const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ExecutionId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Info,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityKind {
    Connected,
    Submitted,
    ExecutionStarted,
    ExecutionCompleted,
    FetchStarted,
    FetchCompleted,
    KeepAliveStarted,
    KeepAliveCompleted,
    CancelRequested,
    Cancelled,
    Error,
    Disconnected,
    HistoryTrimmed,
}

impl ActivityKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::Submitted => "Submitted",
            Self::ExecutionStarted => "Execution started",
            Self::ExecutionCompleted => "Execution complete",
            Self::FetchStarted => "Fetch started",
            Self::FetchCompleted => "Fetch complete",
            Self::KeepAliveStarted => "Keep-alive started",
            Self::KeepAliveCompleted => "Keep-alive complete",
            Self::CancelRequested => "Cancellation requested",
            Self::Cancelled => "Cancelled",
            Self::Error => "Error",
            Self::Disconnected => "Disconnected",
            Self::HistoryTrimmed => "History trimmed",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityEvent {
    pub timestamp: SystemTime,
    pub execution_id: Option<ExecutionId>,
    pub severity: Severity,
    pub kind: ActivityKind,
    pub text: String,
    pub connection: Option<String>,
    pub sql: Option<String>,
    pub duration: Option<Duration>,
}

impl ActivityEvent {
    pub fn new(
        execution_id: Option<ExecutionId>,
        severity: Severity,
        kind: ActivityKind,
        text: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: SystemTime::now(),
            execution_id,
            severity,
            kind,
            text: text.into(),
            connection: None,
            sql: None,
            duration: None,
        }
    }

    pub fn at(
        timestamp: SystemTime,
        execution_id: Option<ExecutionId>,
        severity: Severity,
        kind: ActivityKind,
        text: impl Into<String>,
    ) -> Self {
        let mut event = Self::new(execution_id, severity, kind, text);
        event.timestamp = timestamp;
        event
    }

    pub fn with_connection(mut self, connection: impl Into<String>) -> Self {
        self.connection = Some(connection.into());
        self
    }

    pub fn with_sql(mut self, sql: impl Into<String>) -> Self {
        self.sql = Some(sql.into());
        self
    }

    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityEntry {
    pub timestamp: SystemTime,
    pub execution_id: Option<ExecutionId>,
    pub severity: Severity,
    pub kind: ActivityKind,
    pub text: String,
    pub connection: Option<String>,
    pub sql: Option<String>,
    pub duration: Option<Duration>,
    entry_id: u64,
    group_id: u64,
}

impl From<ActivityEvent> for ActivityEntry {
    fn from(event: ActivityEvent) -> Self {
        Self {
            timestamp: event.timestamp,
            execution_id: event.execution_id,
            severity: event.severity,
            kind: event.kind,
            text: event.text,
            connection: event.connection,
            sql: event.sql,
            duration: event.duration,
            entry_id: 0,
            group_id: 0,
        }
    }
}

impl ActivityEntry {
    pub fn id(&self) -> u64 {
        self.entry_id
    }

    pub fn text_bytes(&self) -> usize {
        self.text.len()
            + self.connection.as_deref().map_or(0, str::len)
            + self.sql.as_deref().map_or(0, str::len)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityGroup {
    pub id: u64,
    pub execution_id: Option<ExecutionId>,
    pub connection: Option<String>,
    pub sql: Option<String>,
    pub entries: Vec<ActivityEntry>,
    text_bytes: usize,
}

impl ActivityGroup {
    fn new(id: u64, entry: ActivityEntry) -> Self {
        let text_bytes = entry.text_bytes();
        Self {
            id,
            execution_id: entry.execution_id,
            connection: entry.connection.clone(),
            sql: entry.sql.clone(),
            entries: vec![entry],
            text_bytes,
        }
    }

    fn add(&mut self, entry: ActivityEntry) {
        self.connection = entry.connection.clone().or_else(|| self.connection.clone());
        self.sql = entry.sql.clone().or_else(|| self.sql.clone());
        self.text_bytes += entry.text_bytes();
        self.entries.push(entry);
    }

    pub fn text_bytes(&self) -> usize {
        self.text_bytes
    }
}

#[derive(Clone, Debug, Default)]
pub struct ActivityLog {
    groups: Vec<ActivityGroup>,
    ordered_entries: Vec<ActivityEntry>,
    next_group_id: u64,
    next_entry_id: u64,
    text_bytes: usize,
    retention_entry_id: Option<u64>,
}

impl ActivityLog {
    pub fn record(&mut self, event: ActivityEvent) {
        let mut entry: ActivityEntry = event.into();
        entry.entry_id = self.next_entry_id;
        self.next_entry_id += 1;
        let group = entry.execution_id.and_then(|execution_id| {
            self.groups
                .iter()
                .position(|group| group.execution_id == Some(execution_id))
        });
        if let Some(index) = group {
            entry.group_id = self.groups[index].id;
            self.ordered_entries.push(entry.clone());
            let mut group = self.groups.remove(index);
            self.text_bytes += entry.text_bytes();
            group.add(entry);
            self.groups.push(group);
        } else {
            let id = self.next_group_id;
            self.next_group_id += 1;
            self.text_bytes += entry.text_bytes();
            entry.group_id = id;
            self.ordered_entries.push(entry.clone());
            self.groups.push(ActivityGroup::new(id, entry));
        }
        self.retain();
    }

    pub fn record_connection(
        &mut self,
        kind: ActivityKind,
        text: impl Into<String>,
        connection: Option<String>,
    ) {
        let mut event = ActivityEvent::new(None, Severity::Info, kind, text);
        event.connection = connection;
        self.record(event);
    }

    pub fn clear(&mut self) {
        self.groups.clear();
        self.ordered_entries.clear();
        self.text_bytes = 0;
        self.retention_entry_id = None;
    }

    pub fn groups(&self) -> &[ActivityGroup] {
        &self.groups
    }

    pub fn entries(&self) -> impl Iterator<Item = &ActivityEntry> {
        self.ordered_entries.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub fn text_bytes(&self) -> usize {
        self.text_bytes
    }

    pub fn latest_error(&self) -> Option<&ActivityEntry> {
        self.entries()
            .filter(|entry| entry.severity == Severity::Error)
            .last()
    }

    pub fn copy_all(&self) -> String {
        self.entries()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn copy_error(&self) -> Option<String> {
        self.latest_error().map(|entry| entry.text.clone())
    }

    fn retain(&mut self) {
        let latest_execution = self
            .groups
            .iter()
            .filter_map(|group| group.execution_id)
            .max_by_key(|id| id.0);
        let latest_execution_group = latest_execution
            .and_then(|execution_id| {
                self.groups
                    .iter()
                    .find(|group| group.execution_id == Some(execution_id))
            })
            .map(|group| group.id);
        let latest_error_group = self.latest_error().map(|entry| entry.group_id);
        let protected_groups = [
            latest_execution_group,
            latest_error_group,
            if latest_execution_group.is_none() && latest_error_group.is_none() {
                self.groups.last().map(|group| group.id)
            } else {
                None
            },
        ];
        let mut removed = false;
        while (self.groups.len() > MAX_EXECUTION_GROUPS || self.text_bytes > MAX_TEXT_BYTES)
            && self.groups.len() > 1
        {
            let Some(index) = self
                .groups
                .iter()
                .position(|group| !protected_groups.contains(&Some(group.id)))
            else {
                break;
            };
            let removed_group = self.groups.remove(index);
            self.text_bytes = self.text_bytes.saturating_sub(removed_group.text_bytes());
            removed = true;
        }
        if removed {
            self.ordered_entries.retain(|entry| {
                Some(entry.id()) == self.retention_entry_id
                    || self.groups.iter().any(|group| group.id == entry.group_id)
            });
            if self.retention_entry_id.is_none() {
                let mut entry: ActivityEntry = ActivityEvent::new(
                    None,
                    Severity::Info,
                    ActivityKind::HistoryTrimmed,
                    "Older activity was removed",
                )
                .into();
                entry.entry_id = self.next_entry_id;
                self.next_entry_id += 1;
                self.text_bytes += entry.text_bytes();
                self.retention_entry_id = Some(entry.id());
                self.ordered_entries.insert(0, entry);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Panel {
    #[default]
    Results,
    Output,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PanelState {
    pub selected: Panel,
    pub unread_error: bool,
}

impl PanelState {
    pub fn execution_started(&mut self) {
        self.unread_error = false;
    }

    pub fn user_select(&mut self, panel: Panel) {
        self.selected = panel;
        if panel == Panel::Output {
            self.unread_error = false;
        }
    }

    pub fn failure(&mut self, _active: bool) {
        self.unread_error = true;
        self.selected = Panel::Output;
    }

    pub fn success(&mut self) {
        self.selected = Panel::Results;
        self.unread_error = false;
    }

    pub fn output_visible(&mut self) {
        if self.selected == Panel::Output {
            self.unread_error = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: Option<u64>, kind: ActivityKind, text: &str) -> ActivityEvent {
        ActivityEvent::at(
            SystemTime::UNIX_EPOCH,
            id.map(ExecutionId),
            if kind == ActivityKind::Error {
                Severity::Error
            } else {
                Severity::Info
            },
            kind,
            text,
        )
    }

    #[test]
    fn execution_entries_stay_in_order_and_connection_entries_are_separate_groups() {
        let mut log = ActivityLog::default();
        log.record(event(Some(1), ActivityKind::Submitted, "select 1"));
        log.record(event(None, ActivityKind::Connected, "connected"));
        log.record(event(Some(1), ActivityKind::ExecutionCompleted, "complete"));

        assert_eq!(log.groups().len(), 2);
        assert_eq!(log.groups()[0].entries.len(), 1);
        assert_eq!(log.groups()[1].entries.len(), 2);
        assert_eq!(
            log.entries()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["select 1", "connected", "complete",]
        );
    }

    #[test]
    fn retention_removes_whole_old_groups_and_keeps_latest_error_complete() {
        let mut log = ActivityLog::default();
        for id in 0..=(MAX_EXECUTION_GROUPS as u64 + 10) {
            log.record(event(Some(id), ActivityKind::Submitted, "start"));
            log.record(event(Some(id), ActivityKind::Error, "line one\nline two"));
        }
        assert_eq!(log.groups().len(), MAX_EXECUTION_GROUPS);
        assert_eq!(log.groups()[0].execution_id, Some(ExecutionId(11)));
        assert_eq!(log.latest_error().unwrap().text, "line one\nline two");
        let entries: Vec<_> = log.entries().collect();
        assert_eq!(entries[0].kind, ActivityKind::HistoryTrimmed);
        assert_eq!(entries[0].text, "Older activity was removed");
        assert_eq!(entries[1].execution_id, Some(ExecutionId(11)));
        assert_eq!(
            entries
                .iter()
                .copied()
                .filter(|entry| entry.kind == ActivityKind::HistoryTrimmed)
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["Older activity was removed"]
        );
        assert!(log.copy_all().contains("Older activity was removed"));
    }

    #[test]
    fn retention_applies_the_text_budget_and_bounds_non_execution_history() {
        let mut log = ActivityLog::default();
        let text = "x".repeat(MAX_TEXT_BYTES / 2 + 1);
        log.record(event(Some(1), ActivityKind::Submitted, &text));
        log.record(event(Some(2), ActivityKind::Submitted, &text));
        assert_eq!(log.groups().len(), 1);
        assert_eq!(log.groups()[0].execution_id, Some(ExecutionId(2)));
        assert_eq!(
            log.entries()
                .filter(|entry| entry.kind == ActivityKind::HistoryTrimmed)
                .count(),
            1
        );

        let mut connection_log = ActivityLog::default();
        for _ in 0..=MAX_EXECUTION_GROUPS {
            connection_log.record(event(None, ActivityKind::Connected, "connected"));
        }
        assert_eq!(connection_log.groups().len(), MAX_EXECUTION_GROUPS);
        assert!(
            connection_log
                .groups()
                .iter()
                .all(|group| group.execution_id.is_none())
        );
    }

    #[test]
    fn retention_keeps_a_latest_connection_error_group() {
        let mut log = ActivityLog::default();
        log.record(event(None, ActivityKind::Error, "important\nerror details"));
        for id in 0..=MAX_EXECUTION_GROUPS as u64 {
            log.record(event(Some(id), ActivityKind::Submitted, "start"));
        }
        assert_eq!(log.latest_error().unwrap().text, "important\nerror details");
        assert!(
            log.groups()
                .iter()
                .any(|group| group.execution_id.is_none())
        );
    }

    #[test]
    fn clear_drops_entries_but_does_not_break_later_events_for_the_same_execution() {
        let mut log = ActivityLog::default();
        log.record(event(Some(7), ActivityKind::Submitted, "old"));
        for id in 0..=MAX_EXECUTION_GROUPS as u64 {
            log.record(event(Some(id), ActivityKind::Submitted, "overflow"));
        }
        log.clear();
        log.record(event(Some(7), ActivityKind::Error, "new"));
        assert_eq!(log.groups().len(), 1);
        assert_eq!(log.groups()[0].execution_id, Some(ExecutionId(7)));
        assert_eq!(log.copy_all(), "new");
        assert!(
            log.entries()
                .all(|entry| entry.kind != ActivityKind::HistoryTrimmed)
        );
    }

    #[test]
    fn successful_queries_select_results_after_logs_were_selected() {
        let mut panel = PanelState::default();
        panel.user_select(Panel::Output);
        assert_eq!(panel.selected, Panel::Output);
        panel.success();
        assert_eq!(panel.selected, Panel::Results);

        panel.failure(true);
        panel.success();
        assert_eq!(panel.selected, Panel::Results);
        assert!(!panel.unread_error);

        panel.failure(true);
        panel.user_select(Panel::Output);
        panel.success();
        assert_eq!(panel.selected, Panel::Results);

        panel.user_select(Panel::Output);
        assert_eq!(panel.selected, Panel::Output);
        panel.success();
        assert_eq!(panel.selected, Panel::Results);
    }

    #[test]
    fn failures_select_logs_and_success_clears_errors_on_active_and_background_tabs() {
        for active in [true, false] {
            let mut panel = PanelState::default();
            panel.failure(active);
            assert_eq!(panel.selected, Panel::Output);
            assert!(panel.unread_error);
            panel.output_visible();
            assert!(!panel.unread_error);
            panel.success();
            assert_eq!(panel.selected, Panel::Results);

            panel.failure(active);
            panel.success();
            assert_eq!(panel.selected, Panel::Results);
            assert!(!panel.unread_error);
        }
    }

    #[test]
    fn execution_start_clears_error_without_changing_panel_selection() {
        let mut panel = PanelState::default();
        panel.failure(true);
        assert_eq!(panel.selected, Panel::Output);
        assert!(panel.unread_error);

        panel.execution_started();
        assert_eq!(panel.selected, Panel::Output);
        assert!(!panel.unread_error);

        panel.user_select(Panel::Results);
        panel.failure(true);
        panel.user_select(Panel::Results);
        panel.execution_started();
        assert_eq!(panel.selected, Panel::Results);
        assert!(!panel.unread_error);
    }

    #[test]
    fn execution_start_does_not_clear_another_tabs_error() {
        let mut retrying_tab = PanelState::default();
        let mut background_tab = PanelState::default();
        retrying_tab.failure(true);
        background_tab.failure(false);

        retrying_tab.execution_started();

        assert!(!retrying_tab.unread_error);
        assert!(background_tab.unread_error);
        assert_eq!(background_tab.selected, Panel::Output);
    }
}
