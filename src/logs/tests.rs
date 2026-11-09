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
fn record_reports_an_append_or_the_entries_before_a_removal() {
    let mut log = LogHistory::default();
    for id in 0..MAX_EXECUTION_GROUPS as u64 {
        assert_eq!(
            log.record(event(Some(id), LogKind::Submitted, "start")),
            LogChange::Appended
        );
    }
    let before: Vec<u64> = log.entries().map(LogEntry::id).collect();
    let change = log.record(event(Some(1_000), LogKind::Submitted, "start"));
    assert_eq!(change, LogChange::Rewritten { before });
    assert_eq!(
        log.record(event(Some(1_000), LogKind::ExecutionCompleted, "complete")),
        LogChange::Appended
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
    panel.success(true);
    assert_eq!(panel.selected, Panel::Results);

    panel.failure(true);
    panel.success(true);
    assert_eq!(panel.selected, Panel::Results);
    assert!(!panel.unread_error);

    panel.failure(true);
    panel.user_select(Panel::Output);
    panel.success(true);
    assert_eq!(panel.selected, Panel::Results);

    panel.user_select(Panel::Output);
    assert_eq!(panel.selected, Panel::Output);
    panel.success(true);
    assert_eq!(panel.selected, Panel::Results);
}

#[test]
fn failures_select_logs_and_success_clears_errors_on_active_and_background_tabs() {
    for active in [true, false] {
        let mut panel = PanelState::default();
        panel.failure(active);
        assert_eq!(panel.selected, Panel::Output);
        assert_eq!(panel.unread_error, !active);
        panel.content_visible();
        assert!(!panel.unread_error);
        panel.success(true);
        assert_eq!(panel.selected, Panel::Results);

        panel.failure(active);
        panel.success(true);
        assert_eq!(panel.selected, Panel::Results);
        assert!(!panel.unread_error);
    }
}

#[test]
fn execution_start_clears_error_without_changing_panel_selection() {
    let mut panel = PanelState::default();
    panel.failure(false);
    assert_eq!(panel.selected, Panel::Output);
    assert!(panel.unread_error);

    panel.execution_started();
    assert_eq!(panel.selected, Panel::Output);
    assert!(!panel.unread_error);

    panel.user_select(Panel::Results);
    panel.failure(false);
    panel.user_select(Panel::Results);
    assert!(panel.unread_error, "Results acknowledged the error");
    panel.execution_started();
    assert_eq!(panel.selected, Panel::Results);
    assert!(!panel.unread_error);
}

#[test]
fn execution_start_does_not_clear_another_tabs_error() {
    let mut retrying_tab = PanelState::default();
    let mut background_tab = PanelState::default();
    retrying_tab.failure(false);
    background_tab.failure(false);

    retrying_tab.execution_started();

    assert!(!retrying_tab.unread_error);
    assert!(background_tab.unread_error);
    assert_eq!(background_tab.selected, Panel::Output);
}

#[test]
fn successful_background_results_stay_unread_until_results_show() {
    let mut panel = PanelState::default();
    panel.success(false);
    assert!(panel.unread_success);
    assert_eq!(panel.selected, Panel::Results);

    panel.user_select(Panel::Output);
    assert!(panel.unread_success);
    panel.content_visible();
    assert!(panel.unread_success);
    panel.user_select(Panel::Results);
    assert!(!panel.unread_success);

    panel.success(true);
    assert!(!panel.unread_success);
    panel.success(false);
    panel.execution_started();
    assert!(!panel.unread_success);
    panel.success(false);
    panel.failure(false);
    assert!(!panel.unread_success);
    assert!(panel.unread_error);
}
