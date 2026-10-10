//! Active export writers participate in application shutdown. Completion is
//! signalled on the writing thread, so shutdown does not depend on UI callbacks.

use std::{
    io,
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
    profile: Option<uuid::Uuid>,
    on_cancel: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    remaining: Mutex<usize>,
    changed: Condvar,
}

/// Keep this guard on the writing thread until its temporary output is removed
/// or published. Dropping the guard signals completion even after an error.
pub struct Writer {
    state: Arc<State>,
}

impl Jobs {
    pub fn register(&self, cancel: Arc<AtomicBool>) -> Writer {
        self.register_with_cancel(cancel, None)
    }

    pub fn register_with_cancel(
        &self,
        cancel: Arc<AtomicBool>,
        on_cancel: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Writer {
        self.register_inner(cancel, on_cancel, None)
            .expect("Unrestricted job registration cannot fail")
    }

    /// Admit one connection export before SQL, then share its permit with each stage.
    pub fn register_for_profile(
        &self,
        cancel: Arc<AtomicBool>,
        on_cancel: Option<Arc<dyn Fn() + Send + Sync>>,
        profile: uuid::Uuid,
        limit: u8,
    ) -> io::Result<Writer> {
        if !(1..=2).contains(&limit) {
            return Err(io::Error::other("Exports at the same time must be 1 or 2."));
        }
        self.register_inner(cancel, on_cancel, Some((profile, limit)))
    }

    fn register_inner(
        &self,
        cancel: Arc<AtomicBool>,
        on_cancel: Option<Arc<dyn Fn() + Send + Sync>>,
        profile: Option<(uuid::Uuid, u8)>,
    ) -> io::Result<Writer> {
        let mut registry = self.inner.lock().unwrap();
        registry
            .jobs
            .retain(|job| *job.remaining.lock().unwrap() > 0);
        // Admission can precede the captured producer callback. Stages with
        // the same cancellation flag share one connection permit.
        if on_cancel.is_none() || profile.is_some() {
            for job in &registry.jobs {
                if Arc::ptr_eq(&job.cancel, &cancel) {
                    if profile.is_some_and(|(id, _)| job.profile != Some(id)) {
                        return Err(io::Error::other("The export connection changed."));
                    }
                    let mut remaining = job.remaining.lock().unwrap();
                    if *remaining > 0 {
                        if let Some(callback) = &on_cancel {
                            let mut existing = job.on_cancel.lock().unwrap();
                            if existing.is_some() {
                                return Err(io::Error::other(
                                    "This export already owns a download.",
                                ));
                            }
                            *existing = Some(callback.clone());
                        }
                        *remaining += 1;
                        let state = job.clone();
                        let cancelled = registry.stopping || cancel.load(Ordering::Relaxed);
                        drop(remaining);
                        drop(registry);
                        if cancelled && let Some(callback) = on_cancel {
                            callback();
                        }
                        return Ok(Writer { state });
                    }
                }
            }
        }
        if let Some((id, limit)) = profile {
            if registry.stopping {
                return Err(io::Error::other(super::Cancelled));
            }
            if registry
                .jobs
                .iter()
                .filter(|job| job.profile == Some(id))
                .count()
                >= usize::from(limit)
            {
                return Err(io::Error::other(format!(
                    "This connection already has {limit} active {}. Wait for an export to finish or cancel it.",
                    if limit == 1 { "export" } else { "exports" }
                )));
            }
        }
        let state = Arc::new(State {
            cancel,
            profile: profile.map(|(id, _)| id),
            on_cancel: Mutex::new(on_cancel),
            remaining: Mutex::new(1),
            changed: Condvar::new(),
        });
        let stopping = registry.stopping;
        registry.jobs.push(state.clone());
        drop(registry);
        if stopping {
            state.request_cancel();
        }
        Ok(Writer { state })
    }

    pub fn active_count(&self) -> usize {
        let mut registry = self.inner.lock().unwrap();
        registry
            .jobs
            .retain(|job| *job.remaining.lock().unwrap() > 0);
        registry.jobs.len()
    }

    /// Cancel the current writers without preventing later work if quit fails.
    pub fn cancel(&self) {
        let jobs = self.inner.lock().unwrap().jobs.clone();
        for job in &jobs {
            job.request_cancel();
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
        for job in &jobs {
            job.request_cancel();
        }
        let deadline = Instant::now() + timeout;
        for job in jobs {
            let mut remaining_tasks = job.remaining.lock().unwrap();
            while *remaining_tasks > 0 {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                let (next, _) = job
                    .changed
                    .wait_timeout(remaining_tasks, remaining)
                    .unwrap();
                remaining_tasks = next;
            }
        }
        true
    }
}

impl State {
    fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        let callback = self.on_cancel.lock().unwrap().clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        *self.state.remaining.lock().unwrap() -= 1;
        self.state.changed.notify_all();
    }
}

impl Writer {
    /// Register another stage of this job before the current stage ends.
    pub(crate) fn fork(&self) -> Self {
        *self.state.remaining.lock().unwrap() += 1;
        Self {
            state: self.state.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, sync::mpsc};

    #[test]
    fn cancellation_before_download_attachment_dispatches_the_late_callback() {
        let jobs = Jobs::default();
        let id = uuid::Uuid::new_v4();
        let flag = Arc::new(AtomicBool::new(false));
        let admission = jobs
            .register_for_profile(flag.clone(), None, id, 1)
            .unwrap();
        jobs.cancel();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let captured = calls.clone();
        let reentrant = jobs.clone();
        let producer = jobs
            .register_for_profile(
                flag,
                Some(Arc::new(move || {
                    assert_eq!(reentrant.active_count(), 1);
                    captured.fetch_add(1, Ordering::SeqCst);
                })),
                id,
                1,
            )
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        drop((admission, producer));
    }

    #[test]
    fn shutdown_dispatches_cleanup_when_download_attaches_after_admission() {
        let jobs = Jobs::default();
        let id = uuid::Uuid::new_v4();
        let flag = Arc::new(AtomicBool::new(false));
        let admission = jobs
            .register_for_profile(flag.clone(), None, id, 1)
            .unwrap();
        let gate = Arc::new(std::sync::Barrier::new(2));
        let wait_gate = gate.clone();
        let shutting_down = jobs.clone();
        let shutdown = std::thread::spawn(move || {
            wait_gate.wait();
            shutting_down.cancel_and_wait(Duration::from_secs(2))
        });
        gate.wait();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !flag.load(Ordering::Relaxed) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let (sent, received) = mpsc::channel();
        let producer = jobs
            .register_for_profile(
                flag,
                Some(Arc::new(move || {
                    sent.send(()).unwrap();
                })),
                id,
                1,
            )
            .unwrap();
        received.recv_timeout(Duration::from_secs(1)).unwrap();
        drop((admission, producer));
        assert!(shutdown.join().unwrap());
    }

    #[test]
    fn profile_permit_lasts_until_every_stage_finishes() {
        let jobs = Jobs::default();
        let profile = uuid::Uuid::new_v4();
        let flag = Arc::new(AtomicBool::new(false));
        let admission = jobs
            .register_for_profile(flag.clone(), None, profile, 1)
            .unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback_calls = calls.clone();
        let producer = jobs
            .register_for_profile(
                flag.clone(),
                Some(Arc::new(move || {
                    callback_calls.fetch_add(1, Ordering::SeqCst);
                })),
                profile,
                1,
            )
            .unwrap();
        let writer = jobs.register(flag);
        assert_eq!(jobs.active_count(), 1);
        let next = || jobs.register_for_profile(Arc::new(AtomicBool::new(false)), None, profile, 1);
        assert!(next().is_err());
        drop(admission);
        drop(producer);
        assert!(next().is_err());
        jobs.cancel();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(next().is_err());
        drop(writer);
        assert!(next().is_ok());
    }

    #[test]
    fn profile_limits_are_shared_between_tabs_and_independent_between_connections() {
        let jobs = Jobs::default();
        let profile = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        let admit = |id| jobs.register_for_profile(Arc::new(AtomicBool::new(false)), None, id, 2);
        let first = admit(profile).unwrap();
        let second = admit(profile).unwrap();
        assert!(admit(profile).is_err());
        let independent = admit(other).unwrap();
        assert_eq!(jobs.active_count(), 3);
        drop(first);
        let replacement = admit(profile).unwrap();
        drop((second, independent, replacement));
        assert_eq!(jobs.active_count(), 0);
    }

    #[test]
    fn shutdown_waits_for_all_stages_of_one_job() {
        let jobs = Jobs::default();
        let producer = jobs.register(Arc::new(AtomicBool::new(false)));
        let cancellation = producer.fork();
        assert_eq!(jobs.active_count(), 1);
        drop(producer);
        assert!(!jobs.cancel_and_wait(Duration::ZERO));
        drop(cancellation);
        assert!(jobs.cancel_and_wait(Duration::ZERO));
        assert_eq!(jobs.active_count(), 0);
    }

    #[test]
    fn registering_a_writer_reuses_its_producer_job_and_cancellation() {
        use std::sync::atomic::AtomicUsize;
        let jobs = Jobs::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = calls.clone();
        let producer = jobs.register_with_cancel(
            cancel.clone(),
            Some(Arc::new(move || {
                callback_calls.fetch_add(1, Ordering::Relaxed);
            })),
        );
        let writer = jobs.register(cancel.clone());
        assert_eq!(jobs.active_count(), 1);
        jobs.cancel();
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        drop(producer);
        assert_eq!(jobs.active_count(), 1);
        assert!(!jobs.cancel_and_wait(Duration::ZERO));
        drop(writer);
        assert!(jobs.cancel_and_wait(Duration::ZERO));
    }

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
