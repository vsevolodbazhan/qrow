use super::{ActivityKind, Event, QueryState, Runner, Severity};
use anyhow::Result;
use std::{sync::atomic::Ordering, thread, time::Duration};

impl Runner {
    pub(super) fn idle_interval(&self) -> Option<Duration> {
        self.session.as_ref()?;
        let policy = &self.profile.as_ref()?.lifecycle;
        Some(Duration::from_secs(if policy.keep_alive_seconds == 0 {
            policy.idle_seconds
        } else {
            policy.keep_alive_seconds
        }))
    }

    pub(super) fn maintain(&mut self) {
        if self.stopped.load(Ordering::SeqCst) {
            return;
        }
        let policy = &self.profile.as_ref().unwrap().lifecycle;
        if policy.keep_alive_seconds == 0 {
            self.disconnect();
            self.activity(
                None,
                Severity::Info,
                ActivityKind::Disconnected,
                "Disconnected after idle timeout",
                None,
            );
            self.emit(Event::IdleDisconnected);
            return;
        }
        let sql = policy.keep_alive_sql.clone();
        self.cancelled.store(false, Ordering::SeqCst);
        self.emit(Event::KeepAliveStarted);
        if let Err(error) = self.keep_alive(&sql) {
            // A failed maintenance query must not repeat unattended.
            self.disconnect();
            let message = format!("Keep-alive failed: {error:#}");
            self.activity(
                None,
                Severity::Error,
                ActivityKind::Error,
                message.clone(),
                None,
            );
            self.emit(Event::Error {
                message,
                disconnected: true,
            });
        } else {
            self.emit(Event::KeepAliveFinished);
        }
    }

    fn keep_alive(&mut self, sql: &str) -> Result<()> {
        anyhow::ensure!(!self.stopped.load(Ordering::SeqCst), "Disconnect requested");
        let previous_target = self.target.lock().unwrap().take();
        let result = (|| -> Result<()> {
            let cancellation = self.session.as_mut().unwrap().execute_keep_alive(sql)?;
            *self.target.lock().unwrap() = Some(cancellation.clone());
            if self.cancelled.load(Ordering::SeqCst) || self.stopped.load(Ordering::SeqCst) {
                cancellation.cancel()?;
            }
            loop {
                match self.session.as_mut().unwrap().poll()? {
                    QueryState::Running => thread::sleep(Duration::from_millis(100)),
                    QueryState::Finished { .. } => return Ok(()),
                    QueryState::Cancelled => anyhow::bail!("Keep-alive was cancelled"),
                }
            }
        })();
        let closed = self.session.as_mut().unwrap().close_keep_alive();
        *self.target.lock().unwrap() = previous_target;
        result.and(closed)
    }
}
