//! Ordered segment slots; the fetch thread owns progression, retries, and ACKs.
use super::{
    segment_download::{Control, Decoded},
    spooling::{Page, Spec},
};
use crate::model::{Batch, Row, transfer::TrinoSpooling};
use anyhow::{Context, Result, ensure};
use std::{
    collections::VecDeque,
    sync::{Arc, mpsc},
    time::Duration,
};

struct Job {
    spec: Arc<Spec>,
    result: mpsc::Receiver<std::result::Result<Decoded, super::segment_download::Failure>>,
    thread_id: u64,
    attempts: u8,
}
impl Job {
    fn start(spec: Arc<Spec>, control: &Control, attempts: u8) -> Result<Self> {
        control.check()?;
        let (tx, result) = mpsc::channel();
        let owned = control.clone();
        let descriptor = spec.clone();
        let thread_id = control.threads.spawn(&control.scope, move || {
            let result = owned.attempt(&descriptor);
            let _ = tx.send(result);
        })?;
        Ok(Self {
            spec,
            result,
            thread_id,
            attempts,
        })
    }
    fn wait(mut self, control: &Control) -> Result<Current> {
        loop {
            control.check()?;
            let result = loop {
                control.check()?;
                match self.result.recv_timeout(Duration::from_millis(25)) {
                    Ok(result) => break result,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        anyhow::bail!("Trino segment worker stopped")
                    }
                }
            };
            control.threads.join(self.thread_id, control)?;
            match result {
                Ok(decoded) => {
                    return Ok(Current {
                        spec: self.spec,
                        decoded,
                        emitted: 0,
                        committed: 0,
                        pending: None,
                    });
                }
                Err(failure) => {
                    control.check()?;
                    if !failure.retryable || self.attempts == 3 {
                        return Err(failure.error);
                    }
                    self.spec.check_expiry()?;
                    control.delay(
                        Duration::from_millis(if self.attempts == 1 { 100 } else { 250 }),
                        None,
                    )?;
                    self.spec.check_expiry()?;
                    self = Self::start(self.spec, control, self.attempts + 1)?;
                }
            }
        }
    }
}
struct Current {
    spec: Arc<Spec>,
    decoded: Decoded,
    emitted: u64,
    committed: u64,
    pending: Option<Row>,
}
struct Commit {
    rows: usize,
    boundary: bool,
}
pub(super) struct Reader {
    pub page: Option<Page>,
    encoding: Option<super::spooling::Encoding>,
    mode: TrinoSpooling,
    control: Control,
    jobs: VecDeque<Job>,
    current: Option<Current>,
    awaiting: Option<Commit>,
    offset: u64,
    pub ack_failures: u64,
}
impl Reader {
    pub fn new(mode: TrinoSpooling, control: Control) -> Self {
        Self {
            page: None,
            encoding: None,
            mode,
            control,
            jobs: VecDeque::with_capacity(2),
            current: None,
            awaiting: None,
            offset: 0,
            ack_failures: 0,
        }
    }
    pub fn install(&mut self, page: Page) -> Result<()> {
        ensure!(
            self.page.as_ref().is_none_or(Page::is_empty),
            "Trino replaced an unread segment page"
        );
        ensure!(
            self.encoding
                .is_none_or(|encoding| encoding == page.encoding),
            "Trino changed segment encoding"
        );
        self.encoding = Some(page.encoding);
        self.page = Some(page);
        Ok(())
    }
    pub fn has_slots(&self) -> bool {
        self.current.is_some() || !self.jobs.is_empty()
    }
    pub fn has_input(&self) -> bool {
        self.has_slots()
            || self.awaiting.is_some()
            || self.page.as_ref().is_some_and(|p| !p.is_empty())
    }
    pub fn has_space(&self) -> bool {
        self.jobs.len() + usize::from(self.current.is_some())
            < if self.mode == TrinoSpooling::Parallel {
                2
            } else {
                1
            }
    }
    pub fn admit(&mut self) -> Result<bool> {
        ensure!(self.has_space(), "Trino segment slot limit reached");
        let Some(spec) = self.page.as_mut().map(Page::next).transpose()?.flatten() else {
            self.page = None;
            return Ok(false);
        };
        ensure!(
            spec.offset == self.offset,
            "Trino segment row offset mismatch"
        );
        self.offset = self
            .offset
            .checked_add(spec.rows)
            .context("Trino segment row count overflow")?;
        self.jobs
            .push_back(Job::start(Arc::new(spec), &self.control, 1)?);
        Ok(true)
    }
    pub fn ready_to_fetch(&self) -> Result<()> {
        self.control.check()?;
        ensure!(
            self.awaiting.is_none(),
            "Commit the previous Trino export batch before fetching again"
        );
        Ok(())
    }
    pub fn fetch(&mut self, count: usize, width: usize) -> Result<Batch> {
        self.control.check()?;
        ensure!(
            self.awaiting.is_none(),
            "Commit the previous Trino export batch before fetching again"
        );
        if self.current.is_none() {
            self.current = self
                .jobs
                .pop_front()
                .map(|job| job.wait(&self.control))
                .transpose()?;
        }
        let Some(current) = self.current.as_mut() else {
            return Ok(Batch { rows: Vec::new() });
        };
        let mut rows = Vec::with_capacity(count.min(1000));
        let mut bytes = rows.capacity() * std::mem::size_of::<Row>();
        while rows.len() < count.min(1000) && current.emitted < current.spec.rows {
            self.control.check()?;
            let row =
                current.pending.take().map(Ok).unwrap_or_else(|| {
                    current.decoded.rows.next(width, true).and_then(|row| {
                        row.context("Trino segment contains fewer rows than declared")
                    })
                })?;
            let owned = crate::export::budget::row_bytes(&row)?;
            if !rows.is_empty() && bytes.saturating_add(owned) > 48 * crate::export::budget::MIB {
                current.pending = Some(row);
                break;
            }
            bytes += owned;
            current.emitted += 1;
            rows.push(row);
        }
        let boundary = current.emitted == current.spec.rows;
        if boundary {
            ensure!(
                current.decoded.rows.next(width, true)?.is_none(),
                "Trino segment contains more rows than declared"
            );
        }
        self.awaiting = Some(Commit {
            rows: rows.len(),
            boundary,
        });
        Ok(Batch { rows })
    }
    pub fn commit(&mut self, rows: usize) -> Result<bool> {
        self.control.check()?;
        let Some(commit) = self.awaiting.as_ref() else {
            ensure!(
                rows == 0 && self.current.is_none(),
                "No Trino export batch is awaiting commit"
            );
            return Ok(false);
        };
        ensure!(
            rows == commit.rows,
            "Trino export batch commit count mismatch"
        );
        let commit = self.awaiting.take().unwrap();
        let current = self
            .current
            .as_mut()
            .context("Missing Trino segment during commit")?;
        current.committed = current
            .committed
            .checked_add(rows as u64)
            .context("Trino commit count overflow")?;
        if commit.boundary {
            ensure!(
                current.committed == current.spec.rows,
                "Trino segment has uncommitted rows"
            );
            if !self.control.ack(&current.spec)? {
                self.ack_failures = self.ack_failures.saturating_add(1);
            }
            self.current = None;
        }
        // A zero-row segment boundary is not result EOF, including the final segment.
        Ok(true)
    }
}
