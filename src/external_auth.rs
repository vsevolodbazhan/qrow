//! Shared, in-memory browser authentication. Database connectors implement
//! their own challenge protocol; this service coordinates their callers.
use crate::{
    connector::Secret,
    model::{Authentication, DatabaseType, Profile},
};
use anyhow::{Result, anyhow};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;
use zeroize::Zeroizing;

pub type Browser = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;
/// Cancellation remains available before a database has returned a cursor.
#[derive(Clone, Default)]
pub struct Control {
    cancelled: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    progress: Option<Arc<dyn Fn(bool) + Send + Sync>>,
}
impl Control {
    pub fn new(
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
        progress: Arc<dyn Fn(bool) + Send + Sync>,
    ) -> Self {
        Self {
            cancelled: Some(cancelled),
            progress: Some(progress),
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.as_ref().is_some_and(|check| check())
    }
    fn progress(&self, waiting: bool) {
        if let Some(notify) = &self.progress {
            notify(waiting);
        }
    }
}
/// A protocol diagnostic that contains no server-provided secrets.
#[derive(Debug)]
pub struct Failure(pub &'static str);
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Failure {}
#[derive(Debug)]
pub struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sign-in cancelled")
    }
}
impl std::error::Error for Cancelled {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    SignedOut,
    Waiting,
    SignedIn,
    Failed,
}

type Token = Arc<Zeroizing<String>>;
struct Flight {
    cancel: Arc<AtomicBool>,
    waiters: AtomicUsize,
    result: Mutex<Option<std::result::Result<Token, String>>>,
}
#[derive(Default)]
struct State {
    token: Option<Token>,
    flight: Option<Arc<Flight>>,
    failed: bool,
}
struct Entry {
    profile: Profile,
    valid: AtomicBool,
    state: Mutex<State>,
}
impl Entry {
    fn invalidate(&self) {
        self.valid.store(false, Ordering::SeqCst);
        let mut state = self.state.lock().unwrap();
        state.token = None;
        if let Some(flight) = state.flight.take() {
            flight.cancel.store(true, Ordering::SeqCst);
        }
    }
}
#[derive(Default)]
struct Registry {
    entries: Vec<Arc<Entry>>,
    profiles: Vec<Profile>,
}
#[derive(Default)]
struct Shared {
    registry: Mutex<Registry>,
    notify: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
impl Shared {
    fn changed(&self) {
        let notify = self.notify.lock().unwrap().clone();
        if let Some(notify) = notify {
            notify();
        }
    }
}

pub struct Service {
    shared: Arc<Shared>,
    browser: Browser,
    timeout: Duration,
}
impl Service {
    pub fn new(browser: Option<Browser>) -> Self {
        Self::with_timeout(browser, Duration::from_secs(120))
    }
    pub fn with_timeout(browser: Option<Browser>, timeout: Duration) -> Self {
        Self {
            shared: Arc::default(),
            browser: browser.unwrap_or_else(|| Arc::new(open_browser)),
            timeout,
        }
    }
    pub fn set_on_change(&self, notify: Arc<dyn Fn() + Send + Sync>) {
        *self.shared.notify.lock().unwrap() = Some(notify);
    }
    /// Invalidate tokens and running work when connection settings change.
    pub fn configure(&self, profiles: &[Profile]) {
        let mut registry = self.shared.registry.lock().unwrap();
        registry.profiles = profiles.to_vec();
        registry.entries.retain(|entry| {
            let keep = profiles.iter().any(|profile| {
                profile.authentication == Authentication::TrinoExternal
                    && entry.profile.connection_identity_eq(profile)
            });
            if !keep {
                entry.invalidate();
            }
            keep
        });
    }
    pub fn secret(&self, profile: &Profile) -> Result<Secret> {
        anyhow::ensure!(
            profile.authentication == Authentication::TrinoExternal,
            "This connection does not use Trino external authentication"
        );
        profile.validate()?;
        anyhow::ensure!(
            profile.database_type == DatabaseType::Trino && profile.tls,
            "Trino external authentication requires HTTPS"
        );
        let mut registry = self.shared.registry.lock().unwrap();
        anyhow::ensure!(
            registry
                .profiles
                .iter()
                .any(|current| current.connection_identity_eq(profile)),
            "Connection settings changed; reconnect"
        );
        let entry = match registry
            .entries
            .iter()
            .find(|entry| entry.profile.connection_identity_eq(profile))
        {
            Some(entry) => entry.clone(),
            None => {
                let entry = Arc::new(Entry {
                    profile: profile.clone(),
                    valid: AtomicBool::new(true),
                    state: Mutex::default(),
                });
                registry.entries.push(entry.clone());
                entry
            }
        };
        Ok(Secret::External(Source {
            entry,
            shared: self.shared.clone(),
            browser: self.browser.clone(),
            control: Control::default(),
            timeout: self.timeout,
        }))
    }
    /// Check only memory; selecting a connection with a cached token needs no probe.
    #[cfg(any(feature = "ui", test))]
    pub(crate) fn is_authenticated(&self, profile: &Profile) -> bool {
        self.shared
            .registry
            .lock()
            .unwrap()
            .entries
            .iter()
            .any(|entry| {
                entry.profile.connection_identity_eq(profile)
                    && entry.valid.load(Ordering::SeqCst)
                    && entry.state.lock().unwrap().token.is_some()
            })
    }
    /// Failures belong to one connection, even if another has a cached token.
    #[cfg(test)]
    pub(crate) fn failed(&self, profile: &Profile) -> bool {
        self.shared
            .registry
            .lock()
            .unwrap()
            .entries
            .iter()
            .any(|entry| {
                entry.profile.connection_identity_eq(profile)
                    && entry.valid.load(Ordering::SeqCst)
                    && entry.state.lock().unwrap().failed
            })
    }
    pub fn status(&self, id: Uuid) -> Status {
        let registry = self.shared.registry.lock().unwrap();
        let entries = &registry.entries;
        let mut status = Status::SignedOut;
        for entry in entries.iter().filter(|entry| entry.profile.id == id) {
            let state = entry.state.lock().unwrap();
            if state.flight.is_some() {
                return Status::Waiting;
            }
            if state.token.is_some() {
                status = Status::SignedIn;
            } else if state.failed && status != Status::SignedIn {
                status = Status::Failed;
            }
        }
        status
    }
    /// Local sign-out also cancels pending authentication. It does not contact
    /// the identity provider or end the browser's session.
    pub fn clear(&self, id: Uuid) {
        self.shared
            .registry
            .lock()
            .unwrap()
            .entries
            .retain(|entry| {
                if entry.profile.id == id {
                    entry.invalidate();
                    false
                } else {
                    true
                }
            });
        self.shared.changed();
    }
}

#[derive(Clone)]
pub struct Source {
    entry: Arc<Entry>,
    shared: Arc<Shared>,
    browser: Browser,
    control: Control,
    timeout: Duration,
}
impl Source {
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
    pub fn with_control(mut self, control: Control) -> Self {
        self.control = control;
        self
    }
    pub fn cached(&self) -> Result<Option<Zeroizing<String>>> {
        anyhow::ensure!(
            self.entry.valid.load(Ordering::SeqCst),
            "Connection settings changed; reconnect"
        );
        Ok(self
            .entry
            .state
            .lock()
            .unwrap()
            .token
            .as_ref()
            .map(|token| Zeroizing::new(token.as_str().to_owned())))
    }
    pub fn reject(&self, token: Option<&str>) {
        let mut state = self.entry.state.lock().unwrap();
        if state
            .token
            .as_ref()
            .is_some_and(|cached| Some(cached.as_str()) == token)
        {
            state.token = None;
            state.failed = true;
        }
        drop(state);
        self.shared.changed();
    }
    pub fn control(&self) -> &Control {
        &self.control
    }
    pub fn browser(&self) -> Browser {
        self.browser.clone()
    }
    /// Start one authentication job and share it with independently cancellable
    /// callers. Cancelling one query does not cancel another query's login.
    pub fn authenticate<F>(
        &self,
        rejected: Option<&str>,
        timeout: Duration,
        job: F,
    ) -> Result<Zeroizing<String>>
    where
        F: FnOnce(Arc<AtomicBool>) -> Result<Zeroizing<String>> + Send + 'static,
    {
        if self.control.is_cancelled() {
            return Err(Cancelled.into());
        }
        anyhow::ensure!(
            self.entry.valid.load(Ordering::SeqCst),
            "Connection settings changed; reconnect"
        );
        let (flight, start) = {
            let mut state = self.entry.state.lock().unwrap();
            if let Some(token) = &state.token
                && Some(token.as_str()) != rejected
            {
                return Ok(Zeroizing::new(token.as_str().to_owned()));
            }
            state.token = None;
            state.failed = false;
            let existing = state
                .flight
                .as_ref()
                .filter(|flight| !flight.cancel.load(Ordering::SeqCst))
                .cloned();
            let start = existing.is_none();
            let flight = existing.unwrap_or_else(|| {
                Arc::new(Flight {
                    cancel: Arc::new(AtomicBool::new(false)),
                    waiters: AtomicUsize::new(0),
                    result: Mutex::default(),
                })
            });
            flight.waiters.fetch_add(1, Ordering::SeqCst);
            state.flight = Some(flight.clone());
            (flight, start)
        };
        let _waiter = Waiter {
            flight: flight.clone(),
            entry: self.entry.clone(),
            control: self.control.clone(),
            shared: self.shared.clone(),
        };
        self.control.progress(true);
        if start {
            self.shared.changed();
            let entry = self.entry.clone();
            let shared = self.shared.clone();
            let running = flight.clone();
            thread::spawn(move || {
                let result = job(running.cancel.clone()).map(Arc::new).map_err(|error| {
                    error.downcast_ref::<Failure>().map_or_else(
                        || "Trino browser sign-in failed. Try again.".to_owned(),
                        ToString::to_string,
                    )
                });
                {
                    let mut state = entry.state.lock().unwrap();
                    // A sign-out, setting change, or last waiter cancellation
                    // must not allow a late result to restore authentication.
                    if entry.valid.load(Ordering::SeqCst)
                        && !running.cancel.load(Ordering::SeqCst)
                        && state
                            .flight
                            .as_ref()
                            .is_some_and(|flight| Arc::ptr_eq(flight, &running))
                    {
                        state.token = result.as_ref().ok().cloned();
                        state.failed = result.is_err();
                        state.flight = None;
                    } else if state
                        .flight
                        .as_ref()
                        .is_some_and(|flight| Arc::ptr_eq(flight, &running))
                    {
                        state.flight = None;
                    }
                    *running.result.lock().unwrap() = Some(result);
                }
                shared.changed();
            });
        }
        let deadline = Instant::now() + timeout;
        loop {
            if self.control.is_cancelled() || !self.entry.valid.load(Ordering::SeqCst) {
                return Err(Cancelled.into());
            }
            if let Some(result) = flight.result.lock().unwrap().as_ref() {
                return result
                    .as_ref()
                    .map(|token| Zeroizing::new(token.as_str().to_owned()))
                    .map_err(|message| anyhow!(message.clone()));
            }
            anyhow::ensure!(Instant::now() < deadline, "Trino browser sign-in timed out");
            thread::sleep(Duration::from_millis(25));
        }
    }
}
struct Waiter {
    flight: Arc<Flight>,
    entry: Arc<Entry>,
    control: Control,
    shared: Arc<Shared>,
}
impl Drop for Waiter {
    fn drop(&mut self) {
        let _state = self.entry.state.lock().unwrap();
        if self.flight.waiters.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.flight.cancel.store(true, Ordering::SeqCst);
        }
        drop(_state);
        self.control.progress(false);
        self.shared.changed();
    }
}
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let command = "open";
    #[cfg(not(target_os = "macos"))]
    let command = "xdg-open";
    let status = std::process::Command::new(command)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| anyhow!("Cannot open the browser"))?;
    anyhow::ensure!(status.success(), "Cannot open the browser");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    fn setup() -> (Arc<Service>, Profile, Source) {
        let profile = Profile {
            name: "Trino".into(),
            host: "localhost".into(),
            username: "alice".into(),
            database_type: DatabaseType::Trino,
            tls: true,
            authentication: Authentication::TrinoExternal,
            ..Profile::default()
        };
        let service = Arc::new(Service::new(Some(Arc::new(|_| Ok(())))));
        service.configure(std::slice::from_ref(&profile));
        let Secret::External(source) = service.secret(&profile).unwrap() else {
            panic!()
        };
        (service, profile, source)
    }
    #[test]
    fn connection_selection_reuses_only_its_own_cached_token() {
        let (service, profile, source) = setup();
        assert!(!service.is_authenticated(&profile));
        source
            .authenticate(None, Duration::from_secs(1), |_| {
                Ok(Zeroizing::new("opaque".into()))
            })
            .unwrap();
        assert!(service.is_authenticated(&profile));
        let mut different = profile.clone();
        different.username = "bob".into();
        assert!(!service.is_authenticated(&different));
        different = profile.clone();
        different.id = Uuid::new_v4();
        assert!(!service.is_authenticated(&different));
        different = profile.clone();
        different.port += 1;
        assert!(!service.is_authenticated(&different));
        service.clear(profile.id);
        assert!(!service.is_authenticated(&profile));
    }
    #[test]
    fn a_failed_connection_is_not_hidden_by_another_cached_token() {
        let (service, profile, source) = setup();
        source
            .authenticate(None, Duration::from_secs(1), |_| {
                Ok(Zeroizing::new("opaque".into()))
            })
            .unwrap();
        let mut other = profile.clone();
        other.id = Uuid::new_v4();
        service.configure(&[profile.clone(), other.clone()]);
        let Secret::External(other_source) = service.secret(&other).unwrap() else {
            panic!()
        };
        assert!(
            other_source
                .authenticate(None, Duration::from_secs(1), |_| Err(Failure(
                    "Synthetic failure"
                )
                .into()))
                .is_err()
        );
        assert_eq!(service.status(profile.id), Status::SignedIn);
        assert!(!service.failed(&profile));
        assert!(service.failed(&other));
    }
    #[test]
    fn concurrent_callers_share_one_flight_and_one_cancel_does_not_stop_other_waiters() {
        let (service, profile, source) = setup();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let leader = source.clone().with_control(Control::new(
            Arc::new(move || cancellation.load(Ordering::SeqCst)),
            Arc::new(|_| {}),
        ));
        let gate = Arc::new(Barrier::new(2));
        let job_gate = gate.clone();
        let release = Arc::new(AtomicBool::new(false));
        let job_release = release.clone();
        let first = thread::spawn(move || {
            leader.authenticate(None, Duration::from_secs(3), move |cancel| {
                job_gate.wait();
                while !cancel.load(Ordering::SeqCst) && !job_release.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
                check(&cancel)?;
                Ok(Zeroizing::new("opaque".into()))
            })
        });
        gate.wait();
        let follower = source.clone();
        let second = thread::spawn(move || {
            follower.authenticate(None, Duration::from_secs(3), |_| {
                panic!("duplicate authentication")
            })
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if source
                .entry
                .state
                .lock()
                .unwrap()
                .flight
                .as_ref()
                .unwrap()
                .waiters
                .load(Ordering::SeqCst)
                == 2
            {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        cancelled.store(true, Ordering::SeqCst);
        assert!(first.join().unwrap().unwrap_err().is::<Cancelled>());
        release.store(true, Ordering::SeqCst);
        assert_eq!(second.join().unwrap().unwrap().as_str(), "opaque");
        assert_eq!(service.status(profile.id), Status::SignedIn);
    }
    fn check(cancel: &AtomicBool) -> Result<()> {
        if cancel.load(Ordering::SeqCst) {
            Err(Cancelled.into())
        } else {
            Ok(())
        }
    }
    #[test]
    fn cache_isolated_by_connection_endpoint_username_and_authentication() {
        let (service, profile, source) = setup();
        source
            .authenticate(None, Duration::from_secs(1), |_| {
                Ok(Zeroizing::new("first".into()))
            })
            .unwrap();
        assert_eq!(
            source
                .authenticate(None, Duration::from_secs(1), |_| panic!("cached"))
                .unwrap()
                .as_str(),
            "first"
        );
        {
            let mut other = profile.clone();
            // Check each scope independently while all profiles remain configured.
            other.id = Uuid::new_v4();
            service.configure(&[profile.clone(), other.clone()]);
            let Secret::External(other_source) = service.secret(&other).unwrap() else {
                panic!()
            };
            assert!(other_source.cached().unwrap().is_none());
        }
        for (username, port) in [
            ("other-user", profile.port),
            (profile.username.as_str(), profile.port + 1),
        ] {
            let other = Profile {
                username: username.into(),
                port,
                ..profile.clone()
            };
            service.configure(&[profile.clone(), other.clone()]);
            let Secret::External(other_source) = service.secret(&other).unwrap() else {
                panic!()
            };
            assert!(other_source.cached().unwrap().is_none());
        }
        let changed = Profile {
            authentication: Authentication::Password,
            ..profile
        };
        service.configure(std::slice::from_ref(&changed));
        assert!(source.cached().is_err());
        assert!(service.secret(&changed).is_err());
    }
    #[test]
    fn stale_connection_snapshots_cannot_recreate_authentication() {
        let (service, profile, source) = setup();
        let changed = Profile {
            username: "new-user".into(),
            ..profile.clone()
        };
        service.configure(std::slice::from_ref(&changed));
        assert!(source.cached().is_err());
        assert!(service.secret(&profile).is_err());
        assert!(service.secret(&changed).is_ok());
        service.configure(&[]);
        assert!(service.secret(&changed).is_err());
    }
    #[test]
    fn sign_out_discards_late_results_and_cancels_waiters() {
        let (service, profile, source) = setup();
        let gate = Arc::new(Barrier::new(2));
        let started = gate.clone();
        let waiter = thread::spawn(move || {
            source.authenticate(None, Duration::from_secs(2), move |cancel| {
                started.wait();
                while !cancel.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
                Ok(Zeroizing::new("late".into()))
            })
        });
        gate.wait();
        service.clear(profile.id);
        assert!(waiter.join().unwrap().unwrap_err().is::<Cancelled>());
        assert_eq!(service.status(profile.id), Status::SignedOut);
    }
}
