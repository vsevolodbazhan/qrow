//! New executions download in the tab worker, with a separately owned writer.
use super::*;
use crate::{
    connector::ConnectionControl,
    export::{self, budget, spool::Spool},
    model::DatabaseType,
};
use std::io;
use uuid::Uuid;

#[derive(Debug)]
pub struct SessionChanged;
impl std::fmt::Display for SessionChanged {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("The original session ended or changed. Its settings, temporary tables, and open transaction are gone. Review the export before running again.")
    }
}
impl std::error::Error for SessionChanged {}

pub(super) struct Request {
    profile: Profile,
    sql: String,
    execution: ExecutionId,
    expected: Option<Uuid>,
    download: Arc<Download>,
    guard: export::Writer,
    memory: Arc<budget::Allowance>,
    _transport: Arc<budget::Allowance>,
    finished: bool,
}
impl Drop for Request {
    fn drop(&mut self) {
        if !self.finished {
            self.download
                .fail("The execution worker stopped before the download completed.".into());
        }
    }
}

fn matches_session(
    session: &Mutex<Option<SessionIdentity>>,
    profile: &Profile,
    expected: Uuid,
) -> bool {
    session.lock().unwrap().as_ref().is_some_and(|session| {
        session.generation == expected && session.profile.connection_identity_eq(profile)
    })
}

impl Worker {
    pub fn session_generation(&self, profile: &Profile) -> Option<Uuid> {
        self.session_identity
            .lock()
            .unwrap()
            .as_ref()
            .filter(|session| session.profile.connection_identity_eq(profile))
            .map(|session| session.generation)
    }

    /// Reserve the source before submitting SQL. An expected generation must
    /// still be live when the worker accepts this command.
    pub fn run_and_export(
        &self,
        profile: Profile,
        sql: String,
        execution: ExecutionId,
        expected: Option<Uuid>,
        jobs: &export::Jobs,
        cancel: Arc<AtomicBool>,
    ) -> io::Result<Arc<Download>> {
        profile.validate().map_err(io::Error::other)?;
        crate::sql::validate_single_for(&sql, profile.database_type).map_err(io::Error::other)?;
        if matches!(
            profile.database_type,
            DatabaseType::Postgres | DatabaseType::Trino
        ) && sql.len() > 1024 * 1024
        {
            return Err(io::Error::other("Export SQL exceeds the 1 MiB limit"));
        }
        if expected
            .is_some_and(|expected| !matches_session(&self.session_identity, &profile, expected))
        {
            return Err(io::Error::other(SessionChanged));
        }
        let memory = budget::GLOBAL.allowance(export::spool::PRODUCER_MEMORY)?;
        let transport = budget::GLOBAL.allowance(match profile.database_type {
            DatabaseType::Trino
                if profile.transfer.settings().trino_spooling
                    != crate::model::transfer::TrinoSpooling::Off =>
            {
                512 * budget::MIB
            }
            DatabaseType::Postgres | DatabaseType::Trino => 192 * budget::MIB,
            DatabaseType::Kyuubi => 64 * budget::MIB,
        })?;
        let download = Download::pending(cancel.clone());
        let weak = Arc::downgrade(&download);
        let guard = jobs.register_for_profile(
            cancel,
            Some(Arc::new(move || {
                if let Some(download) = weak.upgrade() {
                    download.cancel_in_background();
                }
            })),
            profile.id,
            profile.transfer.settings().concurrent_exports,
        )?;
        let mut active = self.download.lock().unwrap();
        if active.is_some() || self.stopped.load(Ordering::SeqCst) {
            return Err(io::Error::other("The query worker is busy or stopped."));
        }
        *active = Some(download.clone());
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        let request = Request {
            profile,
            sql,
            execution,
            expected,
            download: download.clone(),
            guard,
            memory,
            _transport: transport,
            finished: false,
        };
        if self.tx.send(Command::RunExport(Box::new(request))).is_err() {
            *active = None;
            return Err(io::Error::other("The query worker stopped."));
        }
        Ok(download)
    }
}

impl Runner {
    pub(super) fn emit_session(&self, execution: ExecutionId) {
        if let Some(session) = self.session_identity.lock().unwrap().as_ref() {
            self.emit(Event::Session {
                execution,
                generation: session.generation,
            });
        }
    }

    pub(super) fn run_export(&mut self, mut request: Request) {
        let execution = request.execution;
        let result = self.download_execution(&request);
        request.finished = true;
        match result {
            Ok(()) => {}
            Err(error) => {
                let message = crate::connector::error_message(&error);
                if error.is::<SessionChanged>() {
                    request.download.session_changed();
                    self.emit(Event::DownloadFailed {
                        execution,
                        message,
                        consumed: false,
                        disconnected: self.session.is_none(),
                    });
                    *self.download.lock().unwrap() = None;
                    return;
                }
                if error.is::<export::Cancelled>() {
                    request.download.cancel();
                } else {
                    request.download.fail(message.clone());
                }
                let closed = self
                    .session
                    .as_mut()
                    .is_none_or(|session| session.close_operation().is_err());
                let disconnected = !request.download.stopped() || closed;
                if disconnected {
                    self.disconnect();
                }
                self.set_cursor(Cursor::Consumed);
                self.log(
                    Some(execution),
                    Severity::Error,
                    LogKind::Error,
                    message.clone(),
                    self.execution_duration(),
                );
                self.emit(Event::DownloadFailed {
                    execution,
                    message,
                    consumed: true,
                    disconnected,
                });
            }
        }
        *self.target.lock().unwrap() = None;
        *self.download.lock().unwrap() = None;
    }

    fn download_execution(&mut self, request: &Request) -> Result<()> {
        if request.expected.is_some_and(|expected| {
            !matches_session(&self.session_identity, &request.profile, expected)
        }) {
            return Err(SessionChanged.into());
        }
        export::check_cancelled(request.download.cancelled())?;
        self.pending = None;
        self.rows = 0;
        self.bytes = 0;
        self.current_execution = Some(request.execution);
        self.execution = None;
        self.cursor = Cursor::Unavailable;
        let captured = Arc::downgrade(&request.download);
        let control = ConnectionControl::new(
            request.download.cancel_flag(),
            Arc::new(move |transport| {
                captured
                    .upgrade()
                    .context("The export stopped")?
                    .register_transport(transport)?;
                Ok(())
            }),
        );
        self.prepare_session(
            request.profile.clone(),
            request.execution,
            Some(&control),
            request.download.cancel_flag(),
        )?;
        let control = control.with_allowance(request._transport.clone());
        control.check()?;
        self.session
            .as_mut()
            .unwrap()
            .configure_export(&request.profile.transfer)?;
        self.emit(Event::Running);
        let started = Instant::now();
        self.execution = Some(ExecutionTiming {
            id: request.execution,
            started,
            fetch_page: 0,
            execution_completed: false,
        });
        *self.target.lock().unwrap() = None;
        let target = match self
            .session
            .as_mut()
            .unwrap()
            .execute_export_controlled(&request.sql, &control)
        {
            Ok(target) => target,
            Err(error) => {
                if error.is::<QueryError>() {
                    request.download.release_transport();
                }
                return Err(error);
            }
        };
        request
            .download
            .bind(target.clone(), request.guard.fork())?;
        *self.target.lock().unwrap() = Some(target);
        self.emit_session(request.execution);
        self.log(
            Some(request.execution),
            Severity::Info,
            LogKind::ExecutionStarted,
            "SQL accepted by the server",
            None,
        );
        let state = loop {
            export::check_cancelled(request.download.cancelled())?;
            let state = self.session.as_mut().unwrap().poll()?;
            request
                .download
                .set_progress_percentage(self.session.as_ref().unwrap().progress_percentage());
            if request.download.progress_percentage().is_some() {
                self.emit(Event::DownloadProgress {
                    execution: request.execution,
                    rows: 0,
                    bytes: 0,
                    elapsed: started.elapsed(),
                    percentage: request.download.progress_percentage(),
                });
            }
            if state != QueryState::Running {
                break state;
            }
            thread::sleep(Duration::from_millis(100));
        };
        let finished = matches!(state, QueryState::Finished { .. });
        match state {
            QueryState::Cancelled => return Err(export::Cancelled.into()),
            QueryState::Finished { has_results: false }
            | QueryState::Streaming { has_results: false } => {
                export::check_cancelled(request.download.cancelled())?;
                if !finished
                    && self.session.as_mut().unwrap().finish_execution()? == Completion::Cancelled
                {
                    return Err(export::Cancelled.into());
                }
                self.session.as_mut().unwrap().close_operation()?;
                request.download.finish_without_spool(
                    "The statement completed without a result set. No export file was written."
                        .into(),
                )?;
                self.complete_execution(false);
                self.set_cursor(Cursor::Complete);
                self.emit(Event::Ready {
                    more: false,
                    limited: false,
                });
                return Ok(());
            }
            QueryState::Finished { has_results: true }
            | QueryState::Streaming { has_results: true } => {}
            QueryState::Running => anyhow::bail!("The query result is not ready"),
        }
        export::check_cancelled(request.download.cancelled())?;
        let columns = self.session.as_mut().unwrap().columns()?;
        let context = self.session.as_ref().unwrap().export_context();
        let (spool, mut producer) =
            Spool::new_prepared(&columns, &context, request.memory.clone())?;
        request.download.publish_spool(spool.clone())?;
        self.emit(Event::Columns(columns));
        self.emit(Event::ExportContext(context.clone()));
        if finished {
            self.complete_execution(true);
        }
        self.set_cursor(Cursor::Draining);
        let mut count = 0usize;
        let mut preview_rows = 0usize;
        let mut preview_bytes = 0usize;
        let mut preview_full = false;
        let transfer_started = Instant::now();
        let initial_bytes = spool.bytes();
        let result = (|| -> Result<()> {
            loop {
                export::check_cancelled(request.download.cancelled())?;
                let _fetch_memory = producer.reserve_fetch()?;
                let requested = self.session.as_ref().unwrap().export_fetch_rows();
                anyhow::ensure!(
                    (1..=crate::model::transfer::MAX_FETCH_ROWS).contains(&requested),
                    "Invalid export fetch count"
                );
                let mut batch = self.session.as_mut().unwrap().fetch(requested)?;
                request
                    .download
                    .set_progress_percentage(self.session.as_ref().unwrap().progress_percentage());
                export::check_cancelled(request.download.cancelled())?;
                anyhow::ensure!(
                    batch.rows.len() <= requested,
                    "Connector returned more rows than requested"
                );
                if batch.rows.is_empty() {
                    if self.session.as_mut().unwrap().commit_export_rows(0)? {
                        continue;
                    }
                    break;
                }
                producer.append(&batch.rows, request.download.cancelled())?;
                self.session
                    .as_mut()
                    .unwrap()
                    .commit_export_rows(batch.rows.len())?;
                count += batch.rows.len();
                if !preview_full {
                    let mut keep = 0usize;
                    let mut bytes = 512;
                    for row in batch.rows.iter().take(PREVIEW_ROWS - preview_rows) {
                        let row_bytes = budget::row_bytes(row)? + std::mem::size_of::<Row>();
                        if preview_bytes + bytes + row_bytes > MAX_RESULT_BYTES {
                            break;
                        }
                        bytes += row_bytes;
                        keep += 1;
                    }
                    preview_full = keep < batch.rows.len() || preview_rows + keep == PREVIEW_ROWS;
                    if keep > 0 {
                        let allocation = budget::GLOBAL.acquire(bytes)?;
                        let mut rows = Vec::with_capacity(keep);
                        rows.extend(batch.rows.drain(..keep));
                        preview_rows += keep;
                        preview_bytes += bytes;
                        self.emit(Event::PreviewRows {
                            execution: request.execution,
                            rows: export::Rows::accounted(rows, allocation, context.clone()),
                        });
                    }
                }
                self.emit(Event::DownloadProgress {
                    execution: request.execution,
                    rows: count,
                    bytes: spool.bytes(),
                    elapsed: started.elapsed(),
                    percentage: request.download.progress_percentage(),
                });
                drop(batch);
                pace(
                    &request.profile,
                    spool.bytes().saturating_sub(initial_bytes),
                    transfer_started,
                    request.download.cancelled(),
                )?;
            }
            if !finished
                && self.session.as_mut().unwrap().finish_execution()? == Completion::Cancelled
            {
                return Err(export::Cancelled.into());
            }
            export::check_cancelled(request.download.cancelled())?;
            self.complete_execution(true);
            if let Some(warning) = self.session.as_ref().unwrap().export_cleanup_warning() {
                self.log(
                    Some(request.execution),
                    Severity::Info,
                    LogKind::FetchCompleted,
                    warning,
                    None,
                );
            }
            self.session.as_mut().unwrap().close_operation()?;
            Ok(())
        })();
        if let Err(error) = &result {
            if error.is::<export::Cancelled>() {
                spool.cancel();
            } else {
                spool.fail(crate::connector::error_message(error));
            }
        }
        result?;
        request.download.complete(producer)?;
        self.rows = preview_rows;
        self.bytes = preview_bytes;
        self.set_cursor(Cursor::Downloaded);
        self.emit(Event::PreviewComplete {
            execution: request.execution,
            complete: count == preview_rows,
        });
        self.emit(Event::Downloaded {
            execution: request.execution,
            spool,
        });
        Ok(())
    }
}

pub(super) fn pace(
    profile: &Profile,
    bytes: u64,
    started: Instant,
    cancel: &AtomicBool,
) -> Result<()> {
    if profile.database_type != DatabaseType::Kyuubi {
        return Ok(());
    }
    let rate = profile.transfer.settings().speed_limit_mb;
    if rate == 0 {
        return Ok(());
    }
    let desired = Duration::from_secs_f64(bytes as f64 / (f64::from(rate) * 1_000_000.));
    loop {
        export::check_cancelled(cancel)?;
        let remaining = desired.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Ok(());
        }
        thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

#[cfg(test)]
mod pacing_tests {
    use super::{AtomicBool, DatabaseType, Duration, Instant, Profile, pace};
    use crate::model::transfer::{TransferPreset, TransferSettings};

    #[test]
    fn rate_limit_waits_for_transfer_bytes_and_cancel_interrupts_it() {
        let mut profile = Profile {
            database_type: DatabaseType::Kyuubi,
            ..Default::default()
        };
        profile.transfer.preset = TransferPreset::Custom;
        profile.transfer.custom = TransferSettings {
            speed_limit_mb: 1,
            ..Default::default()
        };
        let started = Instant::now();
        pace(&profile, 100_000, started, &AtomicBool::new(false)).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(pace(&profile, 1_000_000, Instant::now(), &AtomicBool::new(true)).is_err());
        profile.database_type = DatabaseType::Postgres;
        let started = Instant::now();
        pace(&profile, 1_000_000_000, started, &AtomicBool::new(false)).unwrap();
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
