//! A thread that reads the catalog of one connection.
//!
//! The worker opens its own session for the queued refreshes and closes it
//! when the queue is empty, so it never changes the idle timer or the result
//! cursor of a query tab. While a tab of the connection has a live session,
//! the worker also refreshes the connection when its refresh period passes.

use super::{Catalog, MetadataRows, Scope, now, parse_columns, parse_relations, parse_schemas};
use crate::{
    activity::{ActivityEvent, ActivityKind, Severity},
    connector::{
        Cancellation, Connector, MetadataRequest, POLL_INTERVAL, QueryError, QueryState, Session,
        hive::HiveConnector,
    },
    model::{CatalogRefresh, MAX_RESULT_BYTES, Profile, Row},
    storage::{self, Credentials},
    worker::PasswordProvider,
};
use anyhow::Result;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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

/// When the next automatic refresh is due, or `None` if no refresh is due.
/// `last` is the start of the last connection refresh. A catalog that Qrow
/// never read is due at once.
pub fn refresh_due(
    refresh: CatalogRefresh,
    warm: bool,
    last: Option<SystemTime>,
    minute: Duration,
) -> Option<SystemTime> {
    let CatalogRefresh::WhileConnected { minutes } = refresh else {
        return None;
    };
    if !warm {
        return None;
    }
    Some(last.map_or(UNIX_EPOCH, |last| last + minute * minutes))
}

/// The refreshes that a worker performs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// The refresh in progress.
    pub active: Option<Scope>,
    /// The refreshes that wait for the active one.
    pub queued: Vec<Scope>,
    /// For a connection refresh, the schemas that are done and all schemas.
    pub done: usize,
    pub total: usize,
}

impl Status {
    /// Return whether `scope` is in progress or waits.
    pub fn includes(&self, scope: &Scope) -> bool {
        self.active.as_ref() == Some(scope) || self.queued.contains(scope)
    }

    pub fn is_idle(&self) -> bool {
        self.active.is_none() && self.queued.is_empty()
    }
}

pub enum Event {
    Catalog(Arc<Catalog>),
    Status(Status),
}

enum Command {
    Refresh(Scope),
    /// A connection refresh when the catalog has never been read.
    RefreshIfUnloaded,
    Cancel,
    UpdateProfile(Box<Profile>),
    /// Whether a tab of the connection has a live session.
    SetWarm(bool),
    Delete,
    Shutdown,
}

type Target = Arc<Mutex<Option<Arc<dyn Cancellation>>>>;

pub struct CatalogWorker {
    tx: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    /// Logs entries, only when the profile enables them.
    pub activities: mpsc::Receiver<ActivityEvent>,
    cancelled: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    target: Target,
    profile: Mutex<Profile>,
    done: mpsc::Receiver<()>,
}

impl CatalogWorker {
    /// `cache` is the cache file, or `None` to keep the catalog only in memory.
    pub fn new(
        profile: Profile,
        cache: Option<PathBuf>,
        wake: Arc<dyn Fn() + Send + Sync>,
        credentials: Arc<dyn Credentials>,
    ) -> Self {
        Self::with_connector(
            profile,
            cache,
            wake,
            Arc::new(HiveConnector),
            Arc::new(move |profile| credentials.password(profile.id)),
            MINUTE,
        )
    }

    /// `minute` is the length of one minute of the refresh period and
    /// timeout. Tests make it shorter.
    pub fn with_connector(
        profile: Profile,
        cache: Option<PathBuf>,
        wake: Arc<dyn Fn() + Send + Sync>,
        connector: Arc<dyn Connector>,
        passwords: PasswordProvider,
        minute: Duration,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let (activity_tx, activities) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let target: Target = Arc::new(Mutex::new(None));
        let runner = Runner {
            catalog: Arc::new(Catalog::new(&profile)),
            profile: profile.clone(),
            cache,
            queue: VecDeque::new(),
            deferred: VecDeque::new(),
            status: Status::default(),
            session: None,
            interrupted: false,
            dirty: false,
            published: None,
            connector,
            passwords,
            cancelled: cancelled.clone(),
            target: target.clone(),
            rx,
            tx: events_tx,
            activity: activity_tx,
            batch: 0,
            failures: 0,
            wake,
            warm: false,
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
            activities,
            cancelled,
            stopped,
            target,
            profile: Mutex::new(profile),
            done,
        }
    }

    pub fn refresh(&self, scope: Scope) {
        let _ = self.tx.send(Command::Refresh(scope));
    }

    /// Refresh the connection if Qrow has never read its catalog. The worker
    /// decides after it loads the cache, so a cached catalog is not read again.
    pub fn refresh_if_unloaded(&self) {
        let _ = self.tx.send(Command::RefreshIfUnloaded);
    }

    /// Tell the worker whether a tab of the connection has a live session.
    /// Only then does it refresh the connection by itself.
    pub fn set_warm(&self, warm: bool) {
        let _ = self.tx.send(Command::SetWarm(warm));
    }

    /// Stop the refresh in progress and remove the waiting refreshes.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(cancellation) = self.target.lock().unwrap().clone() {
            // Cancellation opens a separate transport, so it stays off the caller's thread.
            thread::spawn(move || {
                let _ = cancellation.cancel();
            });
        }
        let _ = self.tx.send(Command::Cancel);
    }

    /// Apply edited settings. A different server or user clears the catalog.
    pub fn update_profile(&self, profile: Profile) {
        let mut current = self.profile.lock().unwrap();
        if !Catalog::new(&current).describes(&profile) {
            self.cancel();
        }
        *current = profile.clone();
        let _ = self.tx.send(Command::UpdateProfile(Box::new(profile)));
    }

    /// Stop the worker and delete the cache file of its profile.
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
    /// A cancellation or a profile change. The catalog keeps its data.
    Cancelled,
    /// Shutdown or deletion.
    Stopped,
    /// The connection failed. The message goes to the refreshed node.
    Unavailable(String),
    /// The refresh took longer than the timeout. The catalog keeps its data.
    TimedOut,
    /// No tab of the connection has a live session any more, so an
    /// automatic refresh stops. The catalog keeps its data.
    Cold,
}

struct Runner {
    profile: Profile,
    cache: Option<PathBuf>,
    catalog: Arc<Catalog>,
    queue: VecDeque<Scope>,
    /// Commands that arrived while a request ran. Refresh commands do not wait here.
    deferred: VecDeque<Command>,
    status: Status,
    session: Option<Box<dyn Session>>,
    /// Set when a command cancels the refresh in progress.
    interrupted: bool,
    /// The catalog has changes that are not in the cache file.
    dirty: bool,
    /// The last snapshot that the UI received, and when.
    published: Option<(Arc<Catalog>, Instant)>,
    connector: Arc<dyn Connector>,
    passwords: PasswordProvider,
    cancelled: Arc<AtomicBool>,
    target: Target,
    rx: mpsc::Receiver<Command>,
    tx: mpsc::Sender<Event>,
    activity: mpsc::Sender<ActivityEvent>,
    /// The Logs batch of the refresh in progress.
    batch: u64,
    /// The nodes that the refresh in progress could not read.
    failures: usize,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Whether a tab of the connection has a live session.
    warm: bool,
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
        if stopped.recv_timeout(wait) == Err(RecvTimeoutError::Timeout)
            && let Some(cancellation) = target.lock().unwrap().clone()
        {
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
        if let Some(cached) = self
            .cache
            .as_deref()
            .and_then(storage::load_catalog)
            .filter(|catalog| catalog.describes(&self.profile))
        {
            self.catalog = Arc::new(cached);
            Arc::make_mut(&mut self.catalog).retain(&self.profile.catalog);
            self.last_refresh = self
                .catalog
                .fetched_at
                .map(|at| UNIX_EPOCH + Duration::from_secs(at));
        }
        self.publish(true);
        'commands: while let Some(command) = self.next_command() {
            if !self.handle(command) {
                break;
            }
            while let Some(scope) = self.queue.pop_front() {
                self.status = Status {
                    active: Some(scope.clone()),
                    queued: self.queue.iter().cloned().collect(),
                    done: 0,
                    total: 0,
                };
                self.interrupted = false;
                self.publish(true);
                self.batch = NEXT_BATCH.fetch_add(1, Ordering::Relaxed);
                self.failures = 0;
                let started = Instant::now();
                let timeout = self.profile.catalog.timeout_minutes;
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
                self.log(
                    severity,
                    format!(
                        "{outcome} (client measurement: {})",
                        format_duration(duration)
                    ),
                    Some(duration),
                );
                match result {
                    Ok(()) => {}
                    // The `Cancel` command can arrive after the flag stops the
                    // request. It also clears the queue, so do it here.
                    Err(Interrupt::Cancelled) => self.queue.clear(),
                    Err(Interrupt::Stopped) => break 'commands,
                    Err(Interrupt::Unavailable(message)) => {
                        // The other refreshes would fail in the same way.
                        self.update(|catalog| {
                            catalog.set_error(&scope, message.clone());
                        });
                        for scope in std::mem::take(&mut self.queue) {
                            self.update(|catalog| catalog.set_error(&scope, message.clone()));
                        }
                    }
                    Err(Interrupt::TimedOut) => {
                        let message = format!("Refresh stopped after {}", format_minutes(timeout));
                        self.update(|catalog| catalog.set_error(&scope, message));
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

    /// Wait for the next command. While an automatic refresh is pending,
    /// wake at its time and return a connection refresh.
    fn next_command(&mut self) -> Option<Command> {
        if let Some(command) = self.deferred.pop_front() {
            return Some(command);
        }
        loop {
            let Some(due) = refresh_due(
                self.profile.catalog.refresh,
                self.warm,
                self.last_refresh,
                self.minute,
            ) else {
                return self.rx.recv().ok();
            };
            // A due time in the past is an error, which means no wait.
            let wait = due.duration_since(SystemTime::now()).unwrap_or_default();
            if wait.is_zero() {
                // A waiting command can change the policy or the live session.
                if let Ok(command) = self.rx.try_recv() {
                    return Some(command);
                }
                self.automatic = true;
                return Some(Command::Refresh(Scope::Connection));
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
            Command::Refresh(scope) => {
                let covered = self
                    .status
                    .active
                    .iter()
                    .chain(&self.queue)
                    .any(|queued| queued.covers(&scope));
                if !covered {
                    self.queue.retain(|queued| !scope.covers(queued));
                    self.queue.push_back(scope);
                    self.status.queued = self.queue.iter().cloned().collect();
                    self.publish(true);
                }
            }
            Command::RefreshIfUnloaded => {
                if self.catalog.fetched_at.is_none() {
                    return self.handle(Command::Refresh(Scope::Connection));
                }
            }
            Command::Cancel => {
                self.cancelled.store(false, Ordering::SeqCst);
                self.interrupted = true;
                self.queue.clear();
                self.status.queued.clear();
            }
            Command::UpdateProfile(profile) => {
                if !self.catalog.describes(&profile) {
                    self.close_session();
                    self.catalog = Arc::new(Catalog::new(&profile));
                    self.last_refresh = None;
                    self.dirty = false;
                    if let Some(path) = &self.cache {
                        storage::delete_catalog(path);
                    }
                } else {
                    if !self.profile.connection_identity_eq(&profile) {
                        // The next refresh opens a session with the new settings.
                        self.close_session();
                    }
                    if profile.catalog != self.profile.catalog {
                        self.update(|catalog| catalog.retain(&profile.catalog));
                    }
                }
                self.profile = *profile;
                self.publish(true);
            }
            Command::SetWarm(warm) => self.warm = warm,
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
        if self.interrupted || self.cancelled.load(Ordering::SeqCst) {
            return Err(Interrupt::Cancelled);
        }
        if self.expired() {
            return Err(Interrupt::TimedOut);
        }
        if self.automatic && !self.warm {
            return Err(Interrupt::Cold);
        }
        Ok(())
    }

    /// Whether the refresh in progress took longer than its timeout.
    fn expired(&self) -> bool {
        past(self.deadline)
    }

    /// Whether the refresh in progress must stop: it timed out, or it is
    /// automatic and the connection has no live session.
    fn must_stop(&self) -> bool {
        self.expired() || (self.automatic && !self.warm)
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
        let settings = self.profile.catalog.clone();
        self.update(|catalog| catalog.apply_schemas(names, &settings, now()));
        // The schemas that Qrow read longest ago go first. A refresh that the
        // timeout stops then does not leave the same schemas old each time.
        let mut schemas: Vec<(Option<u64>, String)> = self
            .catalog
            .schemas
            .iter()
            .map(|(name, schema)| (schema.fetched_at, name.clone()))
            .collect();
        schemas.sort();
        let schemas: Vec<String> = schemas.into_iter().map(|(_, name)| name).collect();
        self.status.total = schemas.len();
        self.publish(true);
        // Each schema is one step: its relations, then their columns. No
        // request reads the columns of the whole connection at once.
        for schema in schemas {
            self.checkpoint()?;
            if self.catalog.schema(&schema).is_none() {
                self.status.total = self.status.total.saturating_sub(1);
                continue;
            }
            self.refresh_schema(&schema)?;
            self.status.done += 1;
            self.publish(false);
        }
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
        self.update(|catalog| catalog.set_error(scope, message));
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
                        .is_some_and(|session| session.close_operation().is_ok());
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
        if self.session.is_none() {
            let started = Instant::now();
            let password = (self.passwords)(&self.profile)?;
            self.session = Some(self.connector.connect(&self.profile, password)?);
            let duration = started.elapsed();
            self.log(
                Severity::Info,
                format!(
                    "Opened a session for the schema refresh (client measurement: {})",
                    format_duration(duration)
                ),
                Some(duration),
            );
        }
        let session = self.session.as_mut().unwrap();
        let cancellation = session.execute_metadata(request)?;
        *self.target.lock().unwrap() = Some(cancellation.clone());
        if self.cancelled.load(Ordering::SeqCst) {
            cancellation.cancel()?;
        }
        let has_results = loop {
            match self.session.as_mut().unwrap().poll()? {
                QueryState::Finished { has_results } => break has_results,
                QueryState::Cancelled => anyhow::bail!("The catalog request was cancelled"),
                QueryState::Running => {}
            }
            // Do not wait for the server to confirm a cancellation. The next
            // request closes the operation.
            anyhow::ensure!(
                !self.cancelled.load(Ordering::SeqCst),
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
            self.session.as_mut().unwrap().close_operation()?;
            return Ok(MetadataRows {
                columns: vec![],
                rows: vec![],
            });
        }
        let columns = self.session.as_mut().unwrap().columns()?;
        let mut rows: Vec<Row> = Vec::new();
        let mut bytes = 0usize;
        loop {
            let batch = self.session.as_mut().unwrap().fetch(METADATA_BATCH)?;
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
            if self.cancelled.load(Ordering::SeqCst) {
                anyhow::bail!("The catalog request was cancelled");
            }
            self.accept_refreshes();
            if self.must_stop() {
                Self::cancel_request(&cancellation);
                anyhow::bail!("The catalog request was stopped");
            }
        }
        self.session.as_mut().unwrap().close_operation()?;
        Ok(MetadataRows { columns, rows })
    }

    /// Cancel a request through the separate cancel transport, off this thread.
    fn cancel_request(cancellation: &Arc<dyn Cancellation>) {
        let cancellation = cancellation.clone();
        thread::spawn(move || {
            let _ = cancellation.cancel();
        });
    }

    /// Queue the refreshes that arrived while a request runs, so the UI shows
    /// them as waiting, and apply the live-session state. Keep the other
    /// commands for the next checkpoint.
    fn accept_refreshes(&mut self) {
        while let Ok(command) = self.rx.try_recv() {
            match command {
                Command::Refresh(_) | Command::RefreshIfUnloaded | Command::SetWarm(_) => {
                    self.handle(command);
                }
                command => self.deferred.push_back(command),
            }
        }
    }

    /// Send a Logs entry of the refresh in progress: an error, or any entry
    /// if the profile enables refresh logs.
    fn log(&self, severity: Severity, text: String, duration: Option<Duration>) {
        // Errors always go to Logs, because the tree shows only a summary.
        if !self.profile.catalog.log_refreshes && severity != Severity::Error {
            return;
        }
        let mut event = ActivityEvent::new(None, severity, ActivityKind::SchemaRefresh, text)
            .with_connection(self.profile.name.clone())
            .with_batch(self.batch);
        event.duration = duration;
        let _ = self.activity.send(event);
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
        if let Some(mut session) = self.session.take() {
            let _ = session.close();
        }
    }
}
