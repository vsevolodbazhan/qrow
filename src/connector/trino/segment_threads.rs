//! At most two owned helper handles, retained through actual thread exit.
use super::{segment_download::Control, transport::Transport};
use anyhow::{Result, ensure};
use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Threads {
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    next: u64,
    handles: Vec<(u64, thread::JoinHandle<()>)>,
}
impl Threads {
    pub fn spawn(
        &self,
        scope: &Arc<Transport>,
        task: impl FnOnce() + Send + 'static,
    ) -> Result<u64> {
        // Cancellation can observe idle only after this handle is published.
        let mut state = self.state.lock().unwrap();
        ensure!(
            state.handles.len() < 2,
            "Trino segment thread limit reached"
        );
        let activity = scope.activity()?;
        let id = state.next;
        state.next = state
            .next
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Trino segment thread ID overflow"))?;
        let handle = thread::Builder::new()
            .name("trino-segment".into())
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let _activity = activity;
                task();
            })?;
        state.handles.push((id, handle));
        Ok(id)
    }
    fn take_finished(&self, id: Option<u64>) -> Option<thread::JoinHandle<()>> {
        let mut state = self.state.lock().unwrap();
        let index = state
            .handles
            .iter()
            .position(|(key, handle)| id.is_none_or(|id| *key == id) && handle.is_finished())?;
        Some(state.handles.swap_remove(index).1)
    }
    fn contains(&self, id: Option<u64>) -> bool {
        self.state
            .lock()
            .unwrap()
            .handles
            .iter()
            .any(|(key, _)| id.is_none_or(|id| *key == id))
    }
    pub fn join(&self, id: u64, control: &Control) -> Result<()> {
        let deadline = Instant::now() + control.timeout;
        loop {
            control.check()?;
            if let Some(handle) = self.take_finished(Some(id)) {
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("Trino segment worker panicked"))?;
                return Ok(());
            }
            if !self.contains(Some(id)) {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Trino segment worker did not end in time"
            );
            control.delay(Duration::from_millis(5), Some(deadline))?;
        }
    }
    pub fn join_all(&self, deadline: Instant) -> Result<()> {
        loop {
            while let Some(handle) = self.take_finished(None) {
                // A panic already failed its receiver; cleanup must join the other worker too.
                let _ = handle.join();
            }
            if !self.contains(None) {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            ensure!(
                !remaining.is_zero(),
                "Trino segment cleanup deadline expired"
            );
            thread::sleep(remaining.min(Duration::from_millis(5)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::budget::{Budget, MIB};
    use std::sync::mpsc;
    #[test]
    fn failed_cleanup_keeps_memory_until_actual_worker_exit_and_retained_registry_does_not()
    -> Result<()> {
        let budget = Budget::new(512 * MIB);
        let allowance = budget.allowance(512 * MIB)?;
        let scope = Transport::new();
        let threads = Arc::new(Threads::default());
        let (ready, seen) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let id = threads.spawn(&scope, move || {
            let _allowance = allowance;
            ready.send(()).unwrap();
            gate.recv().unwrap();
        })?;
        seen.recv_timeout(Duration::from_secs(2))?;
        scope.seal();
        scope.close();
        assert!(
            threads
                .join_all(Instant::now() + Duration::from_millis(20))
                .is_err()
        );
        assert!(budget.allowance(1).is_err());
        assert!(threads.spawn(&scope, || {}).is_err());
        release.send(())?;
        threads.join_all(Instant::now() + Duration::from_secs(2))?;
        assert!(!threads.contains(Some(id)));
        assert!(budget.allowance(512 * MIB).is_ok());
        Ok(())
    }
}
