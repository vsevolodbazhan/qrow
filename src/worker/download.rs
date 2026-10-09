//! Cancellation stays bound to the operation that owns this download.
use crate::{
    connector::Cancellation,
    export::{
        self,
        spool::{Producer, Spool},
    },
};
use std::{
    io,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cursor {
    #[default]
    Unavailable,
    Available,
    Draining,
    Downloaded,
    Consumed,
    Complete,
}

enum Route {
    Pending,
    Transport(Arc<dyn Cancellation>),
    Downloading(Arc<dyn Cancellation>),
    Downloaded,
    Stopped,
}

enum Source {
    Pending,
    Ready(Arc<Spool>),
    Failed(String),
    Cancelled,
}

struct State {
    source: Source,
    route: Route,
    cancellation_started: bool,
    cancellation_done: bool,
    cancellation_failed: bool,
    aborted: bool,
    guard: Option<export::Writer>,
}

pub struct Download {
    cancel: Arc<AtomicBool>,
    state: Mutex<State>,
    changed: Condvar,
}

impl Download {
    pub(super) fn new(spool: Arc<Spool>, cancel: Arc<AtomicBool>) -> Arc<Self> {
        Self::with_source(Source::Ready(spool), cancel)
    }

    pub(super) fn pending(cancel: Arc<AtomicBool>) -> Arc<Self> {
        Self::with_source(Source::Pending, cancel)
    }

    fn with_source(source: Source, cancel: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            cancel,
            state: Mutex::new(State {
                source,
                route: Route::Pending,
                cancellation_started: false,
                cancellation_done: false,
                cancellation_failed: false,
                aborted: false,
                guard: None,
            }),
            changed: Condvar::new(),
        })
    }

    pub fn spool(&self) -> Option<Arc<Spool>> {
        match &self.state.lock().unwrap().source {
            Source::Ready(spool) => Some(spool.clone()),
            _ => None,
        }
    }

    pub fn wait_spool(&self) -> io::Result<Arc<Spool>> {
        let mut state = self.state.lock().unwrap();
        loop {
            match &state.source {
                Source::Ready(spool) => return Ok(spool.clone()),
                Source::Failed(message) => return Err(io::Error::other(message.clone())),
                Source::Cancelled => return Err(io::Error::other(export::Cancelled)),
                Source::Pending => state = self.changed.wait(state).unwrap(),
            }
        }
    }

    pub(super) fn publish_spool(&self, spool: Arc<Spool>) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        export::check_cancelled(&self.cancel)?;
        if !matches!(state.source, Source::Pending) {
            return Err(io::Error::other(
                "The result source is already terminal or ready.",
            ));
        }
        state.source = Source::Ready(spool);
        self.changed.notify_all();
        Ok(())
    }

    pub(super) fn register_transport(&self, target: Arc<dyn Cancellation>) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        if export::check_cancelled(&self.cancel).is_err()
            || state.aborted
            || !matches!(state.route, Route::Pending | Route::Transport(_))
        {
            target.abort_transport();
            return Err(io::Error::other(export::Cancelled));
        }
        state.route = Route::Transport(target);
        Ok(())
    }

    fn terminal_source(&self, failed: Option<String>) {
        let spool = {
            let mut state = self.state.lock().unwrap();
            let spool = match &state.source {
                Source::Ready(spool) => Some(spool.clone()),
                Source::Pending => {
                    state.source = failed.clone().map_or(Source::Cancelled, Source::Failed);
                    None
                }
                _ => None,
            };
            self.changed.notify_all();
            spool
        };
        if let Some(spool) = spool {
            if let Some(message) = failed {
                spool.fail(message);
            } else {
                spool.cancel();
            }
        }
    }
    pub fn cancelled(&self) -> &AtomicBool {
        &self.cancel
    }

    pub(super) fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub(super) fn release_transport(&self) {
        let mut state = self.state.lock().unwrap();
        if matches!(state.route, Route::Transport(_)) && !state.aborted {
            state.route = Route::Pending;
        }
    }

    pub(super) fn finish_without_spool(&self, message: String) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        export::check_cancelled(&self.cancel)?;
        if !matches!(state.route, Route::Downloading(_)) || !matches!(state.source, Source::Pending)
        {
            return Err(io::Error::other("The execution stopped before completion."));
        }
        state.source = Source::Failed(message);
        state.route = Route::Downloaded;
        state.guard = None;
        self.changed.notify_all();
        Ok(())
    }

    pub(super) fn bind(
        &self,
        target: Arc<dyn Cancellation>,
        guard: export::Writer,
    ) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        if export::check_cancelled(&self.cancel).is_err() || state.aborted {
            target.abort_transport();
            return Err(io::Error::other(export::Cancelled));
        }
        if !matches!(state.route, Route::Pending | Route::Transport(_)) {
            return Err(io::Error::other("The download is already active."));
        }
        state.route = Route::Downloading(target);
        state.guard = Some(guard);
        Ok(())
    }

    /// The terminal spool record and cancellation routing have one handoff.
    pub(super) fn complete(&self, producer: Producer) -> io::Result<()> {
        producer.finish_with(&self.cancel, || {
            let mut state = self.state.lock().unwrap();
            export::check_cancelled(&self.cancel)?;
            if !matches!(state.route, Route::Downloading(_)) || state.aborted {
                return Err(io::Error::other("The download stopped before completion."));
            }
            state.route = Route::Downloaded;
            state.guard = None;
            self.changed.notify_all();
            Ok(())
        })
    }

    pub fn cancel(self: &Arc<Self>) {
        self.terminal_source(None);
        self.cancel.store(true, Ordering::SeqCst);
        self.cancel_operation();
    }

    pub fn cancel_in_background(self: &Arc<Self>) {
        self.terminal_source(None);
        self.cancel.store(true, Ordering::SeqCst);
        let download = self.clone();
        if std::thread::Builder::new()
            .name("qrow-export-stop".into())
            .spawn(move || download.cancel_operation())
            .is_err()
        {
            self.abort_operation();
        }
    }

    pub fn fail(self: &Arc<Self>, message: String) {
        self.terminal_source(Some(message));
        // A complete spool is replayable. Only this writer stops after the handoff.
        self.cancel.store(true, Ordering::SeqCst);
        self.cancel_operation();
    }

    /// Resource failure stops the owned transport without creating a thread.
    /// The routing lock protects only the handoff and never waits on disk I/O.
    pub fn abort(&self, message: String) {
        self.terminal_source(Some(message));
        self.cancel.store(true, Ordering::SeqCst);
        self.abort_operation();
    }

    fn abort_operation(&self) {
        let mut state = self.state.lock().unwrap();
        if let Route::Downloading(target) | Route::Transport(target) = &state.route {
            target.abort_transport();
            state.aborted = true;
            state.cancellation_done = true;
            state.cancellation_failed = true;
            state.guard = None;
            self.changed.notify_all();
        }
    }

    fn cancel_operation(self: &Arc<Self>) {
        self.cancel_operation_with(|work| {
            std::thread::Builder::new()
                .name("qrow-export-cancel".into())
                .spawn(work)
        });
    }

    fn cancel_operation_with(
        self: &Arc<Self>,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<std::thread::JoinHandle<()>>,
    ) {
        let (target, guard) = {
            let mut state = self.state.lock().unwrap();
            if let Route::Transport(target) = &state.route {
                target.abort_transport();
                state.aborted = true;
                state.cancellation_done = true;
                state.guard = None;
                self.changed.notify_all();
                return;
            }
            let Route::Downloading(target) = &state.route else {
                return;
            };
            let target = target.clone();
            if state.cancellation_started {
                return;
            }
            state.cancellation_started = true;
            (target, state.guard.take())
        };
        let download = self.clone();
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        if spawn(Box::new(move || {
            let _guard = guard;
            let watchdog = download.clone();
            let abort = target.clone();
            if std::thread::Builder::new()
                .name("qrow-export-deadline".into())
                .spawn(move || {
                    let mut state = watchdog.state.lock().unwrap();
                    while matches!(state.route, Route::Downloading(_)) {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            // Hold the routing lock so this can never abort a reused session.
                            state.aborted = true;
                            abort.abort_transport();
                            watchdog.changed.notify_all();
                            break;
                        }
                        state = watchdog.changed.wait_timeout(state, remaining).unwrap().0;
                    }
                })
                .is_err()
            {
                download.abort_operation();
            }
            let failed = target.cancel_with_deadline(deadline).is_err();
            let mut state = download.state.lock().unwrap();
            state.cancellation_done = true;
            state.cancellation_failed = failed;
            download.changed.notify_all();
        }))
        .is_err()
        {
            self.abort_operation();
        }
    }

    /// Wait for the captured cancel before the session can accept another query.
    /// False requires discarding the session, including its transport.
    pub(super) fn stopped(&self) -> bool {
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        let mut state = self.state.lock().unwrap();
        while state.cancellation_started && !state.cancellation_done && !state.aborted {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.aborted = true;
                break;
            }
            state = self.changed.wait_timeout(state, remaining).unwrap().0;
        }
        let clean = !state.aborted && !state.cancellation_failed;
        if !clean && let Route::Downloading(target) | Route::Transport(target) = &state.route {
            target.abort_transport();
        }
        state.route = Route::Stopped;
        state.guard = None;
        self.changed.notify_all();
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::spool::Status;
    use std::sync::atomic::AtomicUsize;

    struct Target {
        spool: Arc<Spool>,
        calls: AtomicUsize,
        aborted: AtomicBool,
        blocking: bool,
        release: (Mutex<bool>, Condvar),
    }
    impl Cancellation for Target {
        fn cancel(&self) -> anyhow::Result<()> {
            assert!(matches!(
                self.spool.status(),
                Status::Cancelled | Status::Failed(_)
            ));
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut released = self.release.0.lock().unwrap();
            while self.blocking && !*released {
                released = self.release.1.wait(released).unwrap();
            }
            Ok(())
        }
        fn abort_transport(&self) {
            self.aborted.store(true, Ordering::SeqCst);
            *self.release.0.lock().unwrap() = true;
            self.release.1.notify_all();
        }
    }
    fn fixture(blocking: bool) -> (Arc<Download>, Producer, Arc<Target>) {
        let columns = [crate::model::Column {
            name: "n".into(),
            data_type: "INT".into(),
        }];
        let (spool, producer) = Spool::new(&columns, &export::Context::default()).unwrap();
        let target = Arc::new(Target {
            spool: spool.clone(),
            calls: AtomicUsize::new(0),
            aborted: AtomicBool::new(false),
            blocking,
            release: (Mutex::new(false), Condvar::new()),
        });
        let download = Download::new(spool, Arc::new(AtomicBool::new(false)));
        download
            .bind(
                target.clone(),
                export::Jobs::default().register(Arc::new(AtomicBool::new(false))),
            )
            .unwrap();
        (download, producer, target)
    }

    #[test]
    fn cancellation_publishes_a_sticky_terminal_before_protocol_work() {
        let (download, producer, target) = fixture(false);
        download.cancel();
        download.cancel();
        assert!(download.stopped());
        assert_eq!(download.spool().unwrap().status(), Status::Cancelled);
        assert_eq!(target.calls.load(Ordering::SeqCst), 1);
        assert!(producer.finish(&AtomicBool::new(false)).is_err());
    }

    #[test]
    fn pending_source_wakes_all_readers_and_rejects_late_schema_and_transport() {
        for failure in [false, true] {
            let (ready, _producer, target) = fixture(false);
            let pending = Download::pending(Arc::new(AtomicBool::new(false)));
            let readers: Vec<_> = (0..3)
                .map(|_| {
                    let pending = pending.clone();
                    std::thread::spawn(move || pending.wait_spool().err().unwrap())
                })
                .collect();
            if failure {
                pending.fail("First failure".into());
                pending.cancel();
                pending.fail("Later failure".into());
            } else {
                pending.cancel();
                pending.fail("Later failure".into());
            }
            for reader in readers {
                let error = reader.join().unwrap();
                if failure {
                    assert_eq!(error.to_string(), "First failure");
                } else {
                    assert!(error.get_ref().unwrap().is::<export::Cancelled>());
                }
            }
            assert!(pending.publish_spool(ready.spool().unwrap()).is_err());
            assert!(pending.register_transport(target.clone()).is_err());
            assert!(target.aborted.load(Ordering::SeqCst));
            assert_eq!(target.calls.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn a_complete_download_detaches_cancel_and_writer_error_from_the_session() {
        let (download, producer, target) = fixture(false);
        download.complete(producer).unwrap();
        download.fail("output failed".into());
        download.cancel();
        assert_eq!(
            download.spool().unwrap().status(),
            Status::Complete { rows: 0 }
        );
        assert_eq!(target.calls.load(Ordering::SeqCst), 0);
        assert!(!target.aborted.load(Ordering::SeqCst));
    }

    #[test]
    fn failed_cancel_thread_creation_aborts_the_owned_operation_without_retry() {
        let (download, producer, target) = fixture(false);
        download.spool().unwrap().cancel();
        download.cancel.store(true, Ordering::SeqCst);
        download.cancel_operation_with(|_| Err(io::Error::other("No thread resources")));
        assert!(!download.stopped());
        assert!(target.aborted.load(Ordering::SeqCst));
        assert_eq!(target.calls.load(Ordering::SeqCst), 0);
        assert_eq!(download.spool().unwrap().status(), Status::Cancelled);
        assert!(producer.finish(&AtomicBool::new(false)).is_err());
    }

    #[test]
    fn thread_failure_waits_for_routing_contention_and_applies_the_abort() {
        let (download, _producer, target) = fixture(false);
        download.spool().unwrap().cancel();
        download.cancel.store(true, Ordering::SeqCst);
        let pending = download.clone();
        let (waiting, started) = std::sync::mpsc::channel();
        let (proceed, allowed) = std::sync::mpsc::channel();
        let (finished, done) = std::sync::mpsc::channel();
        let cleanup = std::thread::spawn(move || {
            pending.cancel_operation_with(|_| {
                waiting.send(()).unwrap();
                allowed.recv().unwrap();
                Err(io::Error::other("No thread resources"))
            });
            finished.send(()).unwrap();
        });
        started.recv().unwrap();
        let state = download.state.lock().unwrap();
        proceed.send(()).unwrap();
        assert!(done.recv_timeout(Duration::from_millis(100)).is_err());
        assert!(!target.aborted.load(Ordering::SeqCst));
        drop(state);
        cleanup.join().unwrap();
        assert!(target.aborted.load(Ordering::SeqCst));
        assert!(!download.stopped());
        assert_eq!(target.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn emergency_writer_failure_keeps_the_complete_spool_and_detaches_the_session() {
        let (download, producer, target) = fixture(false);
        download.complete(producer).unwrap();
        download.abort("writer stopped".into());
        assert_eq!(
            download.spool().unwrap().status(),
            Status::Complete { rows: 0 }
        );
        assert!(!target.aborted.load(Ordering::SeqCst));
        assert_eq!(target.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stuck_cleanup_aborts_the_captured_transport_within_one_deadline() {
        let (download, _producer, target) = fixture(true);
        let started = Instant::now();
        download.cancel();
        assert!(!download.stopped());
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(target.aborted.load(Ordering::SeqCst));
        assert_eq!(download.spool().unwrap().status(), Status::Cancelled);
    }
}
