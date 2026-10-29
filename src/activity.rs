//! Activity: the log of the background work of each connection.
//!
//! Each connection has one flat log, in the order the entries arrive: schema
//! refreshes, keep-alives, sessions, and short query events of its tabs. The
//! log is in memory and has a size limit. Query tab Logs keep only what
//! changes something for their tab; see [`tab_event`].

use crate::logs::{LogEvent, LogKind, Severity};
use std::{
    collections::{HashMap, VecDeque},
    time::SystemTime,
};
use uuid::Uuid;

/// The most text that the log of one connection keeps.
pub const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
/// The most entries that the log of one connection keeps.
pub const MAX_ENTRIES: usize = 50_000;
/// When a log is over a limit, it removes the oldest entries until it is this
/// fraction of the limit. Removals in large steps keep them rare.
const TRIM_TO: f64 = 0.9;

/// One entry of the Activity of a connection.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityEntry {
    pub timestamp: SystemTime,
    pub severity: Severity,
    pub text: String,
    /// The query tab that the entry is about, if any.
    pub tab: Option<Uuid>,
    /// Whether the entry counts as an unseen error until its Activity shows.
    counts: bool,
    /// Unique in the Activity of all connections, from the order of arrival.
    id: u64,
}

impl ActivityEntry {
    /// An entry that no tab shows. An error counts as unseen.
    pub fn new(severity: Severity, text: impl Into<String>) -> Self {
        Self {
            timestamp: SystemTime::now(),
            severity,
            text: text.into(),
            tab: None,
            counts: severity == Severity::Error,
            id: 0,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Whether the entry counts as an unseen error.
    pub fn counts(&self) -> bool {
        self.counts
    }
}

/// The entry of a schema refresh event. Only a failed outcome counts as an
/// unseen error, so a failed refresh counts once, also when several of its
/// requests failed.
pub fn from_refresh(event: LogEvent) -> ActivityEntry {
    ActivityEntry {
        timestamp: event.timestamp,
        severity: event.severity,
        counts: event.severity == Severity::Error && event.kind == LogKind::SchemaRefreshFinished,
        text: event.text,
        tab: None,
        id: 0,
    }
}

/// The entry of an event of the query tab `tab` with the title `title`, or
/// `None` when Activity does not show it. Activity has no SQL: a query shows
/// as short events with a link to its tab, which has the details. An error
/// does not count as unseen: the tab shows its own unread error.
pub fn from_tab(event: &LogEvent, tab: Uuid, title: &str) -> Option<ActivityEntry> {
    let text = match event.kind {
        LogKind::Connected
        | LogKind::Disconnected
        | LogKind::KeepAliveCompleted
        | LogKind::KeepAliveFailed
        | LogKind::ExecutionCompleted
        | LogKind::SignIn
        | LogKind::Cancelled => event.text.clone(),
        LogKind::Submitted => "Submitted a query".into(),
        // The tab has the full error of its query.
        LogKind::Error if event.execution_id.is_some() => "Query failed".into(),
        LogKind::Error => event.text.clone(),
        LogKind::ExecutionStarted
        | LogKind::FetchStarted
        | LogKind::FetchCompleted
        | LogKind::CancelRequested
        | LogKind::SchemaRefresh
        | LogKind::SchemaRefreshFinished
        | LogKind::HistoryTrimmed => return None,
    };
    Some(ActivityEntry {
        timestamp: event.timestamp,
        severity: event.severity,
        text: format!("{title}: {text}"),
        tab: Some(tab),
        counts: false,
        id: 0,
    })
}

/// The event that the Logs of the tab show, or `None` when it changes
/// nothing for the tab. A completed keep-alive shows only in Activity. A
/// failed keep-alive closed the session of the tab, so the tab shows it with
/// its SQL, which is a likely cause.
pub fn tab_event(mut event: LogEvent) -> Option<LogEvent> {
    match event.kind {
        LogKind::KeepAliveCompleted => None,
        LogKind::KeepAliveFailed => {
            if let Some(sql) = &event.sql {
                event.text = format!("{}\nKeep-alive query:\n{sql}", event.text);
            }
            Some(event)
        }
        _ => Some(event),
    }
}

/// The Activity log of one connection.
#[derive(Clone, Debug, Default)]
pub struct ActivityLog {
    entries: VecDeque<ActivityEntry>,
    text_bytes: usize,
    /// Whether the log removed old entries.
    trimmed: bool,
    unseen_errors: usize,
    /// The sequence number of the newest unseen error.
    newest_unseen: Option<u64>,
}

impl ActivityLog {
    pub fn entries(&self) -> &VecDeque<ActivityEntry> {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn trimmed(&self) -> bool {
        self.trimmed
    }

    pub fn unseen_errors(&self) -> usize {
        self.unseen_errors
    }

    pub fn text_bytes(&self) -> usize {
        self.text_bytes
    }

    /// Add `entry`. Returns how many old entries the log removed to stay in
    /// its limits.
    fn record(&mut self, mut entry: ActivityEntry, sequence: u64) -> usize {
        entry.id = sequence;
        if entry.counts() {
            self.unseen_errors += 1;
            self.newest_unseen = Some(sequence);
        }
        self.text_bytes += entry.text.len();
        self.entries.push_back(entry);
        if self.entries.len() <= MAX_ENTRIES && self.text_bytes <= MAX_TEXT_BYTES {
            return 0;
        }
        let entries = (MAX_ENTRIES as f64 * TRIM_TO) as usize;
        let bytes = (MAX_TEXT_BYTES as f64 * TRIM_TO) as usize;
        let mut removed = 0;
        // The newest entry stays, also when it is larger than the limit.
        while self.entries.len() > 1 && (self.entries.len() > entries || self.text_bytes > bytes) {
            let old = self.entries.pop_front().unwrap();
            self.text_bytes -= old.text.len();
            removed += 1;
        }
        self.trimmed = true;
        removed
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    fn mark_seen(&mut self) {
        self.unseen_errors = 0;
        self.newest_unseen = None;
    }
}

/// The first line of a log that removed old entries.
pub const TRIMMED_TEXT: &str = "Older entries were removed";

/// The Activity of all connections.
#[derive(Clone, Debug, Default)]
pub struct Activity {
    logs: HashMap<Uuid, ActivityLog>,
    sequence: u64,
}

impl Activity {
    pub fn log(&self, connection: Uuid) -> Option<&ActivityLog> {
        self.logs.get(&connection)
    }

    /// Add `entry` to the log of `connection`. Returns how many old entries
    /// that log removed.
    pub fn record(&mut self, connection: Uuid, entry: ActivityEntry) -> usize {
        self.sequence += 1;
        self.logs
            .entry(connection)
            .or_default()
            .record(entry, self.sequence)
    }

    pub fn clear(&mut self, connection: Uuid) {
        if let Some(log) = self.logs.get_mut(&connection) {
            log.clear();
        }
    }

    /// Forget a deleted connection.
    pub fn remove(&mut self, connection: Uuid) {
        self.logs.remove(&connection);
    }

    pub fn mark_seen(&mut self, connection: Uuid) {
        if let Some(log) = self.logs.get_mut(&connection) {
            log.mark_seen();
        }
    }

    /// The unseen errors of all connections.
    pub fn unseen_errors(&self) -> usize {
        self.logs.values().map(ActivityLog::unseen_errors).sum()
    }

    pub fn unseen_errors_of(&self, connection: Uuid) -> usize {
        self.logs
            .get(&connection)
            .map_or(0, ActivityLog::unseen_errors)
    }

    /// The connection with the newest unseen error.
    pub fn newest_unseen(&self) -> Option<Uuid> {
        self.logs
            .iter()
            .filter_map(|(id, log)| log.newest_unseen.map(|sequence| (sequence, *id)))
            .max()
            .map(|(_, id)| id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::ExecutionId;

    fn refresh_error(text: &str) -> ActivityEntry {
        ActivityEntry::new(Severity::Error, text)
    }

    #[test]
    fn only_errors_without_a_tab_count_and_the_newest_wins() {
        let (first, second) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let mut activity = Activity::default();
        activity.record(first, refresh_error("List schemas failed"));
        activity.record(second, refresh_error("List schemas failed"));
        let tab_error = LogEvent::new(None, Severity::Error, LogKind::Error, "Query failed");
        activity.record(
            first,
            from_tab(&tab_error, Uuid::from_u128(9), "Query 1").unwrap(),
        );
        // A failed request of a refresh does not count; its outcome does.
        let request = LogEvent::new(None, Severity::Error, LogKind::SchemaRefresh, "List failed");
        assert!(!from_refresh(request).counts());
        let outcome = LogEvent::new(
            None,
            Severity::Error,
            LogKind::SchemaRefreshFinished,
            "Schema refresh failed",
        );
        assert!(from_refresh(outcome).counts());
        activity.record(first, ActivityEntry::new(Severity::Info, "Started"));
        assert_eq!(activity.unseen_errors(), 2);
        assert_eq!(activity.newest_unseen(), Some(second));

        activity.mark_seen(second);
        assert_eq!(activity.unseen_errors(), 1);
        assert_eq!(activity.newest_unseen(), Some(first));
        activity.clear(first);
        assert_eq!(activity.unseen_errors(), 0);
        assert_eq!(activity.newest_unseen(), None);
        assert!(activity.log(first).unwrap().is_empty());
    }

    #[test]
    fn a_full_log_removes_its_oldest_entries_in_one_step() {
        let mut log = ActivityLog::default();
        for index in 0..MAX_ENTRIES {
            assert_eq!(
                log.record(ActivityEntry::new(Severity::Info, "x"), index as u64),
                0
            );
        }
        let removed = log.record(ActivityEntry::new(Severity::Info, "newest"), 0);
        assert_eq!(
            removed,
            MAX_ENTRIES + 1 - (MAX_ENTRIES as f64 * TRIM_TO) as usize
        );
        assert!(log.trimmed());
        assert_eq!(log.entries().back().unwrap().text, "newest");
        assert_eq!(log.entries().front().unwrap().text, "x");

        // One huge entry stays, as the newest one.
        let mut log = ActivityLog::default();
        log.record(ActivityEntry::new(Severity::Info, "old"), 0);
        log.record(
            ActivityEntry::new(Severity::Error, "e".repeat(MAX_TEXT_BYTES + 1)),
            1,
        );
        assert_eq!(log.entries().len(), 1);
        assert_eq!(log.text_bytes(), MAX_TEXT_BYTES + 1);
    }

    #[test]
    fn a_tab_shows_only_events_that_change_it() {
        let tab = Uuid::from_u128(3);
        let event = |kind, severity, text: &str| LogEvent::new(None, severity, kind, text);

        let completed = event(
            LogKind::KeepAliveCompleted,
            Severity::Info,
            "Keep-alive completed",
        );
        assert_eq!(tab_event(completed.clone()), None);
        assert_eq!(
            from_tab(&completed, tab, "Query 1").unwrap().text,
            "Query 1: Keep-alive completed"
        );

        let failed = event(
            LogKind::KeepAliveFailed,
            Severity::Error,
            "Keep-alive failed and closed the session: timeout",
        )
        .with_sql("SELECT 1");
        assert_eq!(
            tab_event(failed.clone()).unwrap().text,
            "Keep-alive failed and closed the session: timeout\nKeep-alive query:\nSELECT 1"
        );
        let entry = from_tab(&failed, tab, "Query 1").unwrap();
        assert_eq!(
            entry.text,
            "Query 1: Keep-alive failed and closed the session: timeout"
        );
        assert!(entry.is_error() && !entry.counts());

        // A query shows without its SQL and without its error.
        let submitted = LogEvent::new(
            Some(ExecutionId(1)),
            Severity::Info,
            LogKind::Submitted,
            "Submitted query:\nSELECT secret",
        );
        assert_eq!(
            from_tab(&submitted, tab, "Query 2").unwrap().text,
            "Query 2: Submitted a query"
        );
        let error = LogEvent::new(
            Some(ExecutionId(1)),
            Severity::Error,
            LogKind::Error,
            "Syntax error near SELECT secret",
        );
        assert_eq!(
            from_tab(&error, tab, "Query 2").unwrap().text,
            "Query 2: Query failed"
        );
        let fetch = LogEvent::new(
            Some(ExecutionId(1)),
            Severity::Info,
            LogKind::FetchCompleted,
            "Fetched preview page 1",
        );
        assert_eq!(from_tab(&fetch, tab, "Query 2"), None);
        assert!(tab_event(fetch).is_some());
    }
}
