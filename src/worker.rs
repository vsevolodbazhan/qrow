use crate::{
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
    time::Duration,
};
use zeroize::Zeroizing;

mod lifecycle;

pub enum Command {
    Run(Box<Profile>, String),
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
    event_tx: mpsc::Sender<Event>,
    cancelled: Arc<AtomicBool>,
    target: Target,
    wake: Arc<dyn Fn() + Send + Sync>,
    generation: Arc<AtomicU64>,
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
            cancelled: cancelled.clone(),
            target: target.clone(),
            tx: event_tx.clone(),
            wake: wake.clone(),
            connector,
            passwords,
            stopped: stopped.clone(),
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
                    Command::Run(profile, sql) => runner.run(*profile, sql),
                    Command::More => runner.fetch_preview(),
                    Command::Disconnect => {
                        runner.disconnect();
                        runner.emit(Event::Disconnected);
                        Ok(())
                    }
                    Command::Shutdown => break,
                };
                if let Err(error) = result {
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
                    runner.emit(Event::Error {
                        message: format!("{error:#}"),
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
            event_tx,
            cancelled,
            target,
            wake,
            generation,
            stopped,
            done,
        }
    }

    pub fn run(&self, profile: Profile, sql: String) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        let _ = self.tx.send(Command::Run(Box::new(profile), sql));
    }
    pub fn more(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        let _ = self.tx.send(Command::More);
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
    cancelled: Arc<AtomicBool>,
    target: Target,
    tx: mpsc::Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
    connector: Arc<dyn Connector>,
    passwords: PasswordProvider,
}

impl Runner {
    fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
        (self.wake)();
    }
    fn disconnect(&mut self) {
        *self.target.lock().unwrap() = None;
        if let Some(mut session) = self.session.take() {
            let _ = session.close();
        }
        self.profile = None;
    }
    fn run(&mut self, profile: Profile, sql: String) -> Result<()> {
        profile.lifecycle.validate()?;
        self.rows = 0;
        self.bytes = 0;
        if self.profile.as_ref() != Some(&profile) || self.session.is_none() {
            self.disconnect();
            self.emit(Event::Connecting);
            let password = (self.passwords)(&profile)?;
            self.session = Some(self.connector.connect(&profile, password)?);
            self.profile = Some(profile);
            self.emit(Event::Connected);
        }
        if self.cancelled.load(Ordering::SeqCst) {
            self.emit(Event::Cancelled);
            return Ok(());
        }
        self.emit(Event::Running);
        *self.target.lock().unwrap() = None;
        let cancellation = self.session.as_mut().unwrap().execute(&sql)?;
        *self.target.lock().unwrap() = Some(cancellation.clone());
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
                    self.emit(Event::Cancelled);
                    return Ok(());
                }
                QueryState::Finished { has_results } => {
                    if self.cancelled.load(Ordering::SeqCst) {
                        self.session.as_mut().unwrap().close_operation()?;
                        *self.target.lock().unwrap() = None;
                        self.emit(Event::Ready {
                            more: false,
                            limited: false,
                        });
                        return Ok(());
                    }
                    if !has_results {
                        self.session.as_mut().unwrap().close_operation()?;
                        *self.target.lock().unwrap() = None;
                        self.emit(Event::Ready {
                            more: false,
                            limited: false,
                        });
                        return Ok(());
                    }
                    let columns = self.session.as_mut().unwrap().columns()?;
                    self.emit(Event::Columns(columns));
                    return self.fetch_preview();
                }
            }
        }
    }

    fn fetch_preview(&mut self) -> Result<()> {
        let mut fetched = 0;
        while fetched < PREVIEW_ROWS {
            if self.cancelled.load(Ordering::SeqCst) {
                // Fetching only starts after FINISHED. Closing releases the cursor; it does not roll back execution.
                self.session
                    .as_mut()
                    .context("Session is disconnected")?
                    .close_operation()?;
                *self.target.lock().unwrap() = None;
                self.emit(Event::Cancelled);
                return Ok(());
            }
            let batch = self
                .session
                .as_mut()
                .context("Session is disconnected")?
                .fetch((PREVIEW_ROWS - fetched).min(250))?;
            let count = batch.rows.len();
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
                self.emit(Event::Ready {
                    more: false,
                    limited: false,
                });
                return Ok(());
            }
        }
        self.emit(Event::Ready {
            more: true,
            limited: false,
        });
        Ok(())
    }
}
