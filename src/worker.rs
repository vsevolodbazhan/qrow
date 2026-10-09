use crate::{
    connector::{
        Cancellation, Completion, Connector, QueryError, Secret, Session, wait_for_completion,
    },
    logs::{ExecutionId, LogEvent, LogKind, Severity},
    model::{Column, MAX_RESULT_BYTES, MAX_RESULT_ROWS, PREVIEW_ROWS, Profile, Row},
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

mod download;
mod lifecycle;
mod run_export;
pub use download::{Cursor, Download};
pub use run_export::SessionChanged;

struct Drain {
    execution: ExecutionId,
    source: Arc<crate::export::Snapshot>,
    producer: crate::export::spool::Producer,
    download: Arc<Download>,
    _guard: crate::export::Writer,
    _transport: Arc<crate::export::budget::Allowance>,
}

struct Pending {
    rows: Vec<Row>,
    _memory: crate::export::budget::Allocation,
}

enum Command {
    RunExport(Box<run_export::Request>),
    Run(Box<Profile>, String, ExecutionId),
    UpdateProfile(Box<Profile>),
    More,
    Drain(Box<Drain>),
    Disconnect,
    Shutdown,
}
pub enum Event {
    Session {
        execution: ExecutionId,
        generation: uuid::Uuid,
    },
    PreviewRows {
        execution: ExecutionId,
        rows: crate::export::Rows,
    },
    PreviewComplete {
        execution: ExecutionId,
        complete: bool,
    },
    Connecting,
    Authenticating(bool),
    Connected,
    Running,
    Columns(Vec<Column>),
    ExportContext(crate::export::Context),
    Cursor {
        execution: ExecutionId,
        state: Cursor,
    },
    DownloadProgress {
        execution: ExecutionId,
        rows: usize,
        bytes: u64,
        elapsed: Duration,
    },
    Downloaded {
        execution: ExecutionId,
        spool: Arc<crate::export::spool::Spool>,
    },
    DownloadFailed {
        execution: ExecutionId,
        message: String,
        consumed: bool,
        disconnected: bool,
    },
    Rows(Vec<Row>),
    Ready {
        more: bool,
        limited: bool,
    },
    Cancelled,
    Error {
        message: String,
        disconnected: bool,
        /// The connection needs a new browser sign-in.
        sign_in_required: bool,
    },
    CancelError(String),
    Disconnected,
    IdleDisconnected,
    KeepAliveStarted,
    KeepAliveFinished,
}

struct SessionIdentity {
    generation: uuid::Uuid,
    profile: Profile,
}

type Target = Arc<Mutex<Option<Arc<dyn Cancellation>>>>;
/// Returns the password or the access tokens of a connection. It runs on the
/// worker thread before each new session.
pub type CredentialProvider = Arc<dyn Fn(&Profile) -> Result<Secret> + Send + Sync>;
/// Reports the stable ID of active work that defers idle disconnection.
pub type IdleGuard = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

pub struct Worker {
    session_identity: Arc<Mutex<Option<SessionIdentity>>>,
    tx: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    pub logs: mpsc::Receiver<LogEvent>,
    event_tx: mpsc::Sender<Event>,
    cancelled: Arc<AtomicBool>,
    target: Target,
    wake: Arc<dyn Fn() + Send + Sync>,
    generation: Arc<AtomicU64>,
    next_execution: Arc<AtomicU64>,
    stopped: Arc<AtomicBool>,
    done: mpsc::Receiver<()>,
    idle_guard: Arc<Mutex<Option<IdleGuard>>>,
    download: Arc<Mutex<Option<Arc<Download>>>>,
}

impl Worker {
    pub fn with_connector(
        wake: Arc<dyn Fn() + Send + Sync>,
        connector: Arc<dyn Connector>,
        credentials: CredentialProvider,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let (log_tx, logs) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let target = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(0));
        let (done_tx, done) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let idle_guard = Arc::new(Mutex::new(None));
        let download = Arc::new(Mutex::new(None));
        let session_identity = Arc::new(Mutex::new(None));
        let mut runner = Runner {
            session_identity: session_identity.clone(),
            session: None,
            profile: None,
            rows: 0,
            bytes: 0,
            current_execution: None,
            cancelled: cancelled.clone(),
            target: target.clone(),
            tx: event_tx.clone(),
            log_tx,
            wake: wake.clone(),
            connector,
            credentials,
            stopped: stopped.clone(),
            execution: None,
            idle_guard: idle_guard.clone(),
            download: download.clone(),
            cursor: Cursor::Unavailable,
            pending: None,
        };
        thread::spawn(move || {
            let mut idle_started = Instant::now();
            let mut idle_refresh = None;
            loop {
                let command = match runner.idle_interval() {
                    Some(interval) => match rx.recv_timeout({
                        let remaining = interval.saturating_sub(idle_started.elapsed());
                        if remaining.is_zero() && runner.idle_disconnect_deferred(&mut idle_refresh)
                        {
                            Duration::from_millis(100)
                        } else {
                            remaining
                        }
                    }) {
                        Ok(command) => command,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if runner.idle_disconnect_deferred(&mut idle_refresh) {
                                continue;
                            }
                            runner.maintain();
                            idle_started = Instant::now();
                            idle_refresh = None;
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
                    Command::RunExport(request) => {
                        runner.run_export(*request);
                        Ok(())
                    }
                    Command::Run(profile, sql, execution_id) => {
                        runner.run(*profile, sql, execution_id)
                    }
                    Command::UpdateProfile(profile) => {
                        runner.update_profile(*profile);
                        Ok(())
                    }
                    Command::More => runner.fetch_preview(),
                    Command::Drain(drain) => {
                        runner.drain(*drain);
                        Ok(())
                    }
                    Command::Disconnect => {
                        runner.disconnect();
                        runner.log(
                            None,
                            Severity::Info,
                            LogKind::Disconnected,
                            "Disconnected",
                            None,
                        );
                        runner.emit(Event::Disconnected);
                        Ok(())
                    }
                    Command::Shutdown => break,
                };
                if let Err(error) = result {
                    if error.is::<crate::external_auth::Cancelled>() {
                        runner.disconnect();
                        *runner.target.lock().unwrap() = None;
                        runner.emit(Event::Disconnected);
                        runner.emit(Event::Cancelled);
                        idle_started = Instant::now();
                        continue;
                    }
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
                    runner.log(
                        runner.execution_id(),
                        Severity::Error,
                        LogKind::Error,
                        message.clone(),
                        runner.execution_duration(),
                    );
                    if !execution_completed {
                        runner.log(
                            runner.execution_id(),
                            Severity::Info,
                            LogKind::ExecutionCompleted,
                            format_execution_failure(runner.execution_duration()),
                            runner.execution_duration(),
                        );
                    }
                    let sign_in_required =
                        crate::oidc::failure(&error) == crate::oidc::Failure::SignInRequired;
                    runner.emit(Event::Error {
                        message,
                        disconnected,
                        sign_in_required,
                    });
                }
                idle_started = Instant::now();
                idle_refresh = None;
            }
            runner.disconnect();
            let _ = done_tx.send(());
        });
        Self {
            session_identity,
            tx,
            events,
            logs,
            event_tx,
            cancelled,
            target,
            wake,
            generation,
            next_execution: Arc::new(AtomicU64::new(1)),
            stopped,
            done,
            idle_guard,
            download,
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
    /// Append this exact result's delivered prefix and remaining worker-owned cursor.
    pub fn drain(
        &self,
        execution: ExecutionId,
        source: Arc<crate::export::Snapshot>,
        jobs: &crate::export::Jobs,
        cancel: Arc<AtomicBool>,
    ) -> std::io::Result<Arc<Download>> {
        let table = source.table(None);
        let transport = crate::export::budget::GLOBAL.allowance(64 * crate::export::budget::MIB)?;
        let (spool, producer) =
            crate::export::spool::Spool::new(table.columns, table.rows.context())?;
        let download = Download::new(spool, cancel.clone());
        let callback_download = Arc::downgrade(&download);
        let guard = jobs.register_with_cancel(
            cancel,
            Some(Arc::new(move || {
                if let Some(download) = callback_download.upgrade() {
                    download.cancel_in_background();
                }
            })),
        );
        self.tx
            .send(Command::Drain(Box::new(Drain {
                execution,
                source,
                producer,
                download: download.clone(),
                _guard: guard,
                _transport: transport,
            })))
            .map_err(|_| std::io::Error::other("The query worker stopped."))?;
        Ok(download)
    }
    pub fn update_profile(&self, profile: Profile) -> Result<()> {
        profile.lifecycle.validate()?;
        let _ = self.tx.send(Command::UpdateProfile(Box::new(profile)));
        Ok(())
    }
    /// Change idle protection without restarting the idle timer.
    /// Explicit disconnection and keep-alive queries do not use this guard.
    pub fn set_idle_guard(&self, guard: Option<IdleGuard>) {
        *self.idle_guard.lock().unwrap() = guard;
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
        if let Some(download) = self.download.lock().unwrap().clone() {
            download.cancel();
            return;
        }
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
    session_identity: Arc<Mutex<Option<SessionIdentity>>>,
    idle_guard: Arc<Mutex<Option<IdleGuard>>>,
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
    log_tx: mpsc::Sender<LogEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    connector: Arc<dyn Connector>,
    credentials: CredentialProvider,
    download: Arc<Mutex<Option<Arc<Download>>>>,
    cursor: Cursor,
    pending: Option<Pending>,
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
    fn log(
        &self,
        execution_id: Option<ExecutionId>,
        severity: Severity,
        kind: LogKind,
        text: impl Into<String>,
        duration: Option<Duration>,
    ) {
        let mut event = LogEvent::at(SystemTime::now(), execution_id, severity, kind, text);
        event.duration = duration;
        self.emit_log(event);
    }
    fn emit_log(&self, event: LogEvent) {
        let _ = self.log_tx.send(event);
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
        self.log(
            Some(execution_id),
            Severity::Info,
            LogKind::ExecutionCompleted,
            format!(
                "Execution completed on the server, result set: {has_results} (client measurement: {})",
                format_duration(duration)
            ),
            Some(duration),
        );
    }
    fn disconnect(&mut self) {
        *self.session_identity.lock().unwrap() = None;
        self.cursor = Cursor::Unavailable;
        self.pending = None;
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
    fn prepare_session(
        &mut self,
        profile: Profile,
        execution_id: ExecutionId,
        control: Option<&crate::connector::ConnectionControl>,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        let reconnect = self.session.is_none()
            || self
                .profile
                .as_ref()
                .is_none_or(|current| !current.connection_identity_eq(&profile));
        if reconnect {
            if let Some(control) = control {
                control.check()?;
                if let Some(transport) = self
                    .session
                    .as_ref()
                    .and_then(|session| session.transport_cancellation())
                {
                    control.register(transport)?;
                }
            }
            self.disconnect();
            if let Some(control) = control {
                control.check()?;
            }
            let connect_started = Instant::now();
            self.emit(Event::Connecting);
            let cancelled = cancel;
            let events = self.tx.clone();
            let wake = self.wake.clone();
            let secret =
                (self.credentials)(&profile)?.with_control(crate::external_auth::Control::new(
                    Arc::new(move || cancelled.load(Ordering::SeqCst)),
                    Arc::new(move |waiting| {
                        let _ = events.send(Event::Authenticating(waiting));
                        wake();
                    }),
                ));
            self.session = Some(match control {
                Some(control) => self
                    .connector
                    .connect_controlled(&profile, secret, control)?,
                None => self.connector.connect(&profile, secret)?,
            });
            *self.session_identity.lock().unwrap() = Some(SessionIdentity {
                generation: uuid::Uuid::new_v4(),
                profile: profile.clone(),
            });
            self.profile = Some(profile);
            self.log(
                Some(execution_id),
                Severity::Info,
                LogKind::Connected,
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
        if let Some(control) = control {
            control.check()?;
            if let Some(transport) = self.session.as_ref().unwrap().transport_cancellation() {
                control.register(transport)?;
            }
        }
        Ok(())
    }

    fn run(&mut self, profile: Profile, sql: String, execution_id: ExecutionId) -> Result<()> {
        self.cursor = Cursor::Unavailable;
        self.pending = None;
        self.current_execution = Some(execution_id);
        self.execution = None;
        profile.lifecycle.validate()?;
        self.rows = 0;
        self.bytes = 0;
        self.prepare_session(profile, execution_id, None, self.cancelled.clone())?;
        if self.cancelled.load(Ordering::SeqCst) {
            self.log(
                Some(execution_id),
                Severity::Info,
                LogKind::Cancelled,
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
        self.emit_session(execution_id);
        self.log(
            Some(execution_id),
            Severity::Info,
            LogKind::ExecutionStarted,
            "SQL accepted by the server",
            None,
        );
        // Catch cancellation requested before ExecuteStatement returned its handle.
        if self.cancelled.load(Ordering::SeqCst) {
            cancellation.cancel()?;
        }
        match wait_for_completion(self.session.as_mut().unwrap().as_mut(), None)? {
            Completion::Cancelled => {
                self.session.as_mut().unwrap().close_operation()?;
                *self.target.lock().unwrap() = None;
                self.log(
                    Some(execution_id),
                    Severity::Info,
                    LogKind::Cancelled,
                    "Query cancelled by the server",
                    self.execution_duration(),
                );
                self.emit(Event::Cancelled);
                Ok(())
            }
            Completion::Finished { has_results } => {
                if self.cancelled.load(Ordering::SeqCst) {
                    self.session.as_mut().unwrap().close_operation()?;
                    *self.target.lock().unwrap() = None;
                    self.log(
                        Some(execution_id),
                        Severity::Info,
                        LogKind::Cancelled,
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
                self.emit(Event::ExportContext(
                    self.session.as_ref().unwrap().export_context(),
                ));
                self.fetch_preview()
            }
        }
    }

    fn fetch_preview(&mut self) -> Result<()> {
        if self.pending.is_some() {
            self.set_cursor(Cursor::Available);
            self.emit(Event::Ready {
                more: false,
                limited: true,
            });
            return Ok(());
        }
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
        self.log(
            execution_id,
            Severity::Info,
            LogKind::FetchStarted,
            format!("Fetching preview page {page}"),
            None,
        );
        let start_row = self.rows;
        let mut fetched = 0;
        while fetched < PREVIEW_ROWS {
            if self.finish_cancelled_fetch()? {
                return Ok(());
            }
            let live_cursor = self
                .profile
                .as_ref()
                .is_some_and(|profile| profile.database_type == crate::model::DatabaseType::Kyuubi);
            let fetch_memory = live_cursor
                .then(|| crate::export::budget::GLOBAL.acquire(128 * crate::export::budget::MIB))
                .transpose()?;
            let batch = self
                .session
                .as_mut()
                .context("Session is disconnected")?
                .fetch(PREVIEW_ROWS - fetched)?;
            // A blocked fetch may return after Cancel was requested.
            if self.finish_cancelled_fetch()? {
                return Ok(());
            }
            let count = batch.rows.len();
            anyhow::ensure!(
                count <= PREVIEW_ROWS - fetched,
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
            let limited = self.session.as_ref().unwrap().result_limited()
                || self.rows + count > MAX_RESULT_ROWS
                || self.bytes.saturating_add(bytes) > MAX_RESULT_BYTES;
            if limited || count == 0 {
                if limited
                    && count > 0
                    && live_cursor
                    && !self.session.as_ref().unwrap().result_limited()
                {
                    self.pending = Some(Pending {
                        rows: batch.rows,
                        _memory: fetch_memory.unwrap(),
                    });
                    self.set_cursor(Cursor::Available);
                } else {
                    self.session.as_mut().unwrap().close_operation()?;
                    *self.target.lock().unwrap() = None;
                    self.set_cursor(if limited {
                        Cursor::Unavailable
                    } else {
                        Cursor::Complete
                    });
                }
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
        self.set_cursor(
            if self
                .profile
                .as_ref()
                .is_some_and(|profile| profile.database_type == crate::model::DatabaseType::Kyuubi)
            {
                Cursor::Available
            } else {
                Cursor::Unavailable
            },
        );
        self.emit(Event::Ready {
            more: true,
            limited: false,
        });
        Ok(())
    }

    fn set_cursor(&mut self, state: Cursor) {
        self.cursor = state;
        if let Some(execution) = self.current_execution {
            self.emit(Event::Cursor { execution, state });
        }
    }

    fn drain(&mut self, drain: Drain) {
        let Drain {
            execution,
            source,
            mut producer,
            download,
            _guard,
            _transport,
        } = drain;
        // More can have been queued while the dialog's snapshot was open.
        // Refuse a stale prefix before advancing or cancelling any cursor.
        if self.current_execution != Some(execution)
            || self.cursor != Cursor::Available
            || source.row_count() != self.rows
        {
            let message =
                "The result changed. Reopen Export to use the current downloaded rows.".to_owned();
            download
                .spool()
                .expect("A cursor drain has a spool")
                .fail(message.clone());
            self.emit(Event::DownloadFailed {
                execution,
                message,
                consumed: false,
                disconnected: false,
            });
            return;
        }
        let target = self.target.lock().unwrap().clone();
        let Some(target) = target else {
            let message = "The result cursor is no longer available.".to_owned();
            download
                .spool()
                .expect("A cursor drain has a spool")
                .fail(message.clone());
            self.emit(Event::DownloadFailed {
                execution,
                message,
                consumed: false,
                disconnected: false,
            });
            return;
        };
        if let Err(error) = download.bind(target, _guard.fork()) {
            download
                .spool()
                .expect("A cursor drain has a spool")
                .cancel();
            self.emit(Event::DownloadFailed {
                execution,
                message: error.to_string(),
                consumed: false,
                disconnected: false,
            });
            return;
        }
        *self.download.lock().unwrap() = Some(download.clone());
        self.set_cursor(Cursor::Draining);
        let started = Instant::now();
        let mut count = 0usize;
        let result = (|| -> Result<()> {
            for batch in source.batches() {
                producer.append(batch, download.cancelled())?;
                count += batch.len();
            }
            if let Some(pending) = self.pending.take() {
                producer.append(&pending.rows, download.cancelled())?;
                count += pending.rows.len();
            }
            loop {
                crate::export::check_cancelled(download.cancelled())?;
                let _fetch_memory = producer.reserve_fetch()?;
                let batch = self
                    .session
                    .as_mut()
                    .context("Session is disconnected")?
                    .fetch(PREVIEW_ROWS)?;
                crate::export::check_cancelled(download.cancelled())?;
                anyhow::ensure!(
                    batch.rows.len() <= PREVIEW_ROWS,
                    "Connector returned more rows than requested"
                );
                if batch.rows.is_empty() {
                    break;
                }
                producer.append(&batch.rows, download.cancelled())?;
                count += batch.rows.len();
                self.emit(Event::DownloadProgress {
                    execution,
                    rows: count,
                    bytes: download
                        .spool()
                        .expect("A cursor drain has a spool")
                        .bytes(),
                    elapsed: started.elapsed(),
                });
            }
            self.session.as_mut().unwrap().close_operation()?;
            download.complete(producer)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                *self.target.lock().unwrap() = None;
                *self.download.lock().unwrap() = None;
                self.set_cursor(Cursor::Downloaded);
                self.emit(Event::Downloaded {
                    execution,
                    spool: download.spool().expect("A cursor drain has a spool"),
                });
            }
            Err(error) => {
                let message = crate::connector::error_message(&error);
                download.fail(message.clone());
                let close = self
                    .session
                    .as_mut()
                    .is_none_or(|session| session.close_operation().is_err());
                let disconnected = !download.stopped() || close;
                *self.target.lock().unwrap() = None;
                *self.download.lock().unwrap() = None;
                if disconnected {
                    self.disconnect();
                }
                self.set_cursor(Cursor::Consumed);
                self.emit(Event::DownloadFailed {
                    execution,
                    message,
                    consumed: true,
                    disconnected,
                });
            }
        }
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
        self.log(
            execution_id,
            Severity::Info,
            LogKind::FetchCompleted,
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
        self.pending = None;
        self.cursor = Cursor::Consumed;
        self.log(
            self.execution_id(),
            Severity::Info,
            LogKind::Cancelled,
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
