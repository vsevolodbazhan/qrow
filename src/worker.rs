use crate::{
    activity::{ActivityEvent, ActivityKind, ExecutionId, Severity},
    connector::{Cancellation, Connector, QueryError, QueryState, Session, hive::HiveConnector},
    model::{Column, MAX_RESULT_BYTES, MAX_RESULT_ROWS, PREVIEW_ROWS, Profile, Row},
    storage,
};
use anyhow::{Context, Result};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use zeroize::Zeroizing;

mod lifecycle;

pub enum Command {
    Run(Box<Profile>, String, ExecutionId),
    UpdateProfile(Box<Profile>),
    More,
    Disconnect,
    Shutdown,
}
pub enum Event {
    Connecting,
    Connected,
    Running,
    Columns(Vec<Column>),
    Rows(Vec<Row>),
    Ready { more: bool, limited: bool },
    Cancelled,
    Error { message: String, disconnected: bool },
    CancelError(String),
    Disconnected,
    IdleDisconnected,
    KeepAliveStarted,
    KeepAliveFinished,
}

type Target = Arc<Mutex<Option<Arc<dyn Cancellation>>>>;
pub type PasswordProvider = Arc<dyn Fn(&Profile) -> Result<Zeroizing<String>> + Send + Sync>;

pub struct Worker {
    tx: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    pub activities: mpsc::Receiver<ActivityEvent>,
    event_tx: mpsc::Sender<Event>,
    cancelled: Arc<AtomicBool>,
    target: Target,
    wake: Arc<dyn Fn() + Send + Sync>,
    generation: Arc<AtomicU64>,
    next_execution: Arc<AtomicU64>,
    stopped: Arc<AtomicBool>,
    done: mpsc::Receiver<()>,
}

impl Worker {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::with_connector(
            wake,
            Arc::new(HiveConnector),
            Arc::new(|profile| storage::password(profile.id)),
        )
    }
    pub fn with_connector(
        wake: Arc<dyn Fn() + Send + Sync>,
        connector: Arc<dyn Connector>,
        passwords: PasswordProvider,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let (activity_tx, activities) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let target = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(0));
        let (done_tx, done) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let mut runner = Runner {
            session: None,
            profile: None,
            rows: 0,
            bytes: 0,
            current_execution: None,
            cancelled: cancelled.clone(),
            target: target.clone(),
            tx: event_tx.clone(),
            activity_tx,
            wake: wake.clone(),
            connector,
            passwords,
            stopped: stopped.clone(),
            execution: None,
        };
        thread::spawn(move || {
            loop {
                let command = match runner.idle_interval() {
                    Some(interval) => match rx.recv_timeout(interval) {
                        Ok(command) => command,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            runner.maintain();
                            continue;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    },
                    None => match rx.recv() {
                        Ok(command) => command,
                        Err(_) => break,
                    },
                };
                let result = match command {
                    Command::Run(profile, sql, execution_id) => {
                        runner.run(*profile, sql, execution_id)
                    }
                    Command::UpdateProfile(profile) => {
                        runner.update_profile(*profile);
                        Ok(())
                    }
                    Command::More => runner.fetch_preview(),
                    Command::Disconnect => {
                        runner.disconnect();
                        runner.activity(
                            None,
                            Severity::Info,
                            ActivityKind::Disconnected,
                            "Disconnected",
                            None,
                        );
                        runner.emit(Event::Disconnected);
                        Ok(())
                    }
                    Command::Shutdown => break,
                };
                if let Err(error) = result {
                    let message = crate::connector::error_message(&error);
                    let execution_completed = runner
                        .execution
                        .as_ref()
                        .is_some_and(|execution| execution.execution_completed);
                    let mut disconnected =
                        error.downcast_ref::<QueryError>().is_none() || runner.session.is_none();
                    if !disconnected
                        && let Some(session) = runner.session.as_mut()
                        && session.close_operation().is_err()
                    {
                        disconnected = true;
                    }
                    if disconnected {
                        runner.disconnect();
                    }
                    *runner.target.lock().unwrap() = None;
                    runner.activity(
                        runner.execution_id(),
                        Severity::Error,
                        ActivityKind::Error,
                        message.clone(),
                        runner.execution_duration(),
                    );
                    if !execution_completed {
                        runner.activity(
                            runner.execution_id(),
                            Severity::Info,
                            ActivityKind::ExecutionCompleted,
                            format_execution_failure(runner.execution_duration()),
                            runner.execution_duration(),
                        );
                    }
                    runner.emit(Event::Error {
                        message,
                        disconnected,
                    });
                }
            }
            runner.disconnect();
            let _ = done_tx.send(());
        });
        Self {
            tx,
            events,
            activities,
            event_tx,
            cancelled,
            target,
            wake,
            generation,
            next_execution: Arc::new(AtomicU64::new(1)),
            stopped,
            done,
        }
    }

    pub fn run(&self, profile: Profile, sql: String) -> ExecutionId {
        let execution_id = ExecutionId(self.next_execution.fetch_add(1, Ordering::SeqCst));
        self.run_with_id(profile, sql, execution_id)
    }
    pub fn run_with_id(
        &self,
        profile: Profile,
        sql: String,
        execution_id: ExecutionId,
    ) -> ExecutionId {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        let _ = self
            .tx
            .send(Command::Run(Box::new(profile), sql, execution_id));
        execution_id
    }
    pub fn more(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        let _ = self.tx.send(Command::More);
    }
    pub fn update_profile(&self, profile: Profile) -> Result<()> {
        profile.lifecycle.validate()?;
        let _ = self.tx.send(Command::UpdateProfile(Box::new(profile)));
        Ok(())
    }
    pub fn disconnect(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send(Command::Disconnect);
    }
    pub fn shutdown(&self) {
        if !self.stopped.swap(true, Ordering::SeqCst) {
            self.cancel();
            let _ = self.tx.send(Command::Shutdown);
        }
    }
    pub fn wait_for_shutdown(&self, timeout: Duration) {
        let _ = self.done.recv_timeout(timeout);
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let cancellation = self.target.lock().unwrap().clone();
        if let Some(cancellation) = cancellation {
            let tx = self.event_tx.clone();
            let wake = self.wake.clone();
            let generation = self.generation.clone();
            let current = generation.load(Ordering::SeqCst);
            thread::spawn(move || {
                if let Err(error) = cancellation.cancel()
                    && generation.load(Ordering::SeqCst) == current
                {
                    let _ = tx.send(Event::CancelError(format!(
                        "Cancellation request failed: {error:#}"
                    )));
                    wake();
                }
            });
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Runner {
    stopped: Arc<AtomicBool>,
    session: Option<Box<dyn Session>>,
    profile: Option<Profile>,
    rows: usize,
    bytes: usize,
    current_execution: Option<ExecutionId>,
    execution: Option<ExecutionTiming>,
    cancelled: Arc<AtomicBool>,
    target: Target,
    tx: mpsc::Sender<Event>,
    activity_tx: mpsc::Sender<ActivityEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    connector: Arc<dyn Connector>,
    passwords: PasswordProvider,
}

struct ExecutionTiming {
    id: ExecutionId,
    started: Instant,
    fetch_page: usize,
    execution_completed: bool,
}

struct FetchSummary {
    execution_id: Option<ExecutionId>,
    started: Instant,
    page: usize,
    start_row: usize,
    fetched: usize,
    more: bool,
    limited: bool,
}

impl Runner {
    fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
        (self.wake)();
    }
    fn activity(
        &self,
        execution_id: Option<ExecutionId>,
        severity: Severity,
        kind: ActivityKind,
        text: impl Into<String>,
        duration: Option<Duration>,
    ) {
        let mut event = ActivityEvent::at(SystemTime::now(), execution_id, severity, kind, text);
        event.duration = duration;
        self.emit_activity(event);
    }
    fn emit_activity(&self, event: ActivityEvent) {
        let _ = self.activity_tx.send(event);
        (self.wake)();
    }
    fn execution_id(&self) -> Option<ExecutionId> {
        self.current_execution
    }
    fn execution_duration(&self) -> Option<Duration> {
        self.execution
            .as_ref()
            .map(|execution| execution.started.elapsed())
    }
    fn complete_execution(&mut self, has_results: bool) {
        let Some(execution) = self.execution.as_mut() else {
            return;
        };
        if execution.execution_completed {
            return;
        }
        execution.execution_completed = true;
        let duration = execution.started.elapsed();
        let execution_id = execution.id;
        self.activity(
            Some(execution_id),
            Severity::Info,
            ActivityKind::ExecutionCompleted,
            format!(
                "Execution completed on the server, result set: {has_results} (client measurement: {})",
                format_duration(duration)
            ),
            Some(duration),
        );
    }
    fn disconnect(&mut self) {
        *self.target.lock().unwrap() = None;
        if let Some(mut session) = self.session.take() {
            let _ = session.close();
        }
        self.profile = None;
    }
    fn update_profile(&mut self, profile: Profile) {
        if self.session.is_some()
            && self
                .profile
                .as_ref()
                .is_some_and(|current| current.connection_identity_eq(&profile))
        {
            self.profile = Some(profile);
        }
    }
    fn run(&mut self, profile: Profile, sql: String, execution_id: ExecutionId) -> Result<()> {
        self.current_execution = Some(execution_id);
        self.execution = None;
        profile.lifecycle.validate()?;
        self.rows = 0;
        self.bytes = 0;
        let reconnect = self.session.is_none()
            || self
                .profile
                .as_ref()
                .is_none_or(|current| !current.connection_identity_eq(&profile));
        if reconnect {
            self.disconnect();
            let connect_started = Instant::now();
            self.emit(Event::Connecting);
            let password = (self.passwords)(&profile)?;
            self.session = Some(self.connector.connect(&profile, password)?);
            self.profile = Some(profile);
            self.activity(
                Some(execution_id),
                Severity::Info,
                ActivityKind::Connected,
                format!(
                    "Connected to {} (client measurement: {})",
                    self.profile.as_ref().unwrap().name,
                    format_duration(connect_started.elapsed())
                ),
                Some(connect_started.elapsed()),
            );
            self.emit(Event::Connected);
        } else {
            self.profile = Some(profile);
        }
        if self.cancelled.load(Ordering::SeqCst) {
            self.activity(
                Some(execution_id),
                Severity::Info,
                ActivityKind::Cancelled,
                "Execution cancelled before submission",
                self.execution_duration(),
            );
            self.emit(Event::Cancelled);
            return Ok(());
        }
        self.emit(Event::Running);
        self.execution = Some(ExecutionTiming {
            id: execution_id,
            started: Instant::now(),
            fetch_page: 0,
            execution_completed: false,
        });
        *self.target.lock().unwrap() = None;
        let cancellation = self.session.as_mut().unwrap().execute(&sql)?;
        *self.target.lock().unwrap() = Some(cancellation.clone());
        self.activity(
            Some(execution_id),
            Severity::Info,
            ActivityKind::ExecutionStarted,
            "SQL accepted by the server",
            None,
        );
        // Catch cancellation requested before ExecuteStatement returned its handle.
        if self.cancelled.load(Ordering::SeqCst) {
            cancellation.cancel()?;
        }
        loop {
            match self.session.as_mut().unwrap().poll()? {
                QueryState::Running => thread::sleep(Duration::from_millis(100)),
                QueryState::Cancelled => {
                    self.session.as_mut().unwrap().close_operation()?;
                    *self.target.lock().unwrap() = None;
                    self.activity(
                        Some(execution_id),
                        Severity::Info,
                        ActivityKind::Cancelled,
                        "Query cancelled by the server",
                        self.execution_duration(),
                    );
                    self.emit(Event::Cancelled);
                    return Ok(());
                }
                QueryState::Finished { has_results } => {
                    if self.cancelled.load(Ordering::SeqCst) {
                        self.session.as_mut().unwrap().close_operation()?;
                        *self.target.lock().unwrap() = None;
                        self.activity(
                            Some(execution_id),
                            Severity::Info,
                            ActivityKind::Cancelled,
                            "Query finished after cancellation was requested",
                            self.execution_duration(),
                        );
                        self.emit(Event::Ready {
                            more: false,
                            limited: false,
                        });
                        return Ok(());
                    }
                    if !has_results {
                        self.session.as_mut().unwrap().close_operation()?;
                        *self.target.lock().unwrap() = None;
                        self.complete_execution(false);
                        self.emit(Event::Ready {
                            more: false,
                            limited: false,
                        });
                        return Ok(());
                    }
                    let columns = self.session.as_mut().unwrap().columns()?;
                    self.complete_execution(true);
                    self.emit(Event::Columns(columns));
                    return self.fetch_preview();
                }
            }
        }
    }

    fn fetch_preview(&mut self) -> Result<()> {
        let execution_id = self.execution_id();
        let fetch_started = Instant::now();
        let page = self
            .execution
            .as_mut()
            .map(|execution| {
                execution.fetch_page += 1;
                execution.fetch_page
            })
            .unwrap_or(1);
        self.activity(
            execution_id,
            Severity::Info,
            ActivityKind::FetchStarted,
            format!("Fetching preview page {page}"),
            None,
        );
        let start_row = self.rows;
        let mut fetched = 0;
        while fetched < PREVIEW_ROWS {
            if self.finish_cancelled_fetch()? {
                return Ok(());
            }
            let batch = self
                .session
                .as_mut()
                .context("Session is disconnected")?
                .fetch((PREVIEW_ROWS - fetched).min(250))?;
            // A blocked fetch may return after Cancel was requested.
            if self.finish_cancelled_fetch()? {
                return Ok(());
            }
            let count = batch.rows.len();
            anyhow::ensure!(
                count <= (PREVIEW_ROWS - fetched).min(250),
                "Connector returned more rows than requested"
            );
            let bytes: usize = batch
                .rows
                .iter()
                .map(|row| {
                    row.capacity() * std::mem::size_of::<Option<String>>()
                        + row.iter().flatten().map(String::capacity).sum::<usize>()
                })
                .sum();
            let limited = self.rows + count > MAX_RESULT_ROWS
                || self.bytes.saturating_add(bytes) > MAX_RESULT_BYTES;
            if limited || count == 0 {
                self.session.as_mut().unwrap().close_operation()?;
                *self.target.lock().unwrap() = None;
                self.fetch_completed(FetchSummary {
                    execution_id,
                    started: fetch_started,
                    page,
                    start_row,
                    fetched,
                    more: false,
                    limited,
                });
                self.emit(Event::Ready {
                    more: false,
                    limited,
                });
                return Ok(());
            }
            fetched += count;
            self.rows += count;
            self.bytes += bytes;
            self.emit(Event::Rows(batch.rows));
            if !batch.more {
                self.session.as_mut().unwrap().close_operation()?;
                *self.target.lock().unwrap() = None;
                self.fetch_completed(FetchSummary {
                    execution_id,
                    started: fetch_started,
                    page,
                    start_row,
                    fetched,
                    more: false,
                    limited: false,
                });
                self.emit(Event::Ready {
                    more: false,
                    limited: false,
                });
                return Ok(());
            }
        }
        self.fetch_completed(FetchSummary {
            execution_id,
            started: fetch_started,
            page,
            start_row,
            fetched,
            more: true,
            limited: false,
        });
        self.emit(Event::Ready {
            more: true,
            limited: false,
        });
        Ok(())
    }

    fn fetch_completed(&self, summary: FetchSummary) {
        let FetchSummary {
            execution_id,
            started,
            page,
            start_row,
            fetched,
            more,
            limited,
        } = summary;
        let range = if fetched == 0 {
            "no rows".into()
        } else {
            format!("rows {}–{}", start_row + 1, start_row + fetched)
        };
        self.activity(
            execution_id,
            Severity::Info,
            ActivityKind::FetchCompleted,
            format!(
                "Fetched preview page {page}: {range}, {fetched} rows, {} retained, more rows: {more}, preview limit: {limited} (client measurement: {})",
                self.rows,
                format_duration(started.elapsed())
            ),
            Some(started.elapsed()),
        );
    }

    fn finish_cancelled_fetch(&mut self) -> Result<bool> {
        if !self.cancelled.load(Ordering::SeqCst) {
            return Ok(false);
        }
        // Execution has finished. Closing releases the cursor, without rolling back SQL.
        self.session
            .as_mut()
            .context("Session is disconnected")?
            .close_operation()?;
        *self.target.lock().unwrap() = None;
        self.activity(
            self.execution_id(),
            Severity::Info,
            ActivityKind::Cancelled,
            "Preview fetch cancelled; downloaded rows retained",
            self.execution_duration(),
        );
        self.emit(Event::Cancelled);
        Ok(true)
    }
}

fn format_duration(duration: Duration) -> String {
    format!("{:.2} s", duration.as_secs_f64())
}

fn format_execution_failure(duration: Option<Duration>) -> String {
    duration.map_or_else(
        || "Execution failed before query submission".into(),
        |duration| format!("Execution failed after {}", format_duration(duration)),
    )
}
