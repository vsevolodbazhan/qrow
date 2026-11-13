use std::{
    collections::HashSet,
    time::{Duration, SystemTime},
};

pub const MAX_EXECUTION_GROUPS: usize = 100;
/// Groups without an execution, such as failed keep-alives, disconnects, and rejected SQL, have
/// their own cap, so they cannot remove query history.
pub const MAX_NON_EXECUTION_GROUPS: usize = 50;
pub const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;

/// `value` with a comma between groups of three digits, like 1,234,567.
pub fn group_digits(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

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
    /// A query waits for a browser sign-in, or the sign-in ended.
    SignIn,
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

/// How [`LogHistory::record`] changed the entries.
#[derive(Debug, PartialEq, Eq)]
pub enum LogChange {
    /// The new entry is the last entry. The other entries did not change.
    Appended,
    /// Retention removed entries. `before` has the entry IDs from before
    /// the record, in order.
    Rewritten { before: Vec<u64> },
}

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
    pub fn record(&mut self, event: LogEvent) -> LogChange {
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
        match self.retain() {
            Some(before) => LogChange::Rewritten { before },
            None => LogChange::Appended,
        }
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

    /// Remove old groups over the limits. Returns the entry IDs before the
    /// last record when it removes entries.
    fn retain(&mut self) -> Option<Vec<u64>> {
        if !self.over_limit(self.groups.len()) {
            return None;
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
            return None;
        }
        let before = self.entries[..self.entries.len() - 1]
            .iter()
            .map(LogEntry::id)
            .collect();
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
        Some(before)
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
    unread_success: bool,
}

impl PanelState {
    pub fn has_unread_success(&self) -> bool {
        self.unread_success
    }

    pub fn execution_started(&mut self) {
        self.unread_error = false;
        self.unread_success = false;
    }

    pub fn user_select(&mut self, panel: Panel) {
        self.selected = panel;
        self.content_visible();
    }

    pub fn failure(&mut self, active: bool) {
        self.unread_error = !active;
        self.unread_success = false;
        self.selected = Panel::Output;
    }

    pub fn success(&mut self, active: bool) {
        self.selected = Panel::Results;
        self.unread_error = false;
        self.unread_success = !active;
    }

    pub fn content_visible(&mut self) {
        match self.selected {
            Panel::Output => self.unread_error = false,
            Panel::Results => self.unread_success = false,
        }
    }
}

#[cfg(test)]
mod tests;
