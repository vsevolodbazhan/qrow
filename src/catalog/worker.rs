//! A thread that reads the catalog of one connection, or of the connections
//! that share one catalog.
//!
//! The worker opens its own session for the queued refreshes and closes it
//! when the queue is empty, so it never changes the idle timer or the result
//! cursor of a query tab. It runs one refresh at a time, with the profile of
//! the member that asked for it. While a tab of a member has a live session,
//! the worker also refreshes the catalog when its refresh period passes.

use super::{
    Catalog, CatalogConfig, CatalogIdentity, MetadataRows, Scope, Unfinished, now, parse_columns,
    parse_relations, parse_schemas,
};
use crate::{
    connector::{
        Cancellation, Connector, MetadataRequest, POLL_INTERVAL, QueryError, QueryState, Session,
    },
    logs::{LogEvent, LogKind, Severity},
    model::{CatalogRefresh, CatalogSettings, MAX_RESULT_BYTES, Profile, Row},
    storage,
    worker::CredentialProvider,
};
use anyhow::Result;
use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

/// The rows that one fetch of a catalog request asks for.
const METADATA_BATCH: usize = 1_000;
/// The most rows that one catalog request can return.
const MAX_METADATA_ROWS: usize = 200_000;
/// The shortest time between two snapshots during a long refresh.
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(250);
/// Identifies the Logs entries of one refresh across all workers.
static NEXT_BATCH: AtomicU64 = AtomicU64::new(1);
/// The length of one minute of the refresh period and timeout.
pub const MINUTE: Duration = Duration::from_secs(60);
/// The longest wait before the worker checks the clock for an automatic
/// refresh again. A wait does not count the time that the computer sleeps.
const CLOCK_CHECK: Duration = Duration::from_secs(60);
/// The cancel flag value that stops every refresh. Other values stop only
/// the refresh with that Logs batch, and zero stops nothing.
const CANCEL_ALL: u64 = u64::MAX;

/// When the next automatic refresh is due, or `None` if no refresh is due.
/// `last` is the start of the last connection refresh. A catalog that Qrow
/// never read is due at once.
pub fn refresh_due(
    settings: &CatalogSettings,
    warm: bool,
    last: Option<SystemTime>,
    minute: Duration,
) -> Option<SystemTime> {
    if settings.refresh != CatalogRefresh::WhileConnected || !warm {
        return None;
    }
    Some(last.map_or(UNIX_EPOCH, |last| last + minute * settings.refresh_minutes))
}

/// A refresh, and the member whose profile runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub member: Uuid,
    pub scope: Scope,
}

/// The refreshes that a worker performs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// The refresh in progress.
    pub active: Option<Scope>,
    /// The member that runs the refresh in progress.
    pub runner: Option<Uuid>,
    /// The refreshes that wait for the active one.
    pub queued: Vec<Request>,
    /// For a connection refresh, the schemas that are done and all schemas.
    pub done: usize,
    pub total: usize,
}

impl Status {
    /// Return whether `scope` is in progress or waits.
    pub fn includes(&self, scope: &Scope) -> bool {
        self.active.as_ref() == Some(scope)
            || self.queued.iter().any(|request| request.scope == *scope)
    }

    pub fn is_idle(&self) -> bool {
        self.active.is_none() && self.queued.is_empty()
    }

    /// The refreshes of `member` only. A member of a shared catalog shows
    /// the state of its own refreshes; the data of the others arrives in
    /// the catalog.
    pub fn of_member(&self, member: Uuid) -> Status {
        let runs = self.runner == Some(member);
        Status {
            active: self.active.clone().filter(|_| runs),
            runner: self.runner.filter(|_| runs),
            queued: self
                .queued
                .iter()
                .filter(|request| request.member == member)
                .cloned()
                .collect(),
            done: if runs { self.done } else { 0 },
            total: if runs { self.total } else { 0 },
        }
    }
}

pub enum Event {
    Catalog(Arc<Catalog>),
    Status(Status),
}

/// A catalog that a new shared catalog starts with: the private catalog of
/// the connection that made it.
pub enum Seed {
    /// The catalog that the private worker had loaded.
    Catalog(Arc<Catalog>),
    /// The cache file of a private catalog that no worker loaded. The worker
    /// deletes the file.
    File {
        path: PathBuf,
        owner: Uuid,
        identity: CatalogIdentity,
    },
}

enum Command {
    Refresh(Request),
    /// A connection refresh when the catalog has never been read.
    RefreshIfUnloaded(Uuid),
    /// Stop the refreshes of one member, or all refreshes.
    Cancel(Option<Uuid>),
    Configure(Box<CatalogConfig>),
    /// Whether a tab of the member has a live session.
    SetLive(Uuid, bool),
    Seed(Seed),
    Delete,
    Shutdown,
}

/// The request in progress, with the Logs batch of its refresh.
type Target = Arc<Mutex<Option<(u64, Arc<dyn Cancellation>)>>>;
/// The Logs batch and the member of the refresh in progress.
type Running = Arc<Mutex<Option<(u64, Uuid)>>>;

pub struct CatalogWorker {
    tx: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    /// Logs entries, with the member that ran the refresh. Only for members
    /// that enable them.
    pub logs: mpsc::Receiver<(Uuid, LogEvent)>,
    cancelled: Arc<AtomicU64>,
    stopped: Arc<AtomicBool>,
    target: Target,
    running: Running,
    config: Mutex<CatalogConfig>,
    done: mpsc::Receiver<()>,
}

impl CatalogWorker {
    /// `cache` is the cache file, or `None` to keep the catalog only in memory.
    pub fn new(
        config: CatalogConfig,
        cache: Option<PathBuf>,
        wake: Arc<dyn Fn() + Send + Sync>,
        connector: Arc<dyn Connector>,
        credentials: CredentialProvider,
    ) -> Self {
        Self::with_connector(config, cache, wake, connector, credentials, MINUTE)
    }

    /// `minute` is the length of one minute of the refresh period and
    /// timeout. Tests make it shorter.
    pub fn with_connector(
        config: CatalogConfig,
        cache: Option<PathBuf>,
        wake: Arc<dyn Fn() + Send + Sync>,
        connector: Arc<dyn Connector>,
        credentials: CredentialProvider,
        minute: Duration,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let (log_tx, logs) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let cancelled = Arc::new(AtomicU64::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let target: Target = Arc::new(Mutex::new(None));
        let running: Running = Arc::new(Mutex::new(None));
        let runner = Runner {
            catalog: Arc::new(Catalog::empty(config.id, config.identity())),
            config: config.clone(),
            cache,
            queue: VecDeque::new(),
            deferred: VecDeque::new(),
            status: Status::default(),
            session: None,
            interrupted: false,
            dirty: false,
            published: None,
            connector,
            credentials,
            cancelled: cancelled.clone(),
            target: target.clone(),
            running: running.clone(),
            rx,
            tx: events_tx,
            log_tx,
            batch: 0,
            profile: config.members.first().cloned().unwrap_or_default(),
            failures: 0,
            wake,
            live: BTreeSet::new(),
            minute,
            last_refresh: None,
            deadline: None,
            automatic: false,
        };
        thread::spawn(move || {
            runner.run();
            let _ = done_tx.send(());
        });
        Self {
            tx,
            events,
            logs,
            cancelled,
            stopped,
            target,
            running,
            config: Mutex::new(config),
            done,
        }
    }

    /// Read `scope` again with the profile of `member`.
    pub fn refresh(&self, member: Uuid, scope: Scope) {
        let _ = self.tx.send(Command::Refresh(Request { member, scope }));
    }

    /// Refresh the catalog with `member` if Qrow has never read it. The
    /// worker decides after it loads the cache, so a cached catalog is not
    /// read again.
    pub fn refresh_if_unloaded(&self, member: Uuid) {
        let _ = self.tx.send(Command::RefreshIfUnloaded(member));
    }

    /// Tell the worker whether a tab of `member` has a live session. Only
    /// then does it refresh the catalog by itself, with a live member.
    pub fn set_live(&self, member: Uuid, live: bool) {
        let _ = self.tx.send(Command::SetLive(member, live));
    }

    /// Give a new shared catalog the catalog that its first member had. The
    /// worker uses it only if the catalog was never read.
    pub fn seed(&self, seed: Seed) {
        let _ = self.tx.send(Command::Seed(seed));
    }

    /// Stop every refresh in progress and remove the waiting refreshes.
    pub fn cancel(&self) {
        self.cancelled.store(CANCEL_ALL, Ordering::SeqCst);
        self.cancel_target(None);
        let _ = self.tx.send(Command::Cancel(None));
    }

    /// Stop the refresh of `member`, if it runs, and remove its waiting
    /// refreshes. The refreshes of other members continue.
    pub fn stop(&self, member: Uuid) {
        let running = *self.running.lock().unwrap();
        if let Some((batch, runner)) = running
            && runner == member
        {
            let _ = self
                .cancelled
                .compare_exchange(0, batch, Ordering::SeqCst, Ordering::SeqCst);
            self.cancel_target(Some(batch));
        }
        let _ = self.tx.send(Command::Cancel(Some(member)));
    }

    /// Cancel the request in progress, if it belongs to `batch` or `batch`
    /// is `None`.
    fn cancel_target(&self, batch: Option<u64>) {
        let target = self.target.lock().unwrap().clone();
        if let Some((current, cancellation)) = target
            && batch.is_none_or(|batch| batch == current)
        {
            // Cancellation opens a separate transport, so it stays off the caller's thread.
            thread::spawn(move || {
                let _ = cancellation.cancel();
            });
        }
    }

    /// Apply edited settings or members. A private catalog of another
    /// server or user is cleared. A member that leaves stops its refreshes.
    pub fn configure(&self, config: CatalogConfig) {
        let mut current = self.config.lock().unwrap();
        if current.identity() != config.identity() {
            self.cancel();
        } else {
            for member in &current.members {
                if config.member(member.id).is_none() {
                    self.stop(member.id);
                }
            }
        }
        *current = config.clone();
        let _ = self.tx.send(Command::Configure(Box::new(config)));
    }

    /// Stop the worker and delete its cache file.
    pub fn delete(&self) {
        if !self.stopped.swap(true, Ordering::SeqCst) {
            self.cancel();
            let _ = self.tx.send(Command::Delete);
        }
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
}

impl Drop for CatalogWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Why a refresh stopped before its end.
enum Interrupt {
    /// A cancellation or a settings change. The catalog keeps its data.
    Cancelled,
    /// Shutdown or deletion.
    Stopped,
    /// The connection failed. The message goes to the refreshed node.
    Unavailable(String),
    /// The refresh took longer than the timeout. The catalog keeps its data.
    TimedOut,
    /// No tab of the member that runs an automatic refresh has a live
    /// session any more, so the refresh stops. The catalog keeps its data.
    Cold,
}

struct Runner {
    config: CatalogConfig,
    cache: Option<PathBuf>,
    catalog: Arc<Catalog>,
    queue: VecDeque<Request>,
    /// Commands that arrived while a request ran. Refresh commands do not wait here.
    deferred: VecDeque<Command>,
    status: Status,
    /// The open session, and the member whose profile opened it.
    session: Option<(Uuid, Box<dyn Session>)>,
    /// Set when a command cancels the refresh in progress.
    interrupted: bool,
    /// The catalog has changes that are not in the cache file.
    dirty: bool,
    /// The last snapshot that the UI received, and when.
    published: Option<(Arc<Catalog>, Instant)>,
    connector: Arc<dyn Connector>,
    credentials: CredentialProvider,
    cancelled: Arc<AtomicU64>,
    target: Target,
    running: Running,
    rx: mpsc::Receiver<Command>,
    tx: mpsc::Sender<Event>,
    log_tx: mpsc::Sender<(Uuid, LogEvent)>,
    /// The Logs batch of the refresh in progress.
    batch: u64,
    /// The profile of the member that runs the refresh in progress.
    profile: Profile,
    /// The nodes that the refresh in progress could not read.
    failures: usize,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// The members that have a tab with a live session.
    live: BTreeSet<Uuid>,
    minute: Duration,
    /// When the last connection refresh started.
    last_refresh: Option<SystemTime>,
    /// When the refresh in progress times out.
    deadline: Option<Instant>,
    /// The refresh in progress started by itself, not at a request.
    automatic: bool,
}

fn describe_scope(scope: &Scope) -> String {
    match scope {
        Scope::Connection => "the connection".into(),
        Scope::Schema(schema) => format!("schema {schema}"),
        Scope::Relation(schema, relation) => format!("relation {schema}.{relation}"),
    }
}

fn describe_request(request: &MetadataRequest) -> String {
    match request {
        MetadataRequest::Schemas => "List schemas".into(),
        MetadataRequest::Relations {
            schema,
            relation: None,
        } => format!("List relations in {schema}"),
        MetadataRequest::Relations {
            schema,
            relation: Some(relation),
        } => format!("Find relation {schema}.{relation}"),
        MetadataRequest::Columns {
            schema,
            relation: None,
        } => format!("List columns of all relations in {schema}"),
        MetadataRequest::Columns {
            schema,
            relation: Some(relation),
        } => format!("List columns of {schema}.{relation}"),
    }
}

/// The number of items that a request returned, with their name.
fn describe_count(request: &MetadataRequest, count: usize) -> String {
    let (one, many) = match request {
        MetadataRequest::Schemas => ("schema", "schemas"),
        MetadataRequest::Relations { .. } => ("relation", "relations"),
        MetadataRequest::Columns { .. } => ("column", "columns"),
    };
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Parsed catalog results that can tell how many items they hold.
trait Count {
    fn count(&self) -> usize;
}

impl<T> Count for Vec<T> {
    fn count(&self) -> usize {
        self.len()
    }
}

impl<K, T> Count for std::collections::BTreeMap<K, Vec<T>> {
    fn count(&self) -> usize {
        self.values().map(Vec::len).sum()
    }
}

fn format_duration(duration: Duration) -> String {
    format!("{:.2} s", duration.as_secs_f64())
}

/// Cancel the request in progress at `deadline`, also while the worker waits
/// for a call that blocks. Dropping the sender stops the watchdog.
fn watchdog(deadline: Instant, target: Target) -> mpsc::Sender<()> {
    let (stop, stopped) = mpsc::channel::<()>();
    thread::spawn(move || {
        let wait = deadline.saturating_duration_since(Instant::now());
        if stopped.recv_timeout(wait) != Err(RecvTimeoutError::Timeout) {
            return;
        }
        // Release the lock before the network call, so `cancel` and
        // `shutdown` on the window thread do not wait for it.
        let cancellation = target.lock().unwrap().clone();
        if let Some((_, cancellation)) = cancellation {
            let _ = cancellation.cancel();
        }
    });
    stop
}

fn past(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() >= deadline)
}

fn format_minutes(minutes: u32) -> String {
    if minutes == 1 {
        "1 minute".into()
    } else {
        format!("{minutes} minutes")
    }
}

impl Runner {
    fn run(mut self) {
        let identity = self.config.identity();
        if let Some(cached) = self
            .cache
            .as_deref()
            .and_then(storage::load_catalog)
            .filter(|catalog| catalog.describes(self.config.id, identity.as_ref()))
        {
            self.adopt(cached);
        }
        self.publish(true);
        'commands: while let Some(command) = self.next_command() {
            if !self.handle(command) {
                break;
            }
            while let Some(request) = self.queue.pop_front() {
                let Some(profile) = self.config.member(request.member).cloned() else {
                    continue;
                };
                let scope = request.scope;
                self.profile = profile;
                self.batch = NEXT_BATCH.fetch_add(1, Ordering::Relaxed);
                *self.running.lock().unwrap() = Some((self.batch, self.profile.id));
                self.status = Status {
                    active: Some(scope.clone()),
                    runner: Some(self.profile.id),
                    queued: self.queue.iter().cloned().collect(),
                    done: 0,
                    total: 0,
                };
                self.interrupted = false;
                self.publish(true);
                self.failures = 0;
                let started = Instant::now();
                let timeout = self.config.settings.timeout_minutes;
                let deadline = started + self.minute * timeout;
                self.deadline = Some(deadline);
                let watchdog = watchdog(deadline, self.target.clone());
                self.log(
                    Severity::Info,
                    format!(
                        "Started {} schema refresh of {}",
                        if self.automatic { "an automatic" } else { "a" },
                        describe_scope(&scope)
                    ),
                    None,
                );
                let result = self.refresh(&scope);
                drop(watchdog);
                self.deadline = None;
                self.automatic = false;
                let cancelled_all = self.cancelled.load(Ordering::SeqCst) == CANCEL_ALL;
                *self.running.lock().unwrap() = None;
                let _ = self.cancelled.compare_exchange(
                    self.batch,
                    0,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                let duration = started.elapsed();
                let outcome = match &result {
                    Ok(()) if self.failures == 1 => {
                        "Schema refresh completed with 1 error".to_owned()
                    }
                    Ok(()) if self.failures > 1 => {
                        format!("Schema refresh completed with {} errors", self.failures)
                    }
                    Ok(()) => "Schema refresh completed".to_owned(),
                    Err(Interrupt::Cancelled) => "Schema refresh cancelled".to_owned(),
                    Err(Interrupt::Stopped) => "Schema refresh stopped".to_owned(),
                    Err(Interrupt::Unavailable(message)) => {
                        format!("Schema refresh failed: {message}")
                    }
                    Err(Interrupt::TimedOut) => {
                        format!("Schema refresh stopped after {}", format_minutes(timeout))
                    }
                    Err(Interrupt::Cold) => {
                        "Schema refresh stopped because no tab of the connection is connected"
                            .to_owned()
                    }
                };
                let severity = if self.failures > 0
                    || matches!(result, Err(Interrupt::Unavailable(_) | Interrupt::TimedOut))
                {
                    Severity::Error
                } else {
                    Severity::Info
                };
                self.log_as(
                    LogKind::SchemaRefreshFinished,
                    severity,
                    format!(
                        "{outcome} (client measurement: {})",
                        format_duration(duration)
                    ),
                    Some(duration),
                );
                let member = self.profile.id;
                match result {
                    Ok(()) => {}
                    // The `Cancel` command can arrive after the flag stops the
                    // request. It also clears the queue, so do it here.
                    Err(Interrupt::Cancelled) if cancelled_all => self.queue.clear(),
                    Err(Interrupt::Cancelled) => {
                        self.queue.retain(|queued| queued.member != member)
                    }
                    Err(Interrupt::Stopped) => break 'commands,
                    Err(Interrupt::Unavailable(message)) => {
                        // The other refreshes of the member would fail in the
                        // same way. Other members connect with other settings.
                        self.update(|catalog| catalog.set_error(&scope, message.clone(), member));
                        let (failed, kept) = std::mem::take(&mut self.queue)
                            .into_iter()
                            .partition(|queued| queued.member == member);
                        self.queue = kept;
                        for queued in failed {
                            self.update(|catalog| {
                                catalog.set_error(&queued.scope, message.clone(), member)
                            });
                        }
                    }
                    Err(Interrupt::TimedOut) => {
                        let message = format!("Refresh stopped after {}", format_minutes(timeout));
                        self.update(|catalog| catalog.set_error(&scope, message, member));
                        // The cancelled request can still hold the session.
                        self.close_session();
                    }
                    Err(Interrupt::Cold) => {}
                }
                self.save();
            }
            self.close_session();
            self.status = Status::default();
            self.publish(true);
        }
        self.close_session();
    }

    /// Use a catalog that was read before: the cache file, or the seed of a
    /// new shared catalog.
    fn adopt(&mut self, mut catalog: Catalog) {
        catalog.owner = self.config.id;
        catalog.identity = self.config.identity();
        catalog.retain(&self.config.settings);
        self.last_refresh = catalog
            .fetched_at
            .map(|at| UNIX_EPOCH + Duration::from_secs(at));
        self.catalog = Arc::new(catalog);
    }

    /// The member that runs an automatic refresh: the preferred member while
    /// it has a live session, else the first member with a live session.
    fn automatic_runner(&self) -> Option<Uuid> {
        self.config
            .preferred
            .filter(|preferred| self.live.contains(preferred))
            .or_else(|| {
                self.config
                    .members
                    .iter()
                    .map(|member| member.id)
                    .find(|member| self.live.contains(member))
            })
            .filter(|member| self.config.member(*member).is_some())
    }

    /// Wait for the next command. While an automatic refresh is pending,
    /// wake at its time and return a connection refresh.
    fn next_command(&mut self) -> Option<Command> {
        if let Some(command) = self.deferred.pop_front() {
            return Some(command);
        }
        loop {
            let runner = self.automatic_runner();
            let Some(due) = refresh_due(
                &self.config.settings,
                runner.is_some(),
                self.last_refresh,
                self.minute,
            ) else {
                return self.rx.recv().ok();
            };
            // A due time in the past is an error, which means no wait.
            let wait = due.duration_since(SystemTime::now()).unwrap_or_default();
            if wait.is_zero()
                && let Some(member) = runner
            {
                // A waiting command can change the policy or the live session.
                if let Ok(command) = self.rx.try_recv() {
                    return Some(command);
                }
                self.automatic = true;
                return Some(Command::Refresh(Request {
                    member,
                    scope: Scope::Connection,
                }));
            }
            match self.rx.recv_timeout(wait.min(CLOCK_CHECK)) {
                Ok(command) => return Some(command),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// Apply one command. Returns `false` when the worker must stop.
    fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Refresh(request) => {
                if self.config.member(request.member).is_none() {
                    return true;
                }
                // A refresh of any member fills the catalog for all of them.
                let covered = self
                    .status
                    .active
                    .iter()
                    .chain(self.queue.iter().map(|queued| &queued.scope))
                    .any(|queued| queued.covers(&request.scope));
                if !covered {
                    self.queue
                        .retain(|queued| !request.scope.covers(&queued.scope));
                    self.queue.push_back(request);
                    self.status.queued = self.queue.iter().cloned().collect();
                    self.publish(true);
                }
            }
            Command::RefreshIfUnloaded(member) => {
                if self.catalog.fetched_at.is_none() {
                    return self.handle(Command::Refresh(Request {
                        member,
                        scope: Scope::Connection,
                    }));
                }
            }
            Command::Cancel(None) => {
                self.cancelled.store(0, Ordering::SeqCst);
                self.interrupted = true;
                self.queue.clear();
                self.status.queued.clear();
            }
            Command::Cancel(Some(member)) => {
                let active = self.status.active.is_some();
                // A flag of the refresh in progress belongs to a later stop.
                let _ = self
                    .cancelled
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |flag| {
                        (flag != CANCEL_ALL && !(active && flag == self.batch)).then_some(0)
                    });
                if active && self.profile.id == member {
                    self.interrupted = true;
                }
                self.queue.retain(|queued| queued.member != member);
                self.status.queued = self.queue.iter().cloned().collect();
                self.publish(true);
            }
            Command::Configure(config) => self.configure(*config),
            Command::SetLive(member, live) => {
                if live {
                    self.live.insert(member);
                } else {
                    self.live.remove(&member);
                }
            }
            Command::Seed(seed) => self.seed(seed),
            Command::Delete => {
                if let Some(path) = &self.cache {
                    storage::delete_catalog(path);
                }
                self.cache = None;
                return false;
            }
            Command::Shutdown => return false,
        }
        true
    }

    fn configure(&mut self, config: CatalogConfig) {
        if self.catalog.identity != config.identity() {
            self.close_session();
            self.catalog = Arc::new(Catalog::empty(config.id, config.identity()));
            self.last_refresh = None;
            self.dirty = false;
            if let Some(path) = &self.cache {
                storage::delete_catalog(path);
            }
        } else {
            // The next refresh opens a session with the new settings.
            let stale = self.session.as_ref().is_some_and(|(member, _)| {
                config
                    .member(*member)
                    .is_none_or(|profile| !profile.connection_identity_eq(&self.profile))
            });
            if stale {
                self.close_session();
            }
            if config.settings != self.config.settings {
                let settings = config.settings.clone();
                self.update(|catalog| catalog.retain(&settings));
            }
        }
        if let Some(profile) = config.member(self.profile.id) {
            self.profile = profile.clone();
        }
        self.live.retain(|member| config.member(*member).is_some());
        self.queue
            .retain(|queued| config.member(queued.member).is_some());
        self.status.queued = self.queue.iter().cloned().collect();
        self.config = config;
        self.publish(true);
    }

    /// Start a new shared catalog with the private catalog of its first
    /// member, if the shared catalog was never read.
    fn seed(&mut self, seed: Seed) {
        let catalog = match seed {
            Seed::Catalog(catalog) => Some(Arc::unwrap_or_clone(catalog)),
            Seed::File {
                path,
                owner,
                identity,
            } => {
                let catalog = storage::load_catalog(&path)
                    .filter(|catalog| catalog.describes(owner, Some(&identity)));
                storage::delete_catalog(&path);
                catalog
            }
        };
        if let Some(catalog) = catalog
            && self.catalog.fetched_at.is_none()
            && catalog.fetched_at.is_some()
        {
            self.adopt(catalog);
            Arc::make_mut(&mut self.catalog).error = None;
            self.dirty = true;
            self.save();
            self.publish(true);
        }
    }

    /// Apply the commands that arrived during a refresh, then stop the
    /// refresh if a command cancelled it.
    fn checkpoint(&mut self) -> Result<(), Interrupt> {
        while let Some(command) = self
            .deferred
            .pop_front()
            .or_else(|| self.rx.try_recv().ok())
        {
            if !self.handle(command) {
                return Err(Interrupt::Stopped);
            }
        }
        if self.interrupted || self.cancel_requested() {
            return Err(Interrupt::Cancelled);
        }
        if self.expired() {
            return Err(Interrupt::TimedOut);
        }
        if self.cold() {
            return Err(Interrupt::Cold);
        }
        Ok(())
    }

    /// Whether a cancellation stops the refresh in progress.
    fn cancel_requested(&self) -> bool {
        let flag = self.cancelled.load(Ordering::SeqCst);
        flag == CANCEL_ALL || flag == self.batch
    }

    /// Whether the refresh in progress is automatic and its member has no
    /// live session any more.
    fn cold(&self) -> bool {
        self.automatic && !self.live.contains(&self.profile.id)
    }

    /// Whether the refresh in progress took longer than its timeout.
    fn expired(&self) -> bool {
        past(self.deadline)
    }

    /// Whether the refresh in progress must stop: it timed out, or it is
    /// automatic and its member has no live session.
    fn must_stop(&self) -> bool {
        self.expired() || self.cold()
    }

    fn refresh(&mut self, scope: &Scope) -> Result<(), Interrupt> {
        match scope {
            Scope::Connection => self.refresh_connection(),
            Scope::Schema(schema) => self.refresh_schema(schema),
            Scope::Relation(schema, relation) => self.refresh_relation(schema, relation),
        }
    }

    fn refresh_connection(&mut self) -> Result<(), Interrupt> {
        self.last_refresh = Some(SystemTime::now());
        let names = match self.read_parsed(&MetadataRequest::Schemas, parse_schemas)? {
            Ok(names) => names,
            Err(message) => {
                self.fail(&Scope::Connection, message);
                return Ok(());
            }
        };
        let settings = self.config.settings.clone();
        let started = now();
        self.update(|catalog| catalog.apply_schemas(names, &settings, started));
        // A refresh that stopped in the refresh period continues: the schemas
        // that it read are not read again. Otherwise the refresh starts over.
        let period = self.minute * settings.refresh_minutes;
        let unfinished = self
            .catalog
            .unfinished
            .clone()
            .filter(|unfinished| {
                Duration::from_secs(started.saturating_sub(unfinished.started)) < period
            })
            .unwrap_or(Unfinished {
                started,
                done: BTreeSet::new(),
            });
        // The schemas that Qrow read longest ago go first. A refresh that the
        // timeout stops then does not leave the same schemas old each time.
        let mut schemas: Vec<(Option<u64>, String)> = self
            .catalog
            .schemas
            .iter()
            .filter(|(name, _)| !unfinished.done.contains(*name))
            .map(|(name, schema)| (schema.fetched_at, name.clone()))
            .collect();
        schemas.sort();
        let schemas: Vec<String> = schemas.into_iter().map(|(_, name)| name).collect();
        self.status.total = self.catalog.schemas.len();
        self.status.done = self.status.total - schemas.len();
        if self.status.done > 0 {
            self.log(
                Severity::Info,
                format!(
                    "Continued a stopped schema refresh: {} of {} schemas were already read",
                    self.status.done, self.status.total
                ),
                None,
            );
        }
        self.update(|catalog| catalog.unfinished = Some(unfinished));
        self.publish(true);
        // Each schema is one step: its relations, then their columns. No
        // request reads the columns of the whole connection at once.
        for schema in schemas {
            self.checkpoint()?;
            if self.catalog.schema(&schema).is_none() {
                self.status.total = self.status.total.saturating_sub(1);
                continue;
            }
            let failures = self.failures;
            self.refresh_schema(&schema)?;
            if self.failures == failures && self.catalog.schema(&schema).is_some() {
                self.update(|catalog| {
                    if let Some(unfinished) = &mut catalog.unfinished {
                        unfinished.done.insert(schema);
                    }
                });
            }
            self.status.done += 1;
            self.publish(false);
        }
        self.update(|catalog| catalog.unfinished = None);
        Ok(())
    }

    fn refresh_schema(&mut self, schema: &str) -> Result<(), Interrupt> {
        if !self.read_relations(schema, None)? {
            return Ok(());
        }
        self.checkpoint()?;
        let request = MetadataRequest::Columns {
            schema: schema.into(),
            relation: None,
        };
        match self.read_parsed(&request, |rows| parse_columns(rows, schema, None))? {
            Ok(columns) => {
                self.update(|catalog| catalog.apply_columns(schema, None, columns, now()));
                Ok(())
            }
            // One broken view can fail the request for the whole schema. The
            // requests for each relation decide which relations fail.
            Err(_) => {
                let relations: Vec<String> = self
                    .catalog
                    .schema(schema)
                    .and_then(|node| node.relations.as_ref())
                    .map(|relations| relations.keys().cloned().collect())
                    .unwrap_or_default();
                for relation in relations {
                    self.checkpoint()?;
                    self.read_columns(schema, &relation)?;
                    self.publish(false);
                }
                Ok(())
            }
        }
    }

    fn refresh_relation(&mut self, schema: &str, relation: &str) -> Result<(), Interrupt> {
        if self.read_relations(schema, Some(relation))?
            && self.catalog.relation(schema, relation).is_some()
        {
            self.checkpoint()?;
            self.read_columns(schema, relation)?;
        }
        Ok(())
    }

    /// Read a relation list. Returns whether it succeeded.
    fn read_relations(&mut self, schema: &str, relation: Option<&str>) -> Result<bool, Interrupt> {
        let request = MetadataRequest::Relations {
            schema: schema.into(),
            relation: relation.map(str::to_owned),
        };
        let scope = match relation {
            Some(relation) => Scope::Relation(schema.into(), relation.into()),
            None => Scope::Schema(schema.into()),
        };
        match self.read_parsed(&request, |rows| parse_relations(rows, schema, relation))? {
            Ok(entries) => {
                self.update(|catalog| catalog.apply_relations(schema, relation, entries, now()));
                Ok(true)
            }
            Err(message) => {
                self.fail(&scope, message);
                Ok(false)
            }
        }
    }

    fn read_columns(&mut self, schema: &str, relation: &str) -> Result<(), Interrupt> {
        let request = MetadataRequest::Columns {
            schema: schema.into(),
            relation: Some(relation.into()),
        };
        match self.read_parsed(&request, |rows| parse_columns(rows, schema, Some(relation)))? {
            Ok(columns) => {
                self.update(|catalog| catalog.apply_columns(schema, Some(relation), columns, now()))
            }
            Err(message) => self.fail(&Scope::Relation(schema.into(), relation.into()), message),
        }
        Ok(())
    }

    /// Read a catalog request and parse its rows. A parse error is a failed
    /// request too, so it gets its own Logs entry.
    fn read_parsed<T: Count>(
        &mut self,
        request: &MetadataRequest,
        parse: impl FnOnce(&MetadataRows) -> Result<T>,
    ) -> Result<Result<T, String>, Interrupt> {
        let label = describe_request(request);
        let (rows, duration) = match self.read(request)? {
            Ok(read) => read,
            Err(message) => return Ok(Err(message)),
        };
        match parse(&rows) {
            // The count is of the requested names only. The server can also
            // return rows of names that match `_` as a pattern.
            Ok(parsed) => {
                self.log(
                    Severity::Info,
                    format!(
                        "{label}: {} (client measurement: {})",
                        describe_count(request, parsed.count()),
                        format_duration(duration)
                    ),
                    Some(duration),
                );
                Ok(Ok(parsed))
            }
            Err(error) => {
                let message = format!("{error:#}");
                self.log(
                    Severity::Error,
                    format!("{label} returned an unexpected result: {message}"),
                    Some(duration),
                );
                Ok(Err(message))
            }
        }
    }

    /// Record an error on the node of `scope`. The refresh reports it at its end.
    fn fail(&mut self, scope: &Scope, message: String) {
        self.failures += 1;
        let member = self.profile.id;
        self.update(|catalog| catalog.set_error(scope, message, member));
    }

    /// Read all rows of a catalog request. An outer error stops the refresh;
    /// an inner error belongs to the node that the request reads.
    fn read(
        &mut self,
        request: &MetadataRequest,
    ) -> Result<Result<(MetadataRows, Duration), String>, Interrupt> {
        let started = Instant::now();
        let result = self.read_rows(request);
        let duration = started.elapsed();
        *self.target.lock().unwrap() = None;
        self.checkpoint()?;
        let label = describe_request(request);
        match result {
            Ok(rows) => Ok(Ok((rows, duration))),
            Err(error) => {
                let message = crate::connector::error_message(&error);
                self.log(
                    Severity::Error,
                    format!("{label} failed: {message}"),
                    Some(duration),
                );
                let healthy = error.downcast_ref::<QueryError>().is_some()
                    && self
                        .session
                        .as_mut()
                        .is_some_and(|(_, session)| session.close_operation().is_ok());
                if healthy {
                    Ok(Err(message))
                } else {
                    self.close_session();
                    Err(Interrupt::Unavailable(message))
                }
            }
        }
    }

    fn read_rows(&mut self, request: &MetadataRequest) -> Result<MetadataRows> {
        // Opening a session and starting a request can take until the
        // network timeout of the connector, and the watchdog cannot cancel them.
        anyhow::ensure!(!self.must_stop(), "The catalog request was stopped");
        // Each member reads with its own user and settings.
        if self
            .session
            .as_ref()
            .is_some_and(|(member, _)| *member != self.profile.id)
        {
            self.close_session();
        }
        if self.session.is_none() {
            let started = Instant::now();
            let secret = (self.credentials)(&self.profile)?;
            let session = self.connector.connect(&self.profile, secret)?;
            self.session = Some((self.profile.id, session));
            let duration = started.elapsed();
            self.log(
                Severity::Info,
                format!(
                    "Opened a session for the schema refresh (client measurement: {})",
                    format_duration(duration)
                ),
                Some(duration),
            );
            anyhow::ensure!(!self.must_stop(), "The catalog request was stopped");
        }
        let session = self.session();
        let cancellation = session.execute_metadata(request)?;
        *self.target.lock().unwrap() = Some((self.batch, cancellation.clone()));
        if self.cancel_requested() {
            cancellation.cancel()?;
        }
        if self.must_stop() {
            Self::cancel_request(&cancellation);
            anyhow::bail!("The catalog request was stopped");
        }
        let has_results = loop {
            match self.session().poll()? {
                QueryState::Finished { has_results } => break has_results,
                QueryState::Cancelled => anyhow::bail!("The catalog request was cancelled"),
                QueryState::Running => {}
            }
            // Do not wait for the server to confirm a cancellation. The next
            // request closes the operation.
            anyhow::ensure!(
                !self.cancel_requested(),
                "The catalog request was cancelled"
            );
            self.accept_refreshes();
            if self.must_stop() {
                Self::cancel_request(&cancellation);
                anyhow::bail!("The catalog request was stopped");
            }
            thread::sleep(POLL_INTERVAL);
        };
        if !has_results {
            self.session().close_operation()?;
            return Ok(MetadataRows {
                columns: vec![],
                rows: vec![],
            });
        }
        let columns = self.session().columns()?;
        let mut rows: Vec<Row> = Vec::new();
        let mut bytes = 0usize;
        loop {
            let batch = self.session().fetch(METADATA_BATCH)?;
            if batch.rows.is_empty() {
                break;
            }
            bytes = bytes.saturating_add(
                batch
                    .rows
                    .iter()
                    .flatten()
                    .flatten()
                    .map(String::capacity)
                    .sum(),
            );
            rows.extend(batch.rows);
            anyhow::ensure!(
                rows.len() <= MAX_METADATA_ROWS && bytes <= MAX_RESULT_BYTES,
                "The catalog request returned more than {MAX_METADATA_ROWS} rows or {} MB. Hide some schemas in the connection settings.",
                MAX_RESULT_BYTES / 1024 / 1024
            );
            if self.cancel_requested() {
                anyhow::bail!("The catalog request was cancelled");
            }
            self.accept_refreshes();
            if self.must_stop() {
                Self::cancel_request(&cancellation);
                anyhow::bail!("The catalog request was stopped");
            }
        }
        self.session().close_operation()?;
        Ok(MetadataRows { columns, rows })
    }

    fn session(&mut self) -> &mut Box<dyn Session> {
        &mut self.session.as_mut().unwrap().1
    }

    /// Cancel a request through the separate cancel transport, off this thread.
    fn cancel_request(cancellation: &Arc<dyn Cancellation>) {
        let cancellation = cancellation.clone();
        thread::spawn(move || {
            let _ = cancellation.cancel();
        });
    }

    /// Queue the refreshes that arrived while a request runs, so the UI shows
    /// them as waiting, and apply the live-session state and the stops of
    /// members. Keep the other commands for the next checkpoint.
    fn accept_refreshes(&mut self) {
        while let Ok(command) = self.rx.try_recv() {
            match command {
                Command::Refresh(_)
                | Command::RefreshIfUnloaded(_)
                | Command::SetLive(..)
                | Command::Cancel(Some(_)) => {
                    self.handle(command);
                }
                command => self.deferred.push_back(command),
            }
        }
    }

    /// Send an Activity entry of the refresh in progress, for the member
    /// that runs it.
    fn log(&self, severity: Severity, text: String, duration: Option<Duration>) {
        self.log_as(LogKind::SchemaRefresh, severity, text, duration);
    }

    fn log_as(&self, kind: LogKind, severity: Severity, text: String, duration: Option<Duration>) {
        let mut event =
            LogEvent::new(None, severity, kind, text).with_connection(self.profile.name.clone());
        event.duration = duration;
        let _ = self.log_tx.send((self.profile.id, event));
        (self.wake)();
    }

    fn update(&mut self, change: impl FnOnce(&mut Catalog)) {
        change(Arc::make_mut(&mut self.catalog));
        self.dirty = true;
    }

    /// Send the status and a changed catalog to the UI. Without `force`,
    /// send at most one snapshot in each [`SNAPSHOT_INTERVAL`].
    fn publish(&mut self, force: bool) {
        if !force
            && self
                .published
                .as_ref()
                .is_some_and(|(_, at)| at.elapsed() < SNAPSHOT_INTERVAL)
        {
            return;
        }
        if self
            .published
            .as_ref()
            .is_none_or(|(published, _)| !Arc::ptr_eq(published, &self.catalog))
        {
            let _ = self.tx.send(Event::Catalog(self.catalog.clone()));
        }
        let _ = self.tx.send(Event::Status(self.status.clone()));
        self.published = Some((self.catalog.clone(), Instant::now()));
        (self.wake)();
    }

    fn save(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if let Some(path) = &self.cache
            && let Err(error) = storage::save_catalog(path, &self.catalog)
        {
            eprintln!("{error:#}");
        }
    }

    fn close_session(&mut self) {
        if let Some((_, mut session)) = self.session.take() {
            let _ = session.close();
        }
    }
}
