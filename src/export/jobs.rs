//! Active export writers participate in application shutdown. Completion is
//! signalled on the writing thread, so shutdown does not depend on UI callbacks.

use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct Jobs {
    inner: Arc<Mutex<Registry>>,
}

#[derive(Default)]
struct Registry {
    stopping: bool,
    jobs: Vec<Arc<State>>,
}

struct State {
    cancel: Arc<AtomicBool>,
    done: Mutex<bool>,
    changed: Condvar,
}

/// Keep this guard on the writing thread until its temporary output is removed
/// or published. Dropping the guard signals completion even after an error.
pub struct Writer {
    state: Arc<State>,
}

impl Jobs {
    pub fn register(&self, cancel: Arc<AtomicBool>) -> Writer {
        let state = Arc::new(State {
            cancel,
            done: Mutex::new(false),
            changed: Condvar::new(),
        });
        let mut registry = self.inner.lock().unwrap();
        registry.jobs.retain(|job| !*job.done.lock().unwrap());
        if registry.stopping {
            state.cancel.store(true, Ordering::Relaxed);
        }
        registry.jobs.push(state.clone());
        Writer { state }
    }

    pub fn active_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .jobs
            .iter()
            .filter(|job| !*job.done.lock().unwrap())
            .count()
    }

    /// Cancel the current writers without preventing later work if quit fails.
    pub fn cancel(&self) {
        for job in &self.inner.lock().unwrap().jobs {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Stop accepting new work, cancel all writers, then wait within one deadline.
    /// Returns false when a destination remains blocked after the deadline.
    pub fn cancel_and_wait(&self, timeout: Duration) -> bool {
        let jobs = {
            let mut registry = self.inner.lock().unwrap();
            registry.stopping = true;
            for job in &registry.jobs {
                job.cancel.store(true, Ordering::Relaxed);
            }
            registry.jobs.clone()
        };
        let deadline = Instant::now() + timeout;
        for job in jobs {
            let mut done = job.done.lock().unwrap();
            while !*done {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                let (next, _) = job.changed.wait_timeout(done, remaining).unwrap();
                done = next;
            }
        }
        true
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        *self.state.done.lock().unwrap() = true;
        self.state.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, sync::mpsc};

    #[test]
    fn shutdown_waits_for_cancelled_output_cleanup_without_a_ui_callback() {
        let jobs = Jobs::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let guard = jobs.register(cancel.clone());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("result.csv");
        std::fs::write(&path, b"original").unwrap();
        let (started, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let writer_path = path.clone();
        let worker = std::thread::spawn(move || {
            let _guard = guard;
            super::super::save(&writer_path, &cancel, |out| {
                out.write_all(b"replacement")?;
                started.send(()).unwrap();
                released.recv().unwrap();
                Ok(1)
            })
        });
        ready.recv().unwrap();
        assert_eq!(jobs.active_count(), 1);
        // A rejected quit has no effect on the registry or cancellation state.
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert!(!jobs.cancel_and_wait(Duration::ZERO));
        release.send(()).unwrap();
        assert!(jobs.cancel_and_wait(Duration::from_secs(2)));
        assert!(worker.join().unwrap().is_err());
        assert_eq!(jobs.active_count(), 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn blocked_writers_have_one_bounded_deadline_and_late_work_is_cancelled() {
        let jobs = Jobs::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let guard = jobs.register(cancel.clone());
        assert!(!jobs.cancel_and_wait(Duration::ZERO));
        assert!(cancel.load(Ordering::Relaxed));
        let late_cancel = Arc::new(AtomicBool::new(false));
        let late = jobs.register(late_cancel.clone());
        assert!(late_cancel.load(Ordering::Relaxed));
        drop(guard);
        drop(late);
        assert!(jobs.cancel_and_wait(Duration::ZERO));
    }
}
