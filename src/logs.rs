use std::{
    collections::HashSet,
    time::{Duration, SystemTime},
};

pub const MAX_EXECUTION_GROUPS: usize = 100;
/// Groups without an execution, such as failed keep-alives, disconnects, and rejected SQL, have
/// their own cap, so they cannot remove query history.
pub const MAX_NON_EXECUTION_GROUPS: usize = 50;
pub const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn timestamp_label(timestamp: SystemTime) -> String {
    let seconds = timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let hour = day_seconds / 3_600;
    let minute = day_seconds % 3_600 / 60;
    let second = day_seconds % 60;

    // Convert days since 1970-01-01 to a Gregorian date without another crate.
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }).div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_part = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096)
        .div_euclid(365);
    let year = year_part + era * 400;
    let day_of_year = day_of_era - (365 * year_part + year_part / 4 - year_part / 100);
    let month_part = (5 * day_of_year + 2).div_euclid(153);
    let day = day_of_year - (153 * month_part + 2).div_euclid(5) + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ExecutionId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Info,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogKind {
    Connected,
    Submitted,
    ExecutionStarted,
    ExecutionCompleted,
    FetchStarted,
    FetchCompleted,
    KeepAliveCompleted,
    /// A keep-alive failed and closed the session of the tab.
    KeepAliveFailed,
    SchemaRefresh,
    /// The outcome of a schema refresh: completed, stopped, or failed.
    SchemaRefreshFinished,
    CancelRequested,
    Cancelled,
    Error,
    Disconnected,
    HistoryTrimmed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogEvent {
    pub timestamp: SystemTime,
    pub execution_id: Option<ExecutionId>,
    pub severity: Severity,
    pub kind: LogKind,
    pub text: String,
    pub connection: Option<String>,
    pub sql: Option<String>,
    pub duration: Option<Duration>,
}

impl LogEvent {
    pub fn new(
        execution_id: Option<ExecutionId>,
        severity: Severity,
        kind: LogKind,
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
        kind: LogKind,
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub timestamp: SystemTime,
    pub execution_id: Option<ExecutionId>,
    pub severity: Severity,
    pub kind: LogKind,
    pub text: String,
    pub connection: Option<String>,
    pub sql: Option<String>,
    pub duration: Option<Duration>,
    entry_id: u64,
    group_id: u64,
}

impl From<LogEvent> for LogEntry {
    fn from(event: LogEvent) -> Self {
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

impl LogEntry {
    pub fn id(&self) -> u64 {
        self.entry_id
    }

    pub fn text_bytes(&self) -> usize {
        self.text.len()
            + self.connection.as_deref().map_or(0, str::len)
            + self.sql.as_deref().map_or(0, str::len)
    }

    fn copy_text(&self) -> String {
        if self.kind == LogKind::HistoryTrimmed {
            self.text.clone()
        } else {
            format!("[{}] {}", timestamp_label(self.timestamp), self.text)
        }
    }
}

/// Metadata of one execution, or of one event without an execution. The entries stay in
/// [`LogHistory::entries`].
#[derive(Clone, Debug, PartialEq)]
pub struct LogGroup {
    pub id: u64,
    pub execution_id: Option<ExecutionId>,
    text_bytes: usize,
}

impl LogGroup {
    pub fn text_bytes(&self) -> usize {
        self.text_bytes
    }
}

/// The group ID of the retention line. It belongs to no group.
const NO_GROUP: u64 = u64::MAX;

#[derive(Clone, Debug, Default)]
pub struct LogHistory {
    /// Groups in creation order.
    groups: Vec<LogGroup>,
    /// Entries in record order, after the retention line if there is one.
    entries: Vec<LogEntry>,
    execution_groups: usize,
    /// Index of the latest error in `entries`.
    latest_error: Option<usize>,
    next_group_id: u64,
    next_entry_id: u64,
    text_bytes: usize,
    trimmed: bool,
}

impl LogHistory {
    pub fn record(&mut self, event: LogEvent) {
        let mut entry: LogEntry = event.into();
        entry.entry_id = self.next_entry_id;
        self.next_entry_id += 1;
        let existing = entry.execution_id.and_then(|execution_id| {
            self.groups
                .iter()
                .rposition(|group| group.execution_id == Some(execution_id))
        });
        let index = existing.unwrap_or_else(|| {
            let id = self.next_group_id;
            self.next_group_id += 1;
            self.execution_groups += usize::from(entry.execution_id.is_some());
            self.groups.push(LogGroup {
                id,
                execution_id: entry.execution_id,
                text_bytes: 0,
            });
            self.groups.len() - 1
        });
        let group = &mut self.groups[index];
        let text_bytes = entry.text_bytes();
        group.text_bytes += text_bytes;
        self.text_bytes += text_bytes;
        entry.group_id = group.id;
        if entry.severity == Severity::Error {
            self.latest_error = Some(self.entries.len());
        }
        self.entries.push(entry);
        self.retain();
    }

    pub fn clear(&mut self) {
        self.groups.clear();
        self.entries.clear();
        self.execution_groups = 0;
        self.latest_error = None;
        self.text_bytes = 0;
        self.trimmed = false;
    }

    pub fn groups(&self) -> &[LogGroup] {
        &self.groups
    }

    pub fn entries(&self) -> std::slice::Iter<'_, LogEntry> {
        self.entries.iter()
    }

    pub fn group_entries(&self, group_id: u64) -> impl Iterator<Item = &LogEntry> {
        self.entries
            .iter()
            .filter(move |entry| entry.group_id == group_id)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn has_error(&self) -> bool {
        self.latest_error.is_some()
    }

    pub fn text_bytes(&self) -> usize {
        self.text_bytes
    }

    pub fn latest_error(&self) -> Option<&LogEntry> {
        self.latest_error.map(|index| &self.entries[index])
    }

    pub fn copy_all(&self) -> String {
        self.entries()
            .map(LogEntry::copy_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn copy_error(&self) -> Option<String> {
        self.latest_error().map(LogEntry::copy_text)
    }

    fn over_limit(&self, groups: usize) -> bool {
        self.execution_groups > MAX_EXECUTION_GROUPS
            || groups - self.execution_groups > MAX_NON_EXECUTION_GROUPS
            || self.text_bytes > MAX_TEXT_BYTES
    }

    fn retain(&mut self) {
        if !self.over_limit(self.groups.len()) {
            return;
        }
        let latest_execution_group = self
            .groups
            .iter()
            .filter(|group| group.execution_id.is_some())
            .max_by_key(|group| group.execution_id.map(|id| id.0))
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
        let mut removed = HashSet::new();
        let mut groups = self.groups.len();
        for group in &self.groups {
            if groups <= 1 || !self.over_limit(groups) {
                break;
            }
            // Only the text budget removes groups of a kind that is within its own cap.
            let over_cap = if group.execution_id.is_some() {
                self.execution_groups > MAX_EXECUTION_GROUPS
            } else {
                groups - self.execution_groups > MAX_NON_EXECUTION_GROUPS
            };
            if !(over_cap || self.text_bytes > MAX_TEXT_BYTES)
                || protected_groups.contains(&Some(group.id))
            {
                continue;
            }
            removed.insert(group.id);
            groups -= 1;
            self.execution_groups -= usize::from(group.execution_id.is_some());
            self.text_bytes = self.text_bytes.saturating_sub(group.text_bytes);
        }
        if removed.is_empty() {
            return;
        }
        self.groups.retain(|group| !removed.contains(&group.id));
        self.entries
            .retain(|entry| !removed.contains(&entry.group_id));
        if !self.trimmed {
            self.trimmed = true;
            let mut entry: LogEntry = LogEvent::new(
                None,
                Severity::Info,
                LogKind::HistoryTrimmed,
                "Older log entries were removed",
            )
            .into();
            entry.entry_id = self.next_entry_id;
            entry.group_id = NO_GROUP;
            self.next_entry_id += 1;
            self.text_bytes += entry.text_bytes();
            self.entries.insert(0, entry);
        }
        self.latest_error = self
            .entries
            .iter()
            .rposition(|entry| entry.severity == Severity::Error);
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

    fn event(id: Option<u64>, kind: LogKind, text: &str) -> LogEvent {
        LogEvent::at(
            SystemTime::UNIX_EPOCH,
            id.map(ExecutionId),
            if kind == LogKind::Error {
                Severity::Error
            } else {
                Severity::Info
            },
            kind,
            text,
        )
    }

    #[test]
    fn copy_actions_preserve_recorded_timestamps_and_error_details() {
        let mut log = LogHistory::default();
        assert_eq!(log.copy_all(), "");
        assert_eq!(log.copy_error(), None);
        for (seconds, kind, text) in [
            (86_399, LogKind::Submitted, "SELECT 'café 🐦'\nFROM table"),
            (86_400, LogKind::Error, "old error"),
            (86_401, LogKind::Error, "latest error\n詳細 🐦"),
            (86_402, LogKind::ExecutionCompleted, "complete"),
        ] {
            let mut entry = event(Some(1), kind, text);
            entry.timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
            log.record(entry);
        }

        assert_eq!(
            log.copy_all(),
            concat!(
                "[1970-01-01 23:59:59] SELECT 'café 🐦'\nFROM table\n",
                "[1970-01-02 00:00:00] old error\n",
                "[1970-01-02 00:00:01] latest error\n詳細 🐦\n",
                "[1970-01-02 00:00:02] complete",
            )
        );
        assert_eq!(
            log.copy_error().as_deref(),
            Some("[1970-01-02 00:00:01] latest error\n詳細 🐦")
        );
    }

    #[test]
    fn execution_entries_stay_in_order_and_connection_entries_are_separate_groups() {
        let mut log = LogHistory::default();
        log.record(event(Some(1), LogKind::Submitted, "select 1"));
        log.record(event(None, LogKind::Connected, "connected"));
        log.record(event(Some(1), LogKind::ExecutionCompleted, "complete"));

        assert_eq!(log.groups().len(), 2);
        assert_eq!(log.groups()[0].execution_id, Some(ExecutionId(1)));
        assert_eq!(log.group_entries(log.groups()[0].id).count(), 2);
        assert_eq!(log.group_entries(log.groups()[1].id).count(), 1);
        assert_eq!(
            log.entries()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["select 1", "connected", "complete",]
        );
    }

    #[test]
    fn retention_removes_whole_old_groups_and_keeps_latest_error_complete() {
        let mut log = LogHistory::default();
        for id in 0..=(MAX_EXECUTION_GROUPS as u64 + 10) {
            log.record(event(Some(id), LogKind::Submitted, "start"));
            log.record(event(Some(id), LogKind::Error, "line one\nline two"));
        }
        assert_eq!(log.groups().len(), MAX_EXECUTION_GROUPS);
        assert_eq!(log.groups()[0].execution_id, Some(ExecutionId(11)));
        assert_eq!(log.latest_error().unwrap().text, "line one\nline two");
        let entries: Vec<_> = log.entries().collect();
        assert_eq!(entries[0].kind, LogKind::HistoryTrimmed);
        assert_eq!(entries[0].text, "Older log entries were removed");
        assert_eq!(entries[1].execution_id, Some(ExecutionId(11)));
        assert_eq!(
            entries
                .iter()
                .copied()
                .filter(|entry| entry.kind == LogKind::HistoryTrimmed)
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["Older log entries were removed"]
        );
        assert!(
            log.copy_all()
                .starts_with("Older log entries were removed\n")
        );
    }

    #[test]
    fn retention_applies_the_text_budget_and_bounds_non_execution_history() {
        let mut log = LogHistory::default();
        let text = "x".repeat(MAX_TEXT_BYTES / 2 + 1);
        log.record(event(Some(1), LogKind::Submitted, &text));
        log.record(event(Some(2), LogKind::Submitted, &text));
        assert_eq!(log.groups().len(), 1);
        assert_eq!(log.groups()[0].execution_id, Some(ExecutionId(2)));
        assert_eq!(
            log.entries()
                .filter(|entry| entry.kind == LogKind::HistoryTrimmed)
                .count(),
            1
        );

        let mut connection_log = LogHistory::default();
        for _ in 0..=MAX_NON_EXECUTION_GROUPS {
            connection_log.record(event(None, LogKind::Disconnected, "disconnected"));
        }
        assert_eq!(connection_log.groups().len(), MAX_NON_EXECUTION_GROUPS);
        assert!(
            connection_log
                .groups()
                .iter()
                .all(|group| group.execution_id.is_none())
        );
        assert_eq!(connection_log.entries().len(), MAX_NON_EXECUTION_GROUPS + 1);
    }

    #[test]
    fn keep_alive_cycles_do_not_remove_query_history() {
        let mut log = LogHistory::default();
        for id in 0..MAX_EXECUTION_GROUPS as u64 {
            log.record(event(Some(id), LogKind::Submitted, "select 1"));
            log.record(event(Some(id), LogKind::ExecutionCompleted, "complete"));
        }
        for _ in 0..1_000 {
            log.record(event(None, LogKind::KeepAliveCompleted, "complete"));
        }

        let executions: Vec<_> = log
            .groups()
            .iter()
            .filter_map(|group| group.execution_id)
            .collect();
        assert_eq!(
            executions,
            (0..MAX_EXECUTION_GROUPS as u64)
                .map(ExecutionId)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            log.groups().len(),
            MAX_EXECUTION_GROUPS + MAX_NON_EXECUTION_GROUPS
        );
        let entries: Vec<_> = log.entries().collect();
        assert_eq!(
            entries.len(),
            1 + 2 * MAX_EXECUTION_GROUPS + MAX_NON_EXECUTION_GROUPS
        );
        assert_eq!(entries[0].kind, LogKind::HistoryTrimmed);
        assert_eq!(entries[1].execution_id, Some(ExecutionId(0)));
        assert_eq!(entries.last().unwrap().kind, LogKind::KeepAliveCompleted);
        assert!(
            entries
                .windows(2)
                .skip(1)
                .all(|pair| pair[0].id() < pair[1].id())
        );
    }

    #[test]
    fn the_latest_error_is_cached_across_trimming_and_clear() {
        let mut log = LogHistory::default();
        assert!(!log.has_error());
        log.record(event(Some(0), LogKind::Error, "old error"));
        log.record(event(Some(1), LogKind::Error, "latest error"));
        for id in 2..=MAX_EXECUTION_GROUPS as u64 + 5 {
            log.record(event(Some(id), LogKind::Submitted, "start"));
        }
        assert!(log.has_error());
        assert_eq!(
            log.copy_error().as_deref(),
            Some("[1970-01-01 00:00:00] latest error")
        );
        assert!(
            log.groups()
                .iter()
                .all(|group| group.execution_id != Some(ExecutionId(0)))
        );

        log.clear();
        assert!(!log.has_error());
        assert!(log.is_empty());
        assert_eq!(log.copy_error(), None);
    }

    #[test]
    fn group_entries_exclude_the_retention_line() {
        let mut log = LogHistory::default();
        let text = "x".repeat(MAX_TEXT_BYTES / 2 + 1);
        log.record(event(Some(0), LogKind::Submitted, &text));
        log.record(event(None, LogKind::Disconnected, &text));
        log.record(event(Some(0), LogKind::ExecutionCompleted, "complete"));
        assert_eq!(log.groups().len(), 1);
        let group = &log.groups()[0];
        assert_eq!((group.id, group.execution_id), (0, Some(ExecutionId(0))));
        assert_eq!(
            log.group_entries(group.id)
                .map(|entry| entry.kind)
                .collect::<Vec<_>>(),
            [LogKind::Submitted, LogKind::ExecutionCompleted]
        );
        assert_eq!(log.entries().len(), 3);
    }

    #[test]
    fn retention_keeps_a_latest_connection_error_group() {
        let mut log = LogHistory::default();
        log.record(event(None, LogKind::Error, "important\nerror details"));
        for id in 0..=MAX_EXECUTION_GROUPS as u64 {
            log.record(event(Some(id), LogKind::Submitted, "start"));
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
        let mut log = LogHistory::default();
        log.record(event(Some(7), LogKind::Submitted, "old"));
        for id in 0..=MAX_EXECUTION_GROUPS as u64 {
            log.record(event(Some(id), LogKind::Submitted, "overflow"));
        }
        log.clear();
        log.record(event(Some(7), LogKind::Error, "new"));
        assert_eq!(log.groups().len(), 1);
        assert_eq!(log.groups()[0].execution_id, Some(ExecutionId(7)));
        assert_eq!(log.copy_all(), "[1970-01-01 00:00:00] new");
        assert!(
            log.entries()
                .all(|entry| entry.kind != LogKind::HistoryTrimmed)
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
