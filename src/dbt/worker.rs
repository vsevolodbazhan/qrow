//! A thread that keeps the index of each dbt manifest that a connection
//! uses.
//!
//! Connections with the same manifest share one index. The worker saves each
//! index in the data folder and loads it at launch. It parses a manifest
//! when the saved index is missing or does not agree with the file, on
//! Refresh, and, for automatic refresh, when the file changes. It parses one
//! manifest at a time. A failed parse keeps the previous index.
//!
//! Automatic refresh watches the folder of the manifest with FSEvents,
//! because dbt can replace the file. dbt does not write the file in one
//! step, so the worker waits until the file has not changed for a short
//! time before it parses.

use super::{
    Index, Kind, ParseError, parse,
    saved::{self, Saved, Stamp},
};
use crate::logs::Severity;
use notify::{RecursiveMode, Watcher};
use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant, SystemTime},
};

/// How long a manifest must stay unchanged before an automatic refresh.
pub const SETTLE: Duration = Duration::from_secs(2);

/// The key of a manifest: its path without `.` parts. Connections with the
/// same key share one index. The worker reads the manifest through this
/// path, so a symbolic link can change its target. A `..` part stays,
/// because after a symbolic link it names the parent of the link target.
pub fn manifest_key(path: &Path) -> PathBuf {
    path.components()
        .filter(|component| *component != std::path::Component::CurDir)
        .collect()
}

/// The canonical path of the folder of a manifest, and its file name.
fn link_path(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => canonical_path(parent).join(name),
        _ => path.to_owned(),
    }
}

/// The canonical path of a manifest, which FSEvents reports. For a manifest
/// that does not exist yet, the canonical path of the closest folder above
/// it that exists, and the rest of the path.
fn canonical_path(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if let Ok(canonical) = fs::canonicalize(ancestor) {
            return match path.strip_prefix(ancestor) {
                Ok(rest) if !rest.as_os_str().is_empty() => canonical.join(rest),
                _ => canonical,
            };
        }
    }
    path.to_owned()
}

/// The file of the saved index of the manifest `key` in `directory`.
pub fn saved_path(directory: &Path, key: &Path) -> PathBuf {
    let digest = ring::digest::digest(&ring::digest::SHA256, key.as_os_str().as_encoded_bytes());
    let name: String = digest.as_ref()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    directory.join(format!("{name}.index"))
}

/// Why a manifest has no current index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifestError {
    NotFound,
    UnsupportedVersion(String),
    Invalid(String),
    Unreadable(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => {
                formatter.write_str("Manifest not found: run dbt parse in the project")
            }
            Self::UnsupportedVersion(version) => {
                ParseError::UnsupportedVersion(version.clone()).fmt(formatter)
            }
            Self::Invalid(message) => ParseError::Invalid(message.clone()).fmt(formatter),
            Self::Unreadable(message) => write!(formatter, "Cannot read the manifest: {message}"),
        }
    }
}

impl From<ParseError> for ManifestError {
    fn from(error: ParseError) -> Self {
        match error {
            ParseError::UnsupportedVersion(version) => Self::UnsupportedVersion(version),
            ParseError::Invalid(message) => Self::Invalid(message),
        }
    }
}

/// The state of one manifest.
#[derive(Clone, Debug, Default)]
pub struct ManifestState {
    /// The key of the manifest.
    pub path: PathBuf,
    /// The last index that Qrow made, also when a later parse failed.
    pub index: Option<Arc<Index>>,
    /// The stamp of the manifest that the index comes from.
    pub stamp: Option<Stamp>,
    /// When Qrow made the index.
    pub refreshed: Option<SystemTime>,
    pub parsing: bool,
    /// The error of the last refresh, or a manifest that is gone.
    pub error: Option<ManifestError>,
}

impl ManifestState {
    /// Whether the spans of the index agree with the manifest file now.
    pub fn is_current(&self) -> bool {
        self.stamp.is_some() && Stamp::of(&self.path).ok() == self.stamp
    }
}

/// A manifest that a connection uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Use {
    pub manifest: PathBuf,
    /// Whether one of the connections refreshes it when it changes.
    pub automatic: bool,
}

pub enum Event {
    State(Arc<ManifestState>),
    /// A finished refresh, for the Activity of the connections that use the
    /// manifest.
    Log {
        manifest: PathBuf,
        severity: Severity,
        text: String,
    },
}

enum Command {
    Configure(Vec<Use>),
    Refresh(PathBuf),
    Changed(Vec<PathBuf>),
    Shutdown,
}

pub struct DbtWorker {
    tx: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
}

impl DbtWorker {
    /// `directory` keeps the saved indexes, or `None` to keep them only in
    /// memory. `wake` tells the window that events wait.
    pub fn new(directory: Option<PathBuf>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::with_settle(directory, wake, SETTLE)
    }

    /// `settle` is the time without change before an automatic refresh.
    /// Tests make it shorter.
    pub fn with_settle(
        directory: Option<PathBuf>,
        wake: Arc<dyn Fn() + Send + Sync>,
        settle: Duration,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let changes = tx.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                let _ = changes.send(Command::Changed(event.paths));
            }
        })
        .ok();
        let runner = Runner {
            directory,
            manifests: HashMap::new(),
            watcher,
            watched: HashSet::new(),
            due: HashMap::new(),
            settle,
            rx,
            tx: events_tx,
            wake,
        };
        thread::spawn(move || runner.run());
        Self { tx, events }
    }

    /// Keep the indexes of `uses` and forget the others. A saved index that
    /// no connection uses is deleted.
    pub fn configure(&self, uses: Vec<Use>) {
        let _ = self.tx.send(Command::Configure(uses));
    }

    /// Parse the manifest `path` now.
    pub fn refresh(&self, path: &Path) {
        let _ = self.tx.send(Command::Refresh(manifest_key(path)));
    }

    /// A handle that asks the worker to read manifests again, for a view
    /// that cannot reach the worker.
    pub fn refresher(&self) -> Refresher {
        Refresher(self.tx.clone())
    }
}

/// Asks a [`DbtWorker`] to read a manifest again.
#[derive(Clone)]
pub struct Refresher(mpsc::Sender<Command>);

impl Refresher {
    pub fn refresh(&self, path: &Path) {
        let _ = self.0.send(Command::Refresh(manifest_key(path)));
    }
}

impl Drop for DbtWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

struct Slot {
    state: Arc<ManifestState>,
    automatic: bool,
    /// The canonical path of the manifest, for the paths of FSEvents.
    canonical: PathBuf,
    /// The canonical path of the manifest without its last link resolved:
    /// a change of a symbolic link happens there.
    link: PathBuf,
}

/// When a manifest is due, and whether a Refresh asked for it.
#[derive(Clone, Copy)]
struct Due {
    at: Instant,
    forced: bool,
}

struct Runner {
    directory: Option<PathBuf>,
    manifests: HashMap<PathBuf, Slot>,
    watcher: Option<notify::RecommendedWatcher>,
    watched: HashSet<PathBuf>,
    due: HashMap<PathBuf, Due>,
    settle: Duration,
    rx: mpsc::Receiver<Command>,
    tx: mpsc::Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Runner {
    fn run(mut self) {
        loop {
            let next = self.due.values().map(|due| due.at).min();
            let command = match next {
                Some(at) => match self
                    .rx
                    .recv_timeout(at.saturating_duration_since(Instant::now()))
                {
                    Ok(command) => Some(command),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                },
                None => match self.rx.recv() {
                    Ok(command) => Some(command),
                    Err(_) => return,
                },
            };
            match command {
                Some(Command::Configure(uses)) => self.configure(uses),
                Some(Command::Refresh(key)) => {
                    if self.manifests.contains_key(&key) {
                        self.due.insert(
                            key,
                            Due {
                                at: Instant::now(),
                                forced: true,
                            },
                        );
                    }
                }
                Some(Command::Changed(paths)) => self.changed(paths),
                Some(Command::Shutdown) => return,
                None => self.refresh_due(),
            }
        }
    }

    fn configure(&mut self, uses: Vec<Use>) {
        let mut wanted: HashMap<PathBuf, bool> = HashMap::new();
        for item in uses {
            *wanted.entry(manifest_key(&item.manifest)).or_default() |= item.automatic;
        }
        self.manifests.retain(|key, _| wanted.contains_key(key));
        self.due.retain(|key, _| wanted.contains_key(key));
        for (key, automatic) in wanted {
            match self.manifests.get_mut(&key) {
                Some(slot) => {
                    // A manifest that becomes automatic catches up with
                    // changes that happened while it was manual. A manifest
                    // that becomes manual keeps only a requested refresh.
                    if automatic && !slot.automatic && !slot.state.is_current() {
                        self.due.insert(
                            key.clone(),
                            Due {
                                at: Instant::now(),
                                forced: false,
                            },
                        );
                    } else if !automatic && self.due.get(&key).is_some_and(|due| !due.forced) {
                        self.due.remove(&key);
                    }
                    slot.automatic = automatic;
                }
                None => self.open(key, automatic),
            }
        }
        self.sync_watches();
        self.delete_unused();
    }

    /// Load the saved index of a new manifest. Parse when it is missing or
    /// does not agree with the file.
    fn open(&mut self, key: PathBuf, automatic: bool) {
        let mut state = ManifestState {
            path: key.clone(),
            ..ManifestState::default()
        };
        if let Some(saved) = self.load(&key) {
            state.index = Some(Arc::new(saved.index));
            state.stamp = Some(saved.stamp);
            state.refreshed = Some(saved.refreshed);
        }
        // A launch parses also a manual manifest.
        if !state.is_current() {
            self.due.insert(
                key.clone(),
                Due {
                    at: Instant::now(),
                    forced: true,
                },
            );
        }
        let state = Arc::new(state);
        self.manifests.insert(
            key.clone(),
            Slot {
                state: state.clone(),
                automatic,
                canonical: canonical_path(&key),
                link: link_path(&key),
            },
        );
        self.publish(state);
    }

    fn load(&self, key: &Path) -> Option<Saved> {
        let path = saved_path(self.directory.as_deref()?, key);
        let saved = saved::decode(&fs::read(path).ok()?).ok()?;
        (saved.manifest == key).then_some(saved)
    }

    /// Watch the folder of each automatic manifest, without subfolders. A
    /// folder that does not exist yet, like `target` before the first dbt
    /// command, is watched through the closest folder above it that exists.
    fn sync_watches(&mut self) {
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        for (key, slot) in &mut self.manifests {
            slot.canonical = canonical_path(key);
            slot.link = link_path(key);
        }
        // The folder of the file, and of a symbolic link to it.
        let wanted: HashSet<PathBuf> = self
            .manifests
            .values()
            .filter(|slot| slot.automatic)
            .flat_map(|slot| [&slot.canonical, &slot.link])
            .filter_map(|path| path.ancestors().skip(1).find(|folder| folder.is_dir()))
            .map(Path::to_owned)
            .collect();
        for folder in self.watched.difference(&wanted) {
            let _ = watcher.unwatch(folder);
        }
        let mut watched = HashSet::new();
        for folder in wanted {
            if self.watched.contains(&folder)
                || watcher.watch(&folder, RecursiveMode::NonRecursive).is_ok()
            {
                watched.insert(folder);
            }
        }
        self.watched = watched;
    }

    /// Delete the saved indexes of manifests that no connection uses.
    fn delete_unused(&self) {
        let Some(directory) = &self.directory else {
            return;
        };
        let Ok(files) = fs::read_dir(directory) else {
            return;
        };
        let keep: HashSet<PathBuf> = self
            .manifests
            .keys()
            .map(|key| saved_path(directory, key))
            .collect();
        for file in files.flatten() {
            let path = file.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "index")
                && !keep.contains(&path)
            {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// A change in a watched folder. The manifest is due after it settles.
    fn changed(&mut self, paths: Vec<PathBuf>) {
        let at = Instant::now() + self.settle;
        let mut touched_any = false;
        for (key, slot) in &self.manifests {
            if !slot.automatic {
                continue;
            }
            // A change of a folder above the manifest, like a new `target`
            // folder, or of a link to it, can change the manifest too.
            let touched = paths.iter().any(|path| {
                slot.canonical.starts_with(path)
                    || slot.link.starts_with(path)
                    || key.starts_with(path)
            });
            if touched {
                touched_any = true;
                let forced = self.due.get(key).is_some_and(|due| due.forced);
                self.due.insert(key.clone(), Due { at, forced });
            }
        }
        // A new folder or a new link target needs other watches.
        if touched_any {
            self.sync_watches();
        }
    }

    fn refresh_due(&mut self) {
        let now = Instant::now();
        let ready: Vec<(PathBuf, bool)> = self
            .due
            .iter()
            .filter(|(_, due)| due.at <= now)
            .map(|(key, due)| (key.clone(), due.forced))
            .collect();
        for (key, forced) in ready {
            self.due.remove(&key);
            self.refresh(key, forced);
        }
    }

    /// Parse the manifest `key` if it changed since its index, or if a
    /// Refresh asked for it.
    fn refresh(&mut self, key: PathBuf, forced: bool) {
        let Some(slot) = self.manifests.get(&key) else {
            return;
        };
        let previous = slot.state.clone();
        let stamp = match Stamp::of(&key) {
            Ok(stamp) => stamp,
            Err(error) => {
                let error = if error.kind() == std::io::ErrorKind::NotFound {
                    ManifestError::NotFound
                } else {
                    ManifestError::Unreadable(error.to_string())
                };
                // A manifest that stays gone reports once.
                if previous.error.as_ref() != Some(&error) || forced {
                    self.finish(&key, Err(error), Duration::ZERO, None);
                }
                return;
            }
        };
        if !forced && previous.stamp == Some(stamp) && previous.error.is_none() {
            return;
        }
        self.update(&key, |state| state.parsing = true);
        let started = Instant::now();
        let bytes = match fs::read(&key) {
            Ok(bytes) => bytes,
            Err(error) => {
                let error = ManifestError::Unreadable(error.to_string());
                self.finish(&key, Err(error), started.elapsed(), None);
                return;
            }
        };
        // dbt still writes the file: try again when it settles.
        if Stamp::of(&key).ok() != Some(stamp) || bytes.len() as u64 != stamp.len {
            self.update(&key, |state| state.parsing = false);
            self.due.insert(
                key,
                Due {
                    at: Instant::now() + self.settle,
                    forced,
                },
            );
            return;
        }
        let result = parse(&bytes).map_err(ManifestError::from);
        drop(bytes);
        self.finish(&key, result, started.elapsed(), Some(stamp));
    }

    /// Keep the result of a refresh, save a new index, and report it.
    fn finish(
        &mut self,
        key: &Path,
        result: Result<Index, ManifestError>,
        duration: Duration,
        stamp: Option<Stamp>,
    ) {
        let milliseconds = duration.as_millis();
        let (severity, text) = match result {
            Ok(index) => {
                let text = format!(
                    "dbt manifest refreshed in {milliseconds} ms: {}",
                    describe(&index)
                );
                let refreshed = SystemTime::now();
                let saved = Saved {
                    manifest: key.to_owned(),
                    stamp: stamp.unwrap_or(Stamp {
                        len: 0,
                        modified: None,
                    }),
                    refreshed,
                    index,
                };
                let save_error = self.save(&saved).err();
                let index = Arc::new(saved.index);
                self.update(key, |state| {
                    state.index = Some(index);
                    state.stamp = stamp;
                    state.refreshed = Some(refreshed);
                    state.parsing = false;
                    state.error = None;
                });
                if let Some(error) = save_error {
                    self.log(
                        key,
                        Severity::Error,
                        format!("Could not save the dbt index: {error}"),
                    );
                }
                (Severity::Info, text)
            }
            Err(error) => {
                let text = match &error {
                    ManifestError::NotFound => error.to_string(),
                    _ => format!("dbt manifest refresh failed after {milliseconds} ms: {error}"),
                };
                self.update(key, |state| {
                    state.parsing = false;
                    state.error = Some(error);
                });
                (Severity::Error, text)
            }
        };
        self.log(key, severity, text);
    }

    fn save(&self, saved: &Saved) -> anyhow::Result<()> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        fs::create_dir_all(directory)?;
        crate::storage::write_atomically(
            &saved_path(directory, &saved.manifest),
            &saved::encode(saved),
        )
    }

    fn update(&mut self, key: &Path, change: impl FnOnce(&mut ManifestState)) {
        let Some(slot) = self.manifests.get_mut(key) else {
            return;
        };
        let mut state = (*slot.state).clone();
        change(&mut state);
        slot.state = Arc::new(state);
        let state = slot.state.clone();
        self.publish(state);
    }

    fn publish(&self, state: Arc<ManifestState>) {
        let _ = self.tx.send(Event::State(state));
        (self.wake)();
    }

    fn log(&self, key: &Path, severity: Severity, text: String) {
        let _ = self.tx.send(Event::Log {
            manifest: key.to_owned(),
            severity,
            text,
        });
        (self.wake)();
    }
}

/// The counts of an index, like "1,884 models, 287 sources, 7,966 tests".
pub fn describe(index: &Index) -> String {
    let count = |kind| {
        index
            .entries()
            .iter()
            .filter(|entry| entry.kind == kind)
            .count()
    };
    let mut parts = vec![plural(count(Kind::Model), "model")];
    for (kind, name) in [
        (Kind::Seed, "seed"),
        (Kind::Snapshot, "snapshot"),
        (Kind::Source, "source"),
    ] {
        let count = count(kind);
        if count > 0 {
            parts.push(plural(count, name));
        }
    }
    parts.push(plural(index.test_count(), "test"));
    parts.join(", ")
}

/// `count` and `name`, with digits in groups of three and a plural `s`.
pub fn plural(count: usize, name: &str) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{grouped} {name}{}", if count == 1 { "" } else { "s" })
}
