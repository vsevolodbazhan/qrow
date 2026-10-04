use anyhow::Result;
use qrow::{
    connector::{Cancellation, Connector, MetadataRequest, QueryState, Session},
    logs::{LogKind, Severity},
    model::{Batch, Column, Profile},
    worker::{Event, Worker},
};
use std::{
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Default)]
struct Fixture {
    connects: AtomicUsize,
    fetches: Arc<AtomicUsize>,
    second_fetch: Option<Arc<Barrier>>,
    fail_fetch: Option<usize>,
    total_rows: usize,
    value_bytes: usize,
    /// The most rows that one fetch returns, like a server limit. 0 has no limit.
    max_batch: usize,
    requested: Arc<Mutex<Vec<usize>>>,
    closes: Arc<AtomicUsize>,
}
struct FakeSession {
    fetches: Arc<AtomicUsize>,
    second_fetch: Option<Arc<Barrier>>,
    fail_fetch: Option<usize>,
    preview_offset: Option<usize>,
    total_rows: usize,
    value_bytes: usize,
    max_batch: usize,
    requested: Arc<Mutex<Vec<usize>>>,
    offset: usize,
    slow: bool,
    cancelled: Arc<AtomicBool>,
    closes: Arc<AtomicUsize>,
}
struct Cancel(Arc<AtomicBool>);
impl Cancellation for Cancel {
    fn cancel(&self) -> Result<()> {
        self.0.store(true, Ordering::SeqCst);
        Ok(())
    }
}
impl Connector for Fixture {
    fn connect(&self, _: &Profile, _: Zeroizing<String>) -> Result<Box<dyn Session>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeSession {
            fetches: self.fetches.clone(),
            second_fetch: self.second_fetch.clone(),
            fail_fetch: self.fail_fetch,
            preview_offset: None,
            total_rows: if self.total_rows == 0 {
                1250
            } else {
                self.total_rows
            },
            value_bytes: self.value_bytes,
            max_batch: self.max_batch,
            requested: self.requested.clone(),
            offset: 0,
            slow: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            closes: self.closes.clone(),
        }))
    }
}
impl Session for FakeSession {
    fn execute_keep_alive(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.preview_offset = Some(self.offset);
        self.execute(sql)
    }
    fn close_keep_alive(&mut self) -> Result<()> {
        self.offset = self.preview_offset.take().unwrap();
        self.close_operation()
    }
    fn execute(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.offset = 0;
        self.slow = sql == "slow";
        self.cancelled.store(false, Ordering::SeqCst);
        if sql == "broken" {
            return Err(qrow::connector::QueryError("syntax error".into()).into());
        }
        Ok(Arc::new(Cancel(self.cancelled.clone())))
    }
    fn execute_metadata(&mut self, _: &MetadataRequest) -> Result<Arc<dyn Cancellation>> {
        unreachable!("tab workers do not read the catalog")
    }
    fn poll(&mut self) -> Result<QueryState> {
        Ok(if self.cancelled.load(Ordering::SeqCst) {
            QueryState::Cancelled
        } else if self.slow {
            QueryState::Running
        } else {
            QueryState::Finished { has_results: true }
        })
    }
    fn columns(&mut self) -> Result<Vec<Column>> {
        Ok(vec![Column {
            name: "n".into(),
            data_type: "INT".into(),
        }])
    }
    fn fetch(&mut self, count: usize) -> Result<Batch> {
        self.requested.lock().unwrap().push(count);
        let count = if self.max_batch == 0 {
            count
        } else {
            count.min(self.max_batch)
        };
        let fetch = self.fetches.fetch_add(1, Ordering::SeqCst);
        anyhow::ensure!(self.fail_fetch != Some(fetch), "Fetch transport failed");
        if fetch == 1
            && let Some(barrier) = &self.second_fetch
        {
            barrier.wait();
            barrier.wait();
        }
        let end = (self.offset + count).min(self.total_rows);
        let rows: Vec<_> = (self.offset..end)
            .map(|n| {
                vec![Some(if self.value_bytes == 0 {
                    n.to_string()
                } else {
                    "x".repeat(self.value_bytes)
                })]
            })
            .collect();
        self.offset = end;
        Ok(Batch { rows })
    }
    fn close_operation(&mut self) -> Result<()> {
        Ok(())
    }
    fn close(&mut self) -> Result<()> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn worker(fixture: Arc<Fixture>) -> Worker {
    Worker::with_connector(
        Arc::new(|| {}),
        fixture,
        Arc::new(|_| Ok(Zeroizing::new(String::new()))),
    )
}
fn next(worker: &Worker) -> Event {
    worker
        .events
        .recv_timeout(Duration::from_secs(3))
        .expect("worker stalled")
}
fn ready(worker: &Worker) -> (usize, bool) {
    let mut rows = 0;
    loop {
        match next(worker) {
            Event::Rows(batch) => rows += batch.len(),
            Event::Ready { more, .. } => return (rows, more),
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
}

#[test]
fn preview_is_bounded_and_session_reused_until_profile_changes() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let profile = Profile::default();
    worker.run(profile.clone(), "select".into());
    assert_eq!(ready(&worker), (1000, true));
    worker.more();
    assert_eq!(ready(&worker), (250, false));
    worker.run(profile.clone(), "select".into());
    assert_eq!(ready(&worker), (1000, true));
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
    let mut other = profile;
    other.username = "other-size".into();
    worker.run(other, "select".into());
    assert_eq!(ready(&worker), (1000, true));
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 1);
    worker.disconnect();
    loop {
        if matches!(next(&worker), Event::Disconnected) {
            break;
        }
    }
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 2);
}

#[test]
fn live_profile_update_preserves_the_session_and_unfetched_rows() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let profile = Profile::default();
    worker.run(profile.clone(), "select".into());
    assert_eq!(ready(&worker), (1000, true));

    let mut updated = profile;
    updated.name = "Renamed".into();
    updated.lifecycle.idle_seconds = 60;
    worker.update_profile(updated).unwrap();
    worker.more();

    assert_eq!(ready(&worker), (250, false));
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 0);
}

#[test]
fn running_with_only_metadata_changes_reuses_the_session() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let profile = Profile::default();
    worker.run(profile.clone(), "select".into());
    assert_eq!(ready(&worker), (1000, true));

    let mut updated = profile;
    updated.name = "Renamed".into();
    updated.lifecycle.keep_alive_seconds = 60;
    updated.lifecycle.keep_alive_sql = "SELECT 2".into();
    worker.run(updated, "select".into());

    assert_eq!(ready(&worker), (1000, true));
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 0);
}

#[test]
fn lifecycle_update_recomputes_the_heartbeat_deadline() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let mut profile = Profile::default();
    profile.lifecycle.keep_alive_seconds = 60;
    profile.lifecycle.keep_alive_sql = "SELECT old".into();
    worker.run(profile.clone(), "select".into());
    ready(&worker);

    let mut updated = profile;
    updated.lifecycle.keep_alive_seconds = 1;
    updated.lifecycle.keep_alive_sql = "SELECT new".into();
    worker.update_profile(updated).unwrap();
    assert!(matches!(next(&worker), Event::KeepAliveStarted));
    assert!(matches!(next(&worker), Event::KeepAliveFinished));

    let logs: Vec<_> = worker.logs.try_iter().collect();
    assert!(logs.iter().any(|event| {
        event.kind == LogKind::KeepAliveCompleted && event.sql.as_deref() == Some("SELECT new")
    }));
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 0);
}

#[test]
fn switching_to_disconnect_starts_a_new_idle_timeout() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let mut profile = Profile::default();
    profile.lifecycle.keep_alive_seconds = 60;
    worker.run(profile.clone(), "select".into());
    ready(&worker);

    let mut updated = profile;
    updated.lifecycle.keep_alive_seconds = 0;
    updated.lifecycle.idle_seconds = 1;
    worker.update_profile(updated).unwrap();
    assert!(
        worker
            .events
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    assert!(matches!(next(&worker), Event::IdleDisconnected));
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn invalid_or_stale_profile_updates_do_not_change_the_session() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let profile = Profile::default();
    worker.run(profile.clone(), "select".into());
    assert_eq!(ready(&worker), (1000, true));

    let mut invalid = profile.clone();
    invalid.lifecycle.keep_alive_seconds = 1;
    invalid.lifecycle.keep_alive_sql = "SELECT 1; SELECT 2".into();
    assert!(worker.update_profile(invalid).is_err());
    worker.more();
    assert_eq!(ready(&worker), (250, false));

    let mut stale = profile.clone();
    stale.username = "other-user".into();
    stale.name = "Wrong profile".into();
    worker.update_profile(stale).unwrap();
    worker.run(profile, "select".into());
    assert_eq!(ready(&worker), (1000, true));
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 0);
}

#[test]
fn profile_update_before_connect_does_not_open_a_session() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let profile = Profile {
        name: "Updated before connect".into(),
        ..Profile::default()
    };
    worker.update_profile(profile).unwrap();
    assert!(
        worker
            .events
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 0);
}

#[test]
fn concurrent_tabs_and_cancellation_do_not_block_each_other() {
    let fixture = Arc::new(Fixture::default());
    let slow = worker(fixture.clone());
    let fast = worker(fixture.clone());
    slow.run(Profile::default(), "slow".into());
    loop {
        if matches!(next(&slow), Event::Running) {
            break;
        }
    }
    fast.run(Profile::default(), "select".into());
    assert_eq!(ready(&fast), (1000, true));
    slow.cancel();
    loop {
        match next(&slow) {
            Event::Cancelled => break,
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 2);
}

#[test]
fn sql_error_does_not_destroy_the_session() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let profile = Profile::default();
    worker.run(profile.clone(), "broken".into());
    loop {
        if let Event::Error { disconnected, .. } = next(&worker) {
            assert!(!disconnected);
            break;
        }
    }
    worker.run(profile, "select".into());
    assert_eq!(ready(&worker), (1000, true));
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
}

#[test]
fn repeated_previews_stop_at_the_row_cap() {
    use qrow::model::{MAX_RESULT_ROWS, PREVIEW_ROWS};
    let fixture = Arc::new(Fixture {
        total_rows: MAX_RESULT_ROWS + 1000,
        ..Default::default()
    });
    let worker = worker(fixture);
    worker.run(Profile::default(), "rows".into());
    let mut total = 0;
    loop {
        match next(&worker) {
            Event::Rows(rows) => {
                total += rows.len();
                assert!(rows.len() <= PREVIEW_ROWS);
                assert!(total <= MAX_RESULT_ROWS);
            }
            Event::Ready {
                limited: true,
                more,
            } => {
                assert!(!more);
                break;
            }
            Event::Ready { more: true, .. } => {
                assert!(total.is_multiple_of(PREVIEW_ROWS));
                worker.more();
            }
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert_eq!(total, MAX_RESULT_ROWS);
}

#[test]
fn oversized_preview_batches_are_discarded_at_the_memory_cap() {
    use qrow::model::MAX_RESULT_BYTES;
    let fixture = Arc::new(Fixture {
        total_rows: 10_000,
        value_bytes: 16 * 1024,
        ..Default::default()
    });
    let worker = worker(fixture);
    worker.run(Profile::default(), "bytes".into());
    let mut retained_bytes = 0;
    loop {
        match next(&worker) {
            Event::Rows(rows) => {
                retained_bytes += rows
                    .iter()
                    .map(|row| {
                        row.capacity() * std::mem::size_of::<Option<String>>()
                            + row.iter().flatten().map(String::capacity).sum::<usize>()
                    })
                    .sum::<usize>();
                assert!(retained_bytes <= MAX_RESULT_BYTES);
            }
            Event::Ready {
                limited: true,
                more,
            } => {
                assert!(!more);
                break;
            }
            Event::Ready { more: true, .. } => worker.more(),
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert!(retained_bytes > MAX_RESULT_BYTES / 2);
}

#[test]
fn idle_disconnect_releases_session_and_next_run_reconnects() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let mut profile = Profile::default();
    profile.lifecycle.idle_seconds = 1;
    worker.run(profile.clone(), "select".into());
    assert_eq!(ready(&worker), (1000, true));
    assert!(matches!(next(&worker), Event::IdleDisconnected));
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 1);
    assert!(
        worker
            .events
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    worker.run(profile, "select".into());
    assert_eq!(ready(&worker), (1000, true));
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 2);
}

#[test]
fn keep_alive_overrides_idle_disconnect_and_stops_after_manual_disconnect() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let mut profile = Profile::default();
    profile.lifecycle.idle_seconds = 1;
    profile.lifecycle.keep_alive_seconds = 1;
    profile.lifecycle.keep_alive_sql = "SELECT 'keep-alive λ';".into();
    let keep_alive_sql = profile.lifecycle.keep_alive_sql.clone();
    let execution = worker.run(profile, "select".into());
    assert_eq!(ready(&worker), (1000, true));
    let query_logs: Vec<_> = worker.logs.try_iter().collect();
    assert!(
        query_logs
            .iter()
            .all(|event| event.execution_id == Some(execution))
    );
    for _ in 0..2 {
        assert!(matches!(next(&worker), Event::KeepAliveStarted));
        assert!(matches!(next(&worker), Event::KeepAliveFinished));
        let logs: Vec<_> = worker.logs.try_iter().collect();
        assert_eq!(logs.len(), 1, "Log the outcome of each keep-alive");
        assert_eq!(logs[0].kind, LogKind::KeepAliveCompleted);
        assert!(logs[0].text.starts_with("Keep-alive completed"));
        assert_eq!(logs[0].sql.as_deref(), Some(keep_alive_sql.as_str()));
        assert!(logs[0].duration.is_some());
        assert!(logs.iter().all(|event| {
            event.execution_id.is_none()
                && event.severity == Severity::Info
                && event.kind != LogKind::ExecutionCompleted
        }));
    }
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 0);
    worker.more();
    assert_eq!(ready(&worker), (250, false));
    assert!(
        worker
            .logs
            .try_iter()
            .all(|event| event.execution_id == Some(execution))
    );
    worker.disconnect();
    assert!(matches!(next(&worker), Event::Disconnected));
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 1);
    assert!(
        worker
            .events
            .recv_timeout(Duration::from_millis(1200))
            .is_err()
    );
}

#[test]
fn failed_keep_alive_disconnects_without_background_retry() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let mut profile = Profile::default();
    profile.lifecycle.keep_alive_seconds = 1;
    profile.lifecycle.keep_alive_sql = "broken".into();
    worker.run(profile, "select".into());
    ready(&worker);
    worker.logs.try_iter().for_each(drop);
    assert!(matches!(next(&worker), Event::KeepAliveStarted));
    match next(&worker) {
        Event::Error {
            disconnected,
            message,
        } => {
            assert!(disconnected);
            assert!(message.starts_with("Keep-alive failed:"));
        }
        _ => panic!("Expected keep-alive failure"),
    }
    let logs: Vec<_> = worker.logs.try_iter().collect();
    assert_eq!(logs.len(), 1, "Log the failed keep-alive with its SQL");
    assert_eq!(logs[0].kind, LogKind::KeepAliveFailed);
    assert_eq!(logs[0].severity, Severity::Error);
    assert!(
        logs[0]
            .text
            .starts_with("Keep-alive failed and closed the session: syntax error")
    );
    assert_eq!(logs[0].sql.as_deref(), Some("broken"));
    assert!(logs[0].duration.is_some());
    assert!(logs.iter().all(|event| event.execution_id.is_none()));
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 1);
    assert!(
        worker
            .events
            .recv_timeout(Duration::from_millis(1200))
            .is_err()
    );
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
}

#[test]
fn active_queries_are_not_interrupted_by_idle_timeout_or_heartbeat() {
    for keep_alive_seconds in [0, 1] {
        let fixture = Arc::new(Fixture::default());
        let worker = worker(fixture.clone());
        let mut profile = Profile::default();
        profile.lifecycle.idle_seconds = 1;
        profile.lifecycle.keep_alive_seconds = keep_alive_seconds;
        worker.run(profile, "slow".into());
        while !matches!(next(&worker), Event::Running) {}
        assert!(
            worker
                .events
                .recv_timeout(Duration::from_millis(1200))
                .is_err()
        );
        assert_eq!(fixture.closes.load(Ordering::SeqCst), 0);
        worker.cancel();
        assert!(matches!(next(&worker), Event::Cancelled));
    }
}

#[test]
fn a_refresh_defers_idle_disconnect_without_restarting_the_timer() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let refreshing = Arc::new(AtomicBool::new(true));
    let active = refreshing.clone();
    worker.set_idle_guard(Some(Arc::new(move || {
        active.load(Ordering::SeqCst).then_some(1)
    })));
    let mut profile = Profile::default();
    profile.lifecycle.idle_seconds = 1;
    worker.run(profile, "select".into());
    ready(&worker);
    assert!(
        worker
            .events
            .recv_timeout(Duration::from_millis(1200))
            .is_err()
    );
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 0);

    refreshing.store(false, Ordering::SeqCst);
    assert!(matches!(
        worker
            .events
            .recv_timeout(Duration::from_millis(500))
            .unwrap(),
        Event::IdleDisconnected
    ));
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn ending_a_refresh_before_the_idle_deadline_preserves_the_remaining_time() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture);
    let refreshing = Arc::new(AtomicBool::new(true));
    let active = refreshing;
    worker.set_idle_guard(Some(Arc::new(move || {
        active.load(Ordering::SeqCst).then_some(1)
    })));
    let mut profile = Profile::default();
    profile.lifecycle.idle_seconds = 1;
    worker.run(profile, "select".into());
    ready(&worker);
    assert!(
        worker
            .events
            .recv_timeout(Duration::from_millis(600))
            .is_err()
    );
    // Rebinding protection, as the UI does, must not restart the timer either.
    worker.set_idle_guard(None);
    assert!(
        worker
            .events
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    assert!(matches!(
        worker
            .events
            .recv_timeout(Duration::from_millis(550))
            .unwrap(),
        Event::IdleDisconnected
    ));
}

#[test]
fn explicit_disconnect_and_shutdown_do_not_wait_for_a_refresh() {
    for shutdown in [false, true] {
        let fixture = Arc::new(Fixture::default());
        let worker = worker(fixture.clone());
        worker.set_idle_guard(Some(Arc::new(|| Some(1))));
        worker.run(Profile::default(), "select".into());
        ready(&worker);
        if shutdown {
            worker.shutdown();
            worker.wait_for_shutdown(Duration::from_secs(1));
        } else {
            worker.disconnect();
            assert!(matches!(next(&worker), Event::Disconnected));
        }
        assert_eq!(fixture.closes.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn a_refresh_does_not_defer_keep_alive_queries() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture);
    worker.set_idle_guard(Some(Arc::new(|| Some(1))));
    let mut profile = Profile::default();
    profile.lifecycle.keep_alive_seconds = 1;
    worker.run(profile, "select".into());
    ready(&worker);
    assert!(matches!(next(&worker), Event::KeepAliveStarted));
    assert!(matches!(next(&worker), Event::KeepAliveFinished));
}

#[test]
fn a_later_refresh_does_not_extend_an_overdue_idle_session() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let batch = Arc::new(AtomicU64::new(1));
    let active = batch.clone();
    worker.set_idle_guard(Some(Arc::new(move || Some(active.load(Ordering::SeqCst)))));
    let mut profile = Profile::default();
    profile.lifecycle.idle_seconds = 1;
    worker.run(profile, "select".into());
    ready(&worker);
    assert!(
        worker
            .events
            .recv_timeout(Duration::from_millis(1200))
            .is_err()
    );
    // An overlong automatic refresh can finish after its next period is due.
    // The next batch then starts without a gap in refresh activity.
    batch.store(2, Ordering::SeqCst);
    assert!(matches!(
        worker
            .events
            .recv_timeout(Duration::from_millis(500))
            .unwrap(),
        Event::IdleDisconnected
    ));
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn shutdown_cancels_a_running_heartbeat_and_releases_the_session() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let mut profile = Profile::default();
    profile.lifecycle.keep_alive_seconds = 1;
    profile.lifecycle.keep_alive_sql = "slow".into();
    worker.run(profile, "select".into());
    ready(&worker);
    assert!(matches!(next(&worker), Event::KeepAliveStarted));
    worker.shutdown();
    worker.wait_for_shutdown(Duration::from_secs(3));
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn cancelling_a_query_does_not_disable_future_heartbeats() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture);
    let mut profile = Profile::default();
    profile.lifecycle.keep_alive_seconds = 1;
    worker.run(profile, "slow".into());
    while !matches!(next(&worker), Event::Running) {}
    worker.cancel();
    assert!(matches!(next(&worker), Event::Cancelled));
    assert!(matches!(next(&worker), Event::KeepAliveStarted));
    assert!(matches!(next(&worker), Event::KeepAliveFinished));
}

#[test]
fn pages_fetch_only_on_demand_and_confirm_exhaustion_with_an_empty_fetch() {
    let fixture = Arc::new(Fixture {
        total_rows: 2000,
        ..Default::default()
    });
    let worker = worker(fixture.clone());
    worker.run(Profile::default(), "select".into());
    let mut values = Vec::new();
    for page in 0..2 {
        loop {
            match next(&worker) {
                Event::Rows(rows) => {
                    values.extend(rows.into_iter().map(|row| row[0].clone().unwrap()))
                }
                Event::Ready { more, limited } => {
                    assert!(more);
                    assert!(!limited);
                    break;
                }
                Event::Error { message, .. } => panic!("{message}"),
                _ => {}
            }
        }
        assert_eq!(fixture.fetches.load(Ordering::SeqCst), page + 1);
        assert!(
            worker
                .events
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );
        worker.more();
    }
    assert_eq!(ready(&worker), (0, false));
    // Each page is one request for all its rows.
    assert_eq!(*fixture.requested.lock().unwrap(), [1000, 1000, 1000]);
    assert_eq!(values, (0..2000).map(|n| n.to_string()).collect::<Vec<_>>());
}

#[test]
fn rows_stream_before_the_page_finishes_and_cancel_discards_a_late_fetch() {
    let barrier = Arc::new(Barrier::new(2));
    let fixture = Arc::new(Fixture {
        second_fetch: Some(barrier.clone()),
        max_batch: 250,
        ..Default::default()
    });
    let worker = worker(fixture.clone());
    worker.run(Profile::default(), "select".into());
    loop {
        match next(&worker) {
            Event::Rows(rows) => {
                assert_eq!(rows.len(), 250);
                break;
            }
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    barrier.wait();
    worker.cancel();
    barrier.wait();
    assert!(matches!(next(&worker), Event::Cancelled));
    assert_eq!(fixture.fetches.load(Ordering::SeqCst), 2);
    // A short batch leaves the rest of the page for the next request.
    assert_eq!(*fixture.requested.lock().unwrap(), [1000, 750]);
}

#[test]
fn a_failed_later_page_keeps_delivered_rows_and_never_resubmits_sql() {
    let fixture = Arc::new(Fixture {
        total_rows: 2500,
        fail_fetch: Some(5),
        max_batch: 250,
        ..Default::default()
    });
    let worker = worker(fixture.clone());
    worker.run(Profile::default(), "select".into());
    assert_eq!(ready(&worker), (1000, true));
    worker.more();
    match next(&worker) {
        Event::Rows(rows) => {
            assert_eq!(rows.len(), 250);
            assert_eq!(rows[0][0].as_deref(), Some("1000"));
        }
        _ => panic!("Expected the first batch of the second page"),
    }
    assert!(matches!(
        next(&worker),
        Event::Error {
            disconnected: true,
            ..
        }
    ));
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 1);
    assert!(
        worker
            .events
            .recv_timeout(Duration::from_millis(50))
            .is_err()
    );
    let logs: Vec<_> = worker.logs.try_iter().collect();
    let execution_outcomes: Vec<_> = logs
        .iter()
        .filter(|event| event.kind == LogKind::ExecutionCompleted)
        .collect();
    assert_eq!(execution_outcomes.len(), 1);
    assert!(
        execution_outcomes[0]
            .text
            .contains("Execution completed on the server")
    );
    assert!(logs.iter().any(|event| {
        event.kind == LogKind::Error && event.text.contains("Fetch transport failed")
    }));
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
}

#[test]
fn log_events_keep_execution_identity_and_page_measurements() {
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture);
    let execution = worker.run(Profile::default(), "select".into());
    assert_eq!(ready(&worker), (1000, true));

    let logs: Vec<_> = worker.logs.try_iter().collect();
    assert!(logs.iter().any(|event| {
        event.execution_id == Some(execution) && event.kind == LogKind::Connected
    }));
    assert!(logs.iter().any(|event| {
        event.execution_id == Some(execution) && event.kind == LogKind::ExecutionCompleted
    }));
    assert!(logs.iter().any(|event| {
        event.execution_id == Some(execution) && event.kind == LogKind::FetchStarted
    }));
    assert!(logs.iter().any(|event| {
        event.execution_id == Some(execution)
            && event.kind == LogKind::FetchCompleted
            && event.duration.is_some()
    }));
    assert!(logs.iter().all(|event| event.severity == Severity::Info));
}
