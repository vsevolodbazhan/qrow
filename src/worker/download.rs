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
    Downloading(Arc<dyn Cancellation>),
    Downloaded,
    Stopped,
}

struct State {
    route: Route,
    cancellation_started: bool,
    cancellation_done: bool,
    cancellation_failed: bool,
    aborted: bool,
    guard: Option<export::Writer>,
}

pub struct Download {
    spool: Arc<Spool>,
    cancel: Arc<AtomicBool>,
    state: Mutex<State>,
    changed: Condvar,
}

impl Download {
    pub(super) fn new(spool: Arc<Spool>, cancel: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            spool,
            cancel,
            state: Mutex::new(State {
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

    pub fn spool(&self) -> &Arc<Spool> {
        &self.spool
    }
    pub fn cancelled(&self) -> &AtomicBool {
        &self.cancel
    }

    pub(super) fn bind(
        &self,
        target: Arc<dyn Cancellation>,
        guard: export::Writer,
    ) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        export::check_cancelled(&self.cancel)?;
        if !matches!(state.route, Route::Pending) {
            return Err(io::Error::other("The download is already active."));
        }
        state.route = Route::Downloading(target);
        state.guard = Some(guard);
        Ok(())
    }

    /// The terminal spool record and cancellation routing have one handoff.
    pub(super) fn complete(&self, producer: Producer) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        export::check_cancelled(&self.cancel)?;
        if !matches!(state.route, Route::Downloading(_)) || state.aborted {
            return Err(io::Error::other("The download stopped before completion."));
        }
        producer.finish(&self.cancel)?;
        state.route = Route::Downloaded;
        state.guard = None;
        self.changed.notify_all();
        Ok(())
    }

    pub fn cancel(self: &Arc<Self>) {
        self.cancel.store(true, Ordering::SeqCst);
        self.spool.cancel();
        self.cancel_operation();
    }

    pub fn fail(self: &Arc<Self>, message: String) {
        self.spool.fail(message);
        // A complete spool is replayable. Only this writer stops after the handoff.
        self.cancel.store(true, Ordering::SeqCst);
        self.cancel_operation();
    }

    fn cancel_operation(self: &Arc<Self>) {
        let (target, guard) = {
            let mut state = self.state.lock().unwrap();
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
        std::thread::spawn(move || {
            let _guard = guard;
            let watchdog = download.clone();
            let abort = target.clone();
            std::thread::spawn(move || {
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
            });
            let failed = target.cancel_with_deadline(deadline).is_err();
            let mut state = download.state.lock().unwrap();
            state.cancellation_done = true;
            state.cancellation_failed = failed;
            download.changed.notify_all();
        });
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
        if !clean && let Route::Downloading(target) = &state.route {
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
        assert_eq!(download.spool().status(), Status::Cancelled);
        assert_eq!(target.calls.load(Ordering::SeqCst), 1);
        assert!(producer.finish(&AtomicBool::new(false)).is_err());
    }

    #[test]
    fn a_complete_download_detaches_cancel_and_writer_error_from_the_session() {
        let (download, producer, target) = fixture(false);
        download.complete(producer).unwrap();
        download.fail("output failed".into());
        download.cancel();
        assert_eq!(download.spool().status(), Status::Complete { rows: 0 });
        assert_eq!(target.calls.load(Ordering::SeqCst), 0);
        assert!(!target.aborted.load(Ordering::SeqCst));
    }

    #[test]
    fn stuck_cleanup_aborts_the_captured_transport_within_one_deadline() {
        let (download, _producer, target) = fixture(true);
        let started = Instant::now();
        download.cancel();
        assert!(!download.stopped());
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(target.aborted.load(Ordering::SeqCst));
        assert_eq!(download.spool().status(), Status::Cancelled);
    }
}
