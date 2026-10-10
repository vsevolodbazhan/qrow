use anyhow::Result;
use qrow::{
    connector::{Cancellation, Connector, MetadataRequest, QueryState, Secret, Session},
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

#[derive(Default)]
struct Fixture {
    cancels: Arc<AtomicUsize>,
    executions: Arc<Mutex<Vec<String>>>,
    connects: AtomicUsize,
    fetches: Arc<AtomicUsize>,
    second_fetch: Option<Arc<Barrier>>,
    fail_fetch: Option<usize>,
    total_rows: usize,
    empty: bool,
    no_result: bool,
    value_bytes: usize,
    /// The most rows that one fetch returns, like a server limit. 0 has no limit.
    max_batch: usize,
    requested: Arc<Mutex<Vec<usize>>>,
    closes: Arc<AtomicUsize>,
    adaptive: bool,
    transfers: Arc<Mutex<Vec<qrow::model::transfer::Transfer>>>,
}
struct FakeSession {
    cancels: Arc<AtomicUsize>,
    executions: Arc<Mutex<Vec<String>>>,
    fetches: Arc<AtomicUsize>,
    second_fetch: Option<Arc<Barrier>>,
    fail_fetch: Option<usize>,
    preview_offset: Option<usize>,
    total_rows: usize,
    no_result: bool,
    value_bytes: usize,
    max_batch: usize,
    requested: Arc<Mutex<Vec<usize>>>,
    offset: usize,
    slow: bool,
    progress_polls: Option<usize>,
    cancelled: Arc<AtomicBool>,
    closes: Arc<AtomicUsize>,
    adaptive: bool,
    transfers: Arc<Mutex<Vec<qrow::model::transfer::Transfer>>>,
    transfer: qrow::model::transfer::Transfer,
}
struct Cancel(Arc<AtomicBool>, Arc<AtomicUsize>);
impl Cancellation for Cancel {
    fn cancel(&self) -> Result<()> {
        self.0.store(true, Ordering::SeqCst);
        self.1.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
impl Connector for Fixture {
    fn connect(&self, _: &Profile, _: Secret) -> Result<Box<dyn Session>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeSession {
            cancels: self.cancels.clone(),
            executions: self.executions.clone(),
            fetches: self.fetches.clone(),
            second_fetch: self.second_fetch.clone(),
            fail_fetch: self.fail_fetch,
            preview_offset: None,
            total_rows: if self.empty {
                0
            } else if self.total_rows == 0 {
                1250
            } else {
                self.total_rows
            },
            value_bytes: self.value_bytes,
            no_result: self.no_result,
            max_batch: self.max_batch,
            requested: self.requested.clone(),
            offset: 0,
            slow: false,
            progress_polls: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            closes: self.closes.clone(),
            adaptive: self.adaptive,
            transfers: self.transfers.clone(),
            transfer: Default::default(),
        }))
    }
}
impl Session for FakeSession {
    fn configure_export(&mut self, transfer: &qrow::model::transfer::Transfer) -> Result<()> {
        transfer.validate()?;
        self.transfer = transfer.clone();
        self.transfers.lock().unwrap().push(transfer.clone());
        Ok(())
    }
    fn export_fetch_rows(&self) -> usize {
        if self.adaptive {
            self.transfer.settings().request_rows(1024)
        } else {
            1000
        }
    }
    fn execute_keep_alive(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.preview_offset = Some(self.offset);
        self.execute(sql)
    }
    fn close_keep_alive(&mut self) -> Result<()> {
        self.offset = self.preview_offset.take().unwrap();
        self.close_operation()
    }
    fn execute(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.executions.lock().unwrap().push(sql.into());
        self.offset = 0;
        self.slow = sql == "slow";
        self.progress_polls = (sql == "SELECT progress").then_some(0);
        self.cancelled.store(false, Ordering::SeqCst);
        if sql == "broken" {
            return Err(qrow::connector::QueryError("syntax error".into()).into());
        }
        Ok(Arc::new(Cancel(
            self.cancelled.clone(),
            self.cancels.clone(),
        )))
    }
    fn execute_metadata(&mut self, _: &MetadataRequest) -> Result<Arc<dyn Cancellation>> {
        unreachable!("tab workers do not read the catalog")
    }
    fn poll(&mut self) -> Result<QueryState> {
        if let Some(polls) = self.progress_polls.as_mut() {
            *polls += 1;
            if *polls < 3 && !self.cancelled.load(Ordering::SeqCst) {
                return Ok(QueryState::Running);
            }
        }
        Ok(if self.cancelled.load(Ordering::SeqCst) {
            QueryState::Cancelled
        } else if self.slow {
            QueryState::Running
        } else {
            QueryState::Finished {
                has_results: !self.no_result,
            }
        })
    }
    fn progress_percentage(&self) -> Option<f64> {
        self.progress_polls
            .map(|polls| (polls as f64 * 40.).min(100.))
    }
    fn columns(&mut self) -> Result<Vec<Column>> {
        Ok(vec![Column {
            name: "n".into(),
            data_type: if self.value_bytes == 0 {
                "INT"
            } else {
                "STRING"
            }
            .into(),
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
        Arc::new(|_| Ok(Secret::password(""))),
    )
}

struct PausedNotifications {
    notified: std::sync::mpsc::Receiver<()>,
    resume: std::sync::mpsc::Sender<()>,
}

impl PausedNotifications {
    fn terminal(&self, worker: &Worker) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            self.notified
                .recv_timeout(Duration::from_secs(3))
                .expect("Worker notification stalled");
            events.extend(worker.events.try_iter());
            for _ in worker.logs.try_iter() {}
            if events.iter().any(|event| {
                matches!(
                    event,
                    Event::Downloaded { .. } | Event::DownloadFailed { .. } | Event::Ready { .. }
                )
            }) {
                return events;
            }
            self.resume.send(()).unwrap();
        }
    }

    fn resume(&self) {
        self.resume.send(()).unwrap();
    }
}

fn paused_worker(fixture: Arc<Fixture>) -> (Worker, PausedNotifications) {
    let (wake, notified) = std::sync::mpsc::channel();
    let (resume, continued) = std::sync::mpsc::channel();
    let continued = Mutex::new(continued);
    let worker = Worker::with_connector(
        Arc::new(move || {
            if wake.send(()).is_ok() {
                let _ = continued
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(3));
            }
        }),
        fixture,
        Arc::new(|_| Ok(Secret::password("synthetic"))),
    );
    (worker, PausedNotifications { notified, resume })
}

#[test]
fn terminal_export_notifications_release_the_producer_before_immediate_admission() {
    use qrow::{export, logs::ExecutionId};
    for outcome in ["rows", "command", "failure"] {
        let fixture = Arc::new(Fixture {
            total_rows: 4,
            no_result: outcome == "command",
            fail_fetch: (outcome == "failure").then_some(0),
            ..Default::default()
        });
        let (worker, notifications) = paused_worker(fixture.clone());
        let profile = export_profile();
        let jobs = export::Jobs::default();
        let flag = Arc::new(AtomicBool::new(false));
        let publication = jobs
            .register_for_profile(flag.clone(), None, profile.id, 1)
            .unwrap();
        worker
            .run_and_export(
                profile.clone(),
                "SELECT first".into(),
                ExecutionId(401),
                None,
                &jobs,
                flag,
            )
            .unwrap();
        let events = notifications.terminal(&worker);
        assert!(events.iter().any(|event| match outcome {
            "rows" => matches!(event, Event::Downloaded { .. }),
            "command" => matches!(event, Event::Ready { .. }),
            _ => matches!(event, Event::DownloadFailed { .. }),
        }));
        assert_eq!(
            jobs.active_count(),
            1,
            "The unpublished writer must hold admission"
        );
        drop(publication);
        assert_eq!(
            jobs.active_count(),
            0,
            "The producer must release admission before notification"
        );
        worker
            .run_and_export(
                profile,
                "SELECT immediate".into(),
                ExecutionId(402),
                None,
                &jobs,
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        notifications.resume();
        let events = notifications.terminal(&worker);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::DownloadFailed { .. }))
        );
        assert_eq!(jobs.active_count(), 0);
        assert_eq!(
            *fixture.executions.lock().unwrap(),
            ["SELECT first", "SELECT immediate"]
        );
        notifications.resume();
        drop(notifications);
        worker.shutdown();
        worker.wait_for_shutdown(Duration::from_secs(3));
    }
}

#[test]
fn terminal_cursor_drain_notifications_release_the_producer_before_immediate_admission() {
    use qrow::{export, logs::ExecutionId};
    for outcome in ["rows", "stale", "failure"] {
        let fixture = Arc::new(Fixture {
            total_rows: 1250,
            fail_fetch: (outcome == "failure").then_some(1),
            ..Default::default()
        });
        let (worker, notifications) = paused_worker(fixture.clone());
        let profile = export_profile();
        let execution = worker.run(profile.clone(), "SELECT preview".into());
        let events = notifications.terminal(&worker);
        let mut columns = Vec::new();
        let mut rows = export::Rows::default();
        for event in events {
            match event {
                Event::Columns(value) => columns = value,
                Event::Rows(value) if outcome != "stale" => rows.extend(value),
                Event::Ready { more, .. } => assert!(more),
                _ => {}
            }
        }
        let jobs = export::Jobs::default();
        let flag = Arc::new(AtomicBool::new(false));
        let publication = jobs
            .register_for_profile(flag.clone(), None, profile.id, 1)
            .unwrap();
        worker
            .drain(
                execution,
                Arc::new(export::Snapshot::new(&columns, &rows).unwrap()),
                &jobs,
                flag,
            )
            .unwrap();
        notifications.resume();
        let events = notifications.terminal(&worker);
        assert!(events.iter().any(|event| if outcome == "rows" {
            matches!(event, Event::Downloaded { .. })
        } else {
            matches!(event, Event::DownloadFailed { .. })
        }));
        assert_eq!(jobs.active_count(), 1);
        drop(publication);
        assert_eq!(jobs.active_count(), 0);
        worker
            .run_and_export(
                profile,
                "SELECT immediate".into(),
                ExecutionId(403),
                None,
                &jobs,
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        notifications.resume();
        let events = notifications.terminal(&worker);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::Downloaded { .. }))
        );
        assert_eq!(jobs.active_count(), 0);
        assert_eq!(
            *fixture.executions.lock().unwrap(),
            ["SELECT preview", "SELECT immediate"]
        );
        notifications.resume();
        drop(notifications);
        worker.shutdown();
        worker.wait_for_shutdown(Duration::from_secs(3));
    }
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

fn snapshot_page(
    worker: &Worker,
    columns: &mut Vec<Column>,
    rows: &mut qrow::export::Rows,
) -> bool {
    loop {
        match next(worker) {
            Event::Columns(value) => *columns = value,
            Event::ExportContext(context) => rows.set_context(context),
            Event::Rows(batch) => rows.extend(batch),
            Event::Ready { more, .. } => return more,
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
}

fn wait_download(worker: &Worker) -> Arc<qrow::export::spool::Spool> {
    loop {
        match worker
            .events
            .recv_timeout(Duration::from_secs(30))
            .expect("download stalled")
        {
            Event::Downloaded { spool, .. } => return spool,
            Event::DownloadFailed { message, .. } | Event::Error { message, .. } => {
                panic!("{message}")
            }
            _ => {}
        }
    }
}

#[test]
fn all_rows_keeps_pending_overflow_and_replays_without_sql() {
    use qrow::{
        export::{self, Snapshot},
        model::MAX_RESULT_ROWS,
    };
    let fixture = Arc::new(Fixture {
        total_rows: MAX_RESULT_ROWS + 123,
        max_batch: 333,
        ..Default::default()
    });
    let worker = worker(fixture.clone());
    let profile = Profile::default();
    let execution = worker.run(profile.clone(), "select".into());
    let mut columns = Vec::new();
    let mut rows = export::Rows::default();
    while snapshot_page(&worker, &mut columns, &mut rows) {
        worker.more();
    }
    assert_eq!(rows.len(), MAX_RESULT_ROWS);
    let source = Arc::new(Snapshot::new(&columns, &rows).unwrap());
    let jobs = export::Jobs::default();
    let download = worker
        .drain(execution, source, &jobs, Arc::new(AtomicBool::new(false)))
        .unwrap();
    let spool = wait_download(&worker);
    assert_eq!(
        spool.status(),
        export::spool::Status::Complete {
            rows: (MAX_RESULT_ROWS + 123) as u64
        }
    );
    let mut text = Vec::new();
    export::stream::write(
        &mut text,
        &spool,
        &export::Settings::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut csv = csv::Reader::from_reader(text.as_slice());
    for (index, record) in csv.records().enumerate() {
        assert_eq!(&record.unwrap()[0], index.to_string());
    }
    let fetches = fixture.fetches.load(Ordering::SeqCst);
    let mut settings = export::Settings::default();
    settings.format = export::Format::JsonLines;
    settings.json.typed = false;
    let mut text = Vec::new();
    assert_eq!(
        export::stream::write(&mut text, &spool, &settings, &AtomicBool::new(false)).unwrap(),
        MAX_RESULT_ROWS + 123
    );
    assert_eq!(fixture.fetches.load(Ordering::SeqCst), fetches);
    assert_eq!(*fixture.executions.lock().unwrap(), ["select"]);
    // An old writer failure cannot cancel a later query in the same session.
    worker.run(profile, "slow".into());
    while !matches!(next(&worker), Event::Running) {}
    download.fail("late writer failure".into());
    assert_eq!(fixture.cancels.load(Ordering::SeqCst), 0);
    worker.cancel();
    while !matches!(next(&worker), Event::Cancelled) {}
}

#[test]
fn a_stale_prefix_or_execution_is_rejected_before_fetching_or_cancelling() {
    use qrow::export::{self, Snapshot};
    let fixture = Arc::new(Fixture {
        total_rows: 2500,
        ..Default::default()
    });
    let worker = worker(fixture.clone());
    let execution = worker.run(Profile::default(), "select".into());
    let mut columns = Vec::new();
    let mut rows = export::Rows::default();
    assert!(snapshot_page(&worker, &mut columns, &mut rows));
    let old = Arc::new(Snapshot::new(&columns, &rows).unwrap());
    worker.more();
    assert!(snapshot_page(&worker, &mut columns, &mut rows));
    let fetches = fixture.fetches.load(Ordering::SeqCst);
    let jobs = export::Jobs::default();
    let rejected = worker
        .drain(
            execution,
            old.clone(),
            &jobs,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    assert!(matches!(
        next(&worker),
        Event::DownloadFailed {
            consumed: false,
            ..
        }
    ));
    assert!(matches!(
        rejected.spool().unwrap().status(),
        export::spool::Status::Failed(_)
    ));
    assert_eq!(fixture.fetches.load(Ordering::SeqCst), fetches);
    let _new_execution = worker.run(Profile::default(), "select again".into());
    assert_eq!(ready(&worker), (1000, true));
    let fetches = fixture.fetches.load(Ordering::SeqCst);
    worker
        .drain(execution, old, &jobs, Arc::new(AtomicBool::new(false)))
        .unwrap();
    assert!(matches!(
        next(&worker),
        Event::DownloadFailed {
            consumed: false,
            ..
        }
    ));
    assert_eq!(fixture.fetches.load(Ordering::SeqCst), fetches);
    assert_eq!(fixture.cancels.load(Ordering::SeqCst), 0);
}

#[test]
fn cancellation_during_a_blocked_drain_consumes_only_the_old_cursor() {
    use qrow::export::{self, Snapshot};
    let barrier = Arc::new(Barrier::new(2));
    let fixture = Arc::new(Fixture {
        second_fetch: Some(barrier.clone()),
        total_rows: 2500,
        ..Default::default()
    });
    let worker = worker(fixture.clone());
    let profile = Profile::default();
    let execution = worker.run(profile.clone(), "select".into());
    let mut columns = Vec::new();
    let mut rows = export::Rows::default();
    assert!(snapshot_page(&worker, &mut columns, &mut rows));
    let jobs = export::Jobs::default();
    let download = worker
        .drain(
            execution,
            Arc::new(Snapshot::new(&columns, &rows).unwrap()),
            &jobs,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    assert!(matches!(
        next(&worker),
        Event::Cursor {
            state: qrow::worker::Cursor::Draining,
            ..
        }
    ));
    barrier.wait();
    worker.cancel();
    barrier.wait();
    loop {
        match next(&worker) {
            Event::DownloadFailed {
                consumed,
                disconnected,
                ..
            } => {
                assert!(consumed);
                assert!(!disconnected);
                break;
            }
            Event::Downloaded { .. } => panic!("Cancelled download completed"),
            _ => {}
        }
    }
    assert_eq!(
        download.spool().unwrap().status(),
        export::spool::Status::Cancelled
    );
    assert_eq!(fixture.cancels.load(Ordering::SeqCst), 1);
    worker.run(profile, "select again".into());
    assert_eq!(ready(&worker), (1000, true));
    download.fail("old writer failed".into());
    assert_eq!(fixture.cancels.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
}

#[test]
fn byte_limit_overflow_and_a_first_batch_outside_the_preview_are_not_lost() {
    use qrow::export::{self, Snapshot};
    for (count, width) in [(6000, 16 * 1024), (1100, 66 * 1024)] {
        let fixture = Arc::new(Fixture {
            total_rows: count,
            value_bytes: width,
            ..Default::default()
        });
        let worker = worker(fixture.clone());
        let execution = worker.run(Profile::default(), "wide text".into());
        let mut columns = Vec::new();
        let mut rows = export::Rows::default();
        while snapshot_page(&worker, &mut columns, &mut rows) {
            worker.more();
        }
        if width > 64 * 1024 {
            assert!(rows.is_empty());
        }
        let jobs = export::Jobs::default();
        worker
            .drain(
                execution,
                Arc::new(Snapshot::new(&columns, &rows).unwrap()),
                &jobs,
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        let spool = wait_download(&worker);
        let mut reader = spool.reader().unwrap();
        let mut actual = 0;
        while let Some(batch) = reader.next(&AtomicBool::new(false)).unwrap() {
            for row in 0..batch.rows().len() {
                assert_eq!(batch.rows()[row][0].as_ref().unwrap().len(), width);
            }
            actual += batch.rows().len();
        }
        assert_eq!(actual, count);
        assert_eq!(*fixture.executions.lock().unwrap(), ["wide text"]);
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
            ..
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
        while !matches!(next(&worker), Event::Session { .. }) {}
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
    while !matches!(next(&worker), Event::Session { .. }) {}
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

#[test]
fn a_missing_sign_in_fails_before_connecting_and_never_submits_sql() {
    use qrow::oidc::{Failure, SignInError};
    let fixture = Arc::new(Fixture::default());
    let worker = Worker::with_connector(
        Arc::new(|| {}),
        fixture.clone(),
        Arc::new(|_| Err(SignInError::new(Failure::SignInRequired, "Sign in again").into())),
    );
    worker.run(Profile::default(), "select".into());
    loop {
        match next(&worker) {
            Event::Error {
                message,
                disconnected,
                sign_in_required,
            } => {
                assert_eq!(message, "Sign in again");
                assert!(disconnected && sign_in_required);
                break;
            }
            Event::Running | Event::Connected => panic!("SQL must not run"),
            _ => {}
        }
    }
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 0);
}

#[test]
fn each_new_session_asks_for_credentials_and_receives_them() {
    let fixture = Arc::new(Fixture::default());
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    let worker = Worker::with_connector(
        Arc::new(|| {}),
        fixture,
        Arc::new(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(Secret::password("synthetic"))
        }),
    );
    let profile = Profile::default();
    worker.run(profile.clone(), "select".into());
    ready(&worker);
    worker.run(profile.clone(), "select".into());
    ready(&worker);
    assert_eq!(asked.load(Ordering::SeqCst), 1, "a live session is reused");
    worker.disconnect();
    worker.run(profile, "select".into());
    ready(&worker);
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

fn export_profile() -> Profile {
    Profile {
        host: "127.0.0.1".into(),
        username: "synthetic-export".into(),
        ..Default::default()
    }
}

#[test]
fn run_and_export_publishes_query_progress_before_the_schema() {
    use qrow::{export, logs::ExecutionId};
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let download = worker
        .run_and_export(
            export_profile(),
            "SELECT progress".into(),
            ExecutionId(104),
            None,
            &export::Jobs::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    let mut has_columns = false;
    let mut before_schema = Vec::new();
    loop {
        match next(&worker) {
            Event::Columns(_) => has_columns = true,
            Event::DownloadProgress {
                execution,
                rows,
                bytes,
                percentage,
                ..
            } if !has_columns => {
                assert_eq!(execution, ExecutionId(104));
                assert_eq!((rows, bytes), (0, 0));
                before_schema.push(percentage.unwrap());
            }
            Event::Downloaded { spool, .. } => {
                assert_eq!(spool.row_count(), 1250);
                break;
            }
            Event::DownloadFailed { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert_eq!(before_schema, [40., 80., 100.]);
    assert_eq!(download.progress_percentage(), Some(100.));
    assert_eq!(*fixture.executions.lock().unwrap(), ["SELECT progress"]);
}

#[test]
fn run_and_export_has_one_fetch_sequence_and_keeps_only_the_first_page() {
    use qrow::{export, logs::ExecutionId};
    let fixture = Arc::new(Fixture {
        total_rows: 100123,
        max_batch: 333,
        ..Default::default()
    });
    let worker = worker(fixture.clone());
    let profile = export_profile();
    let jobs = export::Jobs::default();
    let flag = Arc::new(AtomicBool::new(false));
    let download = worker
        .run_and_export(
            profile.clone(),
            "SELECT n".into(),
            ExecutionId(101),
            None,
            &jobs,
            flag.clone(),
        )
        .unwrap();
    let writer_guard = jobs.register(flag.clone());
    let source = download.clone();
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("all.csv");
    let output = destination.clone();
    let writer = std::thread::spawn(move || {
        let _guard = writer_guard;
        let spool = source.wait_spool().unwrap();
        export::save(&output, &flag, |out| {
            export::stream::write(out, &spool, &export::Settings::default(), &flag)
        })
        .unwrap()
    });
    let mut preview = export::Rows::default();
    let mut complete = None;
    loop {
        match next(&worker) {
            Event::PreviewRows { rows, .. } => {
                preview.extend(
                    (0..rows.len())
                        .map(|index| rows.get(index).unwrap().clone())
                        .collect(),
                );
            }
            Event::PreviewComplete {
                complete: value, ..
            } => complete = Some(value),
            Event::Downloaded { spool, .. } => {
                assert_eq!(
                    spool.status(),
                    export::spool::Status::Complete { rows: 100123 }
                );
                break;
            }
            Event::DownloadFailed { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert_eq!(preview.len(), 1000);
    assert_eq!(complete, Some(false));
    assert_eq!(preview.get(999).unwrap()[0].as_deref(), Some("999"));
    assert_eq!(writer.join().unwrap(), 100123);
    for (expected, row) in csv::Reader::from_path(destination)
        .unwrap()
        .records()
        .enumerate()
    {
        assert_eq!(&row.unwrap()[0], expected.to_string());
    }
    assert_eq!(*fixture.executions.lock().unwrap(), ["SELECT n"]);
    assert_eq!(jobs.active_count(), 0);
    worker.run(profile, "SELECT newer".into());
    assert_eq!(ready(&worker), (1000, true));
    download.fail("late writer failure".into());
    download.cancel();
    assert_eq!(fixture.cancels.load(Ordering::SeqCst), 0);
}

#[test]
fn run_and_export_completeness_uses_eof_and_session_generation_is_from_execution() {
    use qrow::{export, logs::ExecutionId};
    for total in [1, 1000, 1001] {
        let fixture = Arc::new(Fixture {
            total_rows: total,
            ..Default::default()
        });
        let worker = worker(fixture.clone());
        let profile = export_profile();
        assert!(worker.session_generation(&profile).is_none());
        let download = worker
            .run_and_export(
                profile.clone(),
                "SELECT n".into(),
                ExecutionId(102),
                None,
                &export::Jobs::default(),
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        let mut generation = None;
        let mut rows = 0;
        let mut complete = None;
        loop {
            match next(&worker) {
                Event::Session {
                    generation: value, ..
                } => generation = Some(value),
                Event::PreviewRows { rows: value, .. } => rows += value.len(),
                Event::PreviewComplete {
                    complete: value, ..
                } => complete = Some(value),
                Event::Downloaded { .. } => break,
                Event::DownloadFailed { message, .. } => panic!("{message}"),
                _ => {}
            }
        }
        assert_eq!(rows, total.min(1000));
        assert_eq!(complete, Some(total <= 1000));
        assert_eq!(generation, worker.session_generation(&profile));
        assert!(generation.is_some());
        assert_eq!(download.spool().unwrap().row_count(), total as u64);
        worker.disconnect();
        while !matches!(next(&worker), Event::Disconnected) {}
        assert!(worker.session_generation(&profile).is_none());
        let rejected = worker
            .run_and_export(
                profile,
                "SELECT n".into(),
                ExecutionId(103),
                generation,
                &export::Jobs::default(),
                Arc::new(AtomicBool::new(false)),
            )
            .err()
            .unwrap();
        assert!(
            rejected
                .get_ref()
                .unwrap()
                .is::<qrow::worker::SessionChanged>()
        );
        assert_eq!(*fixture.executions.lock().unwrap(), ["SELECT n"]);
    }
}

#[test]
fn run_and_export_empty_result_has_a_schema_but_a_command_writes_no_file() {
    use qrow::{export, logs::ExecutionId};
    for command in [false, true] {
        let fixture = Arc::new(Fixture {
            empty: true,
            no_result: command,
            ..Default::default()
        });
        let worker = worker(fixture.clone());
        let download = worker
            .run_and_export(
                export_profile(),
                "SELECT n".into(),
                ExecutionId(107),
                None,
                &export::Jobs::default(),
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        if command {
            ready(&worker);
            let message = download.wait_spool().err().unwrap().to_string();
            assert!(message.contains("No export file was written"));
            download.cancel();
            assert_eq!(fixture.cancels.load(Ordering::SeqCst), 0);
        } else {
            wait_download(&worker);
            let spool = download.wait_spool().unwrap();
            let mut output = Vec::new();
            assert_eq!(
                export::stream::write(
                    &mut output,
                    &spool,
                    &export::Settings::default(),
                    &AtomicBool::new(false)
                )
                .unwrap(),
                0
            );
            assert_eq!(output, b"n\r\n");
        }
        assert_eq!(fixture.executions.lock().unwrap().len(), 1);
    }
}

#[test]
fn run_again_reuses_the_expected_session_and_rejects_profile_identity_changes() {
    use qrow::{export, logs::ExecutionId};
    let fixture = Arc::new(Fixture::default());
    let worker = worker(fixture.clone());
    let profile = export_profile();
    worker.run(profile.clone(), "SELECT setup".into());
    ready(&worker);
    let generation = worker.session_generation(&profile).unwrap();
    let mut renamed = profile.clone();
    renamed.name = "Renamed display".into();
    worker
        .run_and_export(
            renamed,
            "SELECT original result SQL".into(),
            ExecutionId(104),
            Some(generation),
            &export::Jobs::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    wait_download(&worker);
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
    let mut changed = profile;
    changed.username = "different-account".into();
    assert!(
        worker
            .run_and_export(
                changed,
                "SELECT must_not_execute".into(),
                ExecutionId(105),
                Some(generation),
                &export::Jobs::default(),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
    );
    assert_eq!(
        *fixture.executions.lock().unwrap(),
        ["SELECT setup", "SELECT original result SQL"]
    );
}

#[test]
fn cancel_and_writer_failure_before_schema_wake_the_writer_without_submitting_sql() {
    use qrow::{export, logs::ExecutionId};
    for fail in [false, true] {
        let fixture = Arc::new(Fixture::default());
        let gate = Arc::new(Barrier::new(2));
        let blocked = gate.clone();
        let worker = Worker::with_connector(
            Arc::new(|| {}),
            fixture.clone(),
            Arc::new(move |_| {
                blocked.wait();
                blocked.wait();
                Ok(Secret::password("synthetic"))
            }),
        );
        let jobs = export::Jobs::default();
        let download = worker
            .run_and_export(
                export_profile(),
                "SELECT n".into(),
                ExecutionId(106),
                None,
                &jobs,
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        gate.wait();
        assert_eq!(jobs.active_count(), 1);
        assert!(download.spool().is_none());
        let source = download.clone();
        let reader = std::thread::spawn(move || source.wait_spool().err().unwrap());
        if fail {
            download.fail("Output failed before schema".into());
        } else {
            worker.cancel();
        }
        let error = reader.join().unwrap();
        if fail {
            assert_eq!(error.to_string(), "Output failed before schema");
        } else {
            assert!(error.get_ref().unwrap().is::<export::Cancelled>());
        }
        gate.wait();
        while !matches!(next(&worker), Event::DownloadFailed { .. }) {}
        assert!(jobs.cancel_and_wait(Duration::from_secs(3)));
        assert_eq!(fixture.connects.load(Ordering::SeqCst), 0);
        assert!(fixture.executions.lock().unwrap().is_empty());
    }
}

#[test]
fn adaptive_exports_reuse_the_session_and_hold_the_profile_limit_until_publication() {
    use qrow::{export, logs::ExecutionId, model::transfer::TransferPreset};
    let fixture = Arc::new(Fixture {
        adaptive: true,
        total_rows: 20_000,
        ..Default::default()
    });
    let worker = worker(fixture.clone());
    let mut profile = export_profile();
    let jobs = export::Jobs::default();
    let cancel = Arc::new(AtomicBool::new(false));
    let publication = jobs
        .register_for_profile(cancel.clone(), None, profile.id, 1)
        .unwrap();
    worker
        .run_and_export(
            profile.clone(),
            "SELECT n".into(),
            ExecutionId(301),
            None,
            &jobs,
            cancel,
        )
        .unwrap();
    let mut preview = 0;
    loop {
        match next(&worker) {
            Event::PreviewRows { rows, .. } => preview += rows.len(),
            Event::Downloaded { spool, .. } => {
                assert_eq!(spool.row_count(), 20_000);
                break;
            }
            Event::DownloadFailed { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert_eq!(preview, 1000);
    assert!(
        fixture
            .requested
            .lock()
            .unwrap()
            .iter()
            .all(|count| *count == 13_107)
    );
    let error = worker
        .run_and_export(
            profile.clone(),
            "SELECT blocked".into(),
            ExecutionId(302),
            None,
            &jobs,
            Arc::new(AtomicBool::new(false)),
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("active export"));
    assert_eq!(*fixture.executions.lock().unwrap(), ["SELECT n"]);
    drop(publication);
    profile.transfer.preset = TransferPreset::Conservative;
    worker.update_profile(profile.clone()).unwrap();
    let expected = worker.session_generation(&profile).unwrap();
    worker
        .run_and_export(
            profile.clone(),
            "SELECT next".into(),
            ExecutionId(303),
            Some(expected),
            &jobs,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    loop {
        match next(&worker) {
            Event::Downloaded { spool, .. } => {
                assert_eq!(spool.row_count(), 20_000);
                break;
            }
            Event::DownloadFailed { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert_eq!(fixture.connects.load(Ordering::SeqCst), 1);
    assert_eq!(worker.session_generation(&profile), Some(expected));
    assert_eq!(
        fixture
            .transfers
            .lock()
            .unwrap()
            .iter()
            .map(|transfer| transfer.preset)
            .collect::<Vec<_>>(),
        [TransferPreset::Balanced, TransferPreset::Conservative]
    );
    assert_eq!(
        *fixture.executions.lock().unwrap(),
        ["SELECT n", "SELECT next"]
    );
    assert!(fixture.requested.lock().unwrap().contains(&3276));
}

#[test]
fn a_cursor_transfer_override_keeps_preview_sizing_and_does_not_resubmit_sql() {
    use qrow::{
        export::{self, Snapshot},
        model::transfer::{Transfer, TransferPreset},
    };
    let fixture = Arc::new(Fixture {
        adaptive: true,
        total_rows: 20_000,
        ..Default::default()
    });
    let worker = worker(fixture.clone());
    let profile = export_profile();
    let execution = worker.run(profile.clone(), "SELECT existing".into());
    let mut columns = Vec::new();
    let mut rows = export::Rows::default();
    loop {
        match next(&worker) {
            Event::Columns(value) => columns = value,
            Event::Rows(value) => rows.extend(value),
            Event::Ready { more, .. } => {
                assert!(more);
                break;
            }
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert_eq!(rows.len(), 1000);
    assert_eq!(*fixture.requested.lock().unwrap(), [1000]);
    let generation = worker.session_generation(&profile).unwrap();
    let jobs = export::Jobs::default();
    let cancel = Arc::new(AtomicBool::new(false));
    let publication = jobs
        .register_for_profile(cancel.clone(), None, profile.id, 1)
        .unwrap();
    let download = worker
        .drain_with_transfer(
            execution,
            Arc::new(Snapshot::new(&columns, &rows).unwrap()),
            &jobs,
            cancel,
            Some(Transfer {
                preset: TransferPreset::Conservative,
                ..Default::default()
            }),
        )
        .unwrap();
    loop {
        match next(&worker) {
            Event::Downloaded { .. } => break,
            Event::DownloadFailed { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert_eq!(download.spool().unwrap().row_count(), 20_000);
    assert!(
        fixture.requested.lock().unwrap()[1..]
            .iter()
            .all(|count| *count == 3276)
    );
    assert_eq!(*fixture.executions.lock().unwrap(), ["SELECT existing"]);
    assert_eq!(worker.session_generation(&profile), Some(generation));
    assert_eq!(jobs.active_count(), 1);
    drop(publication);
    worker.run(profile, "SELECT normal".into());
    ready(&worker);
    assert_eq!(*fixture.requested.lock().unwrap().last().unwrap(), 1000);
}

#[test]
fn rejected_advancing_fetch_stops_without_retry_or_sql_resubmission() {
    use qrow::{export, logs::ExecutionId};
    let fixture = Arc::new(Fixture {
        adaptive: true,
        fail_fetch: Some(1),
        total_rows: 20_000,
        ..Default::default()
    });
    let worker = worker(fixture.clone());
    let download = worker
        .run_and_export(
            export_profile(),
            "SELECT n".into(),
            ExecutionId(304),
            None,
            &export::Jobs::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    loop {
        if let Event::DownloadFailed {
            consumed, message, ..
        } = next(&worker)
        {
            assert!(consumed);
            assert!(message.contains("Fetch transport failed"));
            break;
        }
    }
    assert_eq!(fixture.fetches.load(Ordering::SeqCst), 2);
    assert_eq!(*fixture.executions.lock().unwrap(), ["SELECT n"]);
    assert!(matches!(
        download.wait_spool().unwrap().status(),
        export::spool::Status::Failed(_)
    ));
}
