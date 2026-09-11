use anyhow::Result;
use qrow::{
    connector::{Cancellation, Connector, QueryState, Session},
    model::{Batch, Column, Profile},
    worker::{Event, Worker},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Default)]
struct Fixture {
    connects: AtomicUsize,
    total_rows: usize,
    value_bytes: usize,
    closes: Arc<AtomicUsize>,
}
struct FakeSession {
    total_rows: usize,
    value_bytes: usize,
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
            total_rows: if self.total_rows == 0 {
                1250
            } else {
                self.total_rows
            },
            value_bytes: self.value_bytes,
            offset: 0,
            slow: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            closes: self.closes.clone(),
        }))
    }
}
impl Session for FakeSession {
    fn execute(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.offset = 0;
        self.slow = sql == "slow";
        self.cancelled.store(false, Ordering::SeqCst);
        if sql == "broken" {
            return Err(qrow::connector::QueryError("syntax error".into()).into());
        }
        Ok(Arc::new(Cancel(self.cancelled.clone())))
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
        Ok(Batch {
            more: !rows.is_empty(),
            rows,
        })
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
                assert!(rows.len() <= 250);
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
