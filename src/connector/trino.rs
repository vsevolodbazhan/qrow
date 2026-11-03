//! Trino's HTTP statement protocol and per-tab session state.
mod external;
mod heartbeat;
mod preview;
mod protocol;
mod raw;
mod segment_download;
mod segment_reader;
mod segment_threads;
mod spooling;
mod transport;
#[cfg(test)]
mod transport_tests;

use super::{
    Cancellation, ConnectionControl, Connector, MetadataRequest, QueryError, QueryState, Secret,
    Session,
};
use crate::model::transfer::TrinoSpooling;
use crate::{
    model::{Batch, Column, DatabaseType, Profile, Row},
    tls::Trust,
};
use anyhow::{Context, Result};
use protocol::{Http, SessionHeaders};
use serde::Deserialize;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use url::Url;

#[derive(Default)]
pub struct TrinoConnector {
    trust: Trust,
}

impl TrinoConnector {
    pub fn new(trust: Trust) -> Self {
        Self { trust }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    #[serde(default, deserialize_with = "raw::optional_text::<_, 16384>")]
    next_uri: Option<String>,
    columns: Option<Box<serde_json::value::RawValue>>,
    data: Option<Box<serde_json::value::RawValue>>,
    error: Option<TrinoError>,
    stats: Option<Stats>,
    #[serde(default, deserialize_with = "raw::optional_text::<_, 1024>")]
    update_type: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrinoError {
    #[serde(deserialize_with = "raw::text::<_, 65536>")]
    message: String,
    #[serde(deserialize_with = "raw::text::<_, 1024>")]
    error_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stats {
    progress_percentage: Option<f64>,
}

#[derive(Default)]
struct Cursor {
    next: Option<Url>,
    terminal: bool,
}

struct Cancel {
    http: Arc<Http>,
    next: Mutex<Cursor>,
    requested: Arc<AtomicBool>,
    threads: Arc<segment_threads::Threads>,
    active: Mutex<bool>,
    requests: Arc<transport::Transport>,
    cleanup: Mutex<Cleanup>,
    changed: Condvar,
    heartbeat: heartbeat::Heartbeat,
}

#[derive(Default)]
struct Cleanup {
    deadline: Option<Instant>,
    done: Option<std::result::Result<(), String>>,
}

impl Cancellation for Cancel {
    fn cancel(&self) -> Result<()> {
        self.cancel_with_deadline(Instant::now() + Duration::from_secs(2))
    }

    fn cancel_with_deadline(&self, deadline: Instant) -> Result<()> {
        if !*self.active.lock().unwrap() {
            return Ok(());
        }
        self.requested.store(true, Ordering::SeqCst);
        self.requests.seal();
        self.requests.close();
        self.heartbeat.stop();
        let mut cleanup = self.cleanup.lock().unwrap();
        if let Some(original) = cleanup.deadline {
            while cleanup.done.is_none() {
                let remaining = original
                    .min(deadline)
                    .saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    drop(cleanup);
                    self.abort_transport();
                    anyhow::bail!("Trino cleanup deadline expired");
                }
                cleanup = self.changed.wait_timeout(cleanup, remaining).unwrap().0;
            }
            return cleanup
                .done
                .as_ref()
                .unwrap()
                .clone()
                .map_err(anyhow::Error::msg);
        }
        cleanup.deadline = Some(deadline);
        drop(cleanup);
        let deleted = self.delete_before(deadline);
        let idle = self.requests.wait_idle(deadline);
        let joined = self.threads.join_all(deadline);
        self.http.end_requests(&self.requests);
        let result = deleted.and(idle).and(joined).and_then(|()| {
            anyhow::ensure!(
                !self.http.transport.is_closed(),
                "Trino cleanup closed the session; reconnect"
            );
            Ok(())
        });
        if result.is_err() {
            self.abort_transport();
        }
        self.cleanup.lock().unwrap().done = Some(
            result
                .as_ref()
                .copied()
                .map_err(|error| format!("{error:#}")),
        );
        self.changed.notify_all();
        result
    }

    fn abort_transport(&self) {
        let active = self.active.lock().unwrap();
        if *active {
            self.http.transport.close_all();
        }
    }
}

impl Cancel {
    fn delete_before(&self, deadline: Instant) -> Result<()> {
        if !*self.active.lock().unwrap() {
            return Ok(());
        }
        let (next, terminal) = {
            let cursor = self.next.lock().unwrap();
            (cursor.next.clone(), cursor.terminal)
        };
        if let Some(next) = next {
            self.http.delete(&next, deadline)?;
        } else if !terminal {
            self.abort_transport();
            anyhow::bail!("Trino submission ended before a cancellation URL arrived; reconnect");
        }
        Ok(())
    }
}

struct Operation {
    cancel: Arc<Cancel>,
    columns: Vec<Column>,
    prefix: Option<preview::Prefix>,
    data: Option<raw::Rows>,
    segments: Option<segment_reader::Reader>,
    encoded_started: bool,
    _allowance: Option<Arc<crate::export::budget::Allowance>>,
    pending: Option<Row>,
    exhausted: bool,
    finished: bool,
    cancelled: bool,
    executing: bool,
    progress: Option<f64>,
    announced: bool,
    command: bool,
}

impl Operation {
    fn new(cancel: Arc<Cancel>, direct: bool) -> Result<Self> {
        Ok(Self {
            cancel,
            columns: Vec::new(),
            prefix: if direct {
                None
            } else {
                Some(preview::Prefix::new()?)
            },
            data: None,
            segments: None,
            encoded_started: false,
            _allowance: None,
            pending: None,
            exhausted: false,
            finished: false,
            cancelled: false,
            executing: false,
            progress: None,
            announced: false,
            command: false,
        })
    }

    fn read_page(
        &self,
        next: &Url,
        headers: &SessionHeaders,
    ) -> Result<Option<(Page, reqwest::header::HeaderMap)>> {
        match self
            .cancel
            .http
            .page(reqwest::Method::GET, next, None, headers)
        {
            Err(error)
                if error.is::<crate::export::Cancelled>()
                    && self.cancel.requested.load(Ordering::SeqCst) =>
            {
                Ok(None)
            }
            result => result.map(Some),
        }
    }

    fn accept_response(
        &mut self,
        page: Page,
        response: &reqwest::header::HeaderMap,
        headers: &mut SessionHeaders,
    ) -> Result<()> {
        // page publishes the cursor first. Even an SQL error can finish a
        // transaction or change session state; keep its original SQL cause.
        let result = self.page(page);
        let update = headers.apply(response);
        if update.is_err() {
            self.cancel.abort_transport();
        }
        result.and(update)
    }

    fn page(&mut self, page: Page) -> Result<()> {
        let next = page
            .next_uri
            .as_deref()
            .map(|next| self.cancel.http.cursor(next))
            .transpose()?;
        let terminal = next.is_none();
        *self.cancel.next.lock().unwrap() = Cursor { next, terminal };
        if let Some(error) = page.error {
            if error.error_name == "USER_CANCELED" {
                self.cancelled = true;
                return Ok(());
            }
            return Err(QueryError(format!("{}: {}", error.error_name, error.message)).into());
        }
        if self.cancel.requested.load(Ordering::SeqCst) {
            self.cancelled = true;
            return Ok(());
        }
        self.command |= page.update_type.is_some();
        if self.command {
            anyhow::ensure!(
                self.columns.is_empty(),
                "Trino changed a row result into a command"
            );
        }
        if let Some(columns) = page.columns.filter(|_| !self.command) {
            let columns = raw::columns(&columns)?;
            anyhow::ensure!(
                self.columns.is_empty() || columns == self.columns,
                "Trino changed the result schema during execution"
            );
            self.columns = columns;
        }
        self.progress = page
            .stats
            .and_then(|stats| stats.progress_percentage)
            .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
            .or(self.progress);
        if let Some(data) = page.data.filter(|_| !self.command) {
            if data.get().trim_start().starts_with('{') {
                let reader = self
                    .segments
                    .as_mut()
                    .context("Trino returned unrequested encoded data")?;
                reader.install(spooling::Page::new(data)?)?;
                self.encoded_started = true;
            } else {
                anyhow::ensure!(
                    !self.encoded_started,
                    "Trino changed encoded results to classic rows"
                );
                self.segments = None;
                self.data = Some(raw::Rows::new(data)?);
            }
        }
        self.complete_if_drained();
        Ok(())
    }

    fn retain_prefix(&mut self) -> Result<()> {
        if let Some(prefix) = self.prefix.as_mut() {
            if prefix.limited {
                self.data = None;
            }
            while let Some(data) = self.data.as_mut() {
                if self.cancel.requested.load(Ordering::SeqCst) {
                    self.cancelled = true;
                    return Ok(());
                }
                let Some(row) = data.next(self.columns.len(), false)? else {
                    self.data = None;
                    break;
                };
                prefix.append(row)?;
                if prefix.limited {
                    self.data = None;
                    break;
                }
            }
        }
        self.complete_if_drained();
        Ok(())
    }

    fn complete_if_drained(&mut self) {
        let terminal = self.cancel.next.lock().unwrap().terminal;
        if terminal {
            self.cancel.heartbeat.stop();
        }
        if terminal
            && self.data.is_none()
            && self.pending.is_none()
            && self
                .segments
                .as_ref()
                .is_none_or(|reader| !reader.has_input())
        {
            self.finished = true;
            // Keep cancellation authority until close_operation joins any
            // concurrent cleanup, including a DELETE racing the final response.
        }
    }

    fn advance(&mut self, headers: &mut SessionHeaders) -> Result<()> {
        let next = self
            .cancel
            .next
            .lock()
            .unwrap()
            .next
            .clone()
            .context("Trino query has no result cursor")?;
        let Some((page, response)) = self.read_page(&next, headers)? else {
            self.cancelled = true;
            return Ok(());
        };
        self.executing |= next.path().starts_with("/v1/statement/executing/");
        self.accept_response(page, &response, headers)?;
        if self.executing && !self.finished && !self.cancelled {
            let next = self.cancel.next.lock().unwrap().next.clone();
            if let Some(next) = next {
                self.cancel.heartbeat.update(next, headers);
            }
        }
        self.retain_prefix()?;
        Ok(())
    }

    fn state(&self) -> QueryState {
        if self.cancel.requested.load(Ordering::SeqCst) || self.cancelled {
            QueryState::Cancelled
        } else if self.finished {
            QueryState::Finished {
                has_results: !self.columns.is_empty(),
            }
        } else if !self.columns.is_empty() {
            QueryState::Streaming { has_results: true }
        } else {
            QueryState::Running
        }
    }

    fn poll(&mut self, headers: &mut SessionHeaders) -> Result<QueryState> {
        if matches!(
            self.state(),
            QueryState::Cancelled | QueryState::Finished { .. }
        ) {
            return Ok(self.state());
        }
        if !self.announced && matches!(self.state(), QueryState::Streaming { .. }) {
            self.announced = true;
            return Ok(self.state());
        }
        // Legacy completion callers still advance a normal preview. Export
        // paging belongs to fetch, so a page is never replaced before its rows.
        if self.data.is_none()
            && self
                .segments
                .as_ref()
                .is_none_or(|reader| !reader.has_input())
        {
            self.advance(headers)?;
        }
        self.announced |= matches!(self.state(), QueryState::Streaming { .. });
        Ok(self.state())
    }

    fn next_direct(&mut self, headers: &mut SessionHeaders) -> Result<Option<Row>> {
        loop {
            if self.cancel.requested.load(Ordering::SeqCst) || self.cancelled {
                return Err(crate::export::Cancelled.into());
            }
            if let Some(row) = self.pending.take() {
                return Ok(Some(row));
            }
            if let Some(data) = self.data.as_mut() {
                if let Some(row) = data.next(self.columns.len(), true)? {
                    return Ok(Some(row));
                }
                self.data = None;
                self.complete_if_drained();
            }
            if self.finished {
                return Ok(None);
            }
            self.advance(headers)?;
        }
    }

    fn fetch_segments(&mut self, count: usize, headers: &mut SessionHeaders) -> Result<Batch> {
        self.segments.as_ref().unwrap().ready_to_fetch()?;
        while self
            .segments
            .as_ref()
            .is_some_and(segment_reader::Reader::has_space)
        {
            anyhow::ensure!(!self.cancelled, crate::export::Cancelled);
            self.segments.as_ref().unwrap().ready_to_fetch()?;
            if self.segments.as_mut().unwrap().admit()? {
                continue;
            }
            if self.cancel.next.lock().unwrap().terminal {
                break;
            }
            self.advance(headers)?;
        }
        if self.segments.is_none() {
            return self.fetch(count, headers);
        }
        let batch = self
            .segments
            .as_mut()
            .unwrap()
            .fetch(count, self.columns.len())?;
        self.exhausted = batch.rows.is_empty() && !self.segments.as_ref().unwrap().has_input();
        Ok(batch)
    }

    fn commit_export_rows(&mut self, rows: usize) -> Result<bool> {
        let more = self
            .segments
            .as_mut()
            .map(|reader| reader.commit(rows))
            .transpose()?
            .unwrap_or(false);
        self.complete_if_drained();
        Ok(more)
    }

    fn fetch(&mut self, count: usize, headers: &mut SessionHeaders) -> Result<Batch> {
        if self.segments.is_some() && count > 0 {
            return self.fetch_segments(count, headers);
        }
        let count = count.min(1000);
        if count == 0 {
            return Ok(Batch { rows: Vec::new() });
        }
        let mut rows = Vec::with_capacity(count);
        let mut bytes = rows.capacity() * std::mem::size_of::<Row>();
        for _ in 0..count {
            if self.prefix.is_some() && matches!(self.state(), QueryState::Cancelled) {
                rows.clear();
                break;
            }
            let row = if self.prefix.is_some() {
                while self.pending.is_none()
                    && self.prefix.as_ref().unwrap().available() == 0
                    && !self.finished
                    && !self.prefix.as_ref().unwrap().limited
                {
                    if matches!(self.state(), QueryState::Cancelled) {
                        return Ok(Batch { rows: Vec::new() });
                    }
                    self.advance(headers)?;
                }
                if let Some(row) = self.pending.take() {
                    Some(row)
                } else {
                    self.prefix.as_mut().unwrap().next(self.columns.len())?
                }
            } else {
                self.next_direct(headers)?
            };
            let Some(row) = row else {
                break;
            };
            let owned = if self.prefix.is_some() {
                row.capacity() * std::mem::size_of::<Option<String>>()
                    + row.iter().flatten().map(String::capacity).sum::<usize>()
            } else {
                crate::export::budget::row_bytes(&row)?
            };
            if !rows.is_empty() && bytes.saturating_add(owned) > 48 * crate::export::budget::MIB {
                self.pending = Some(row);
                break;
            }
            bytes += owned;
            rows.push(row);
        }
        self.exhausted = rows.is_empty();
        Ok(Batch { rows })
    }

    fn finish(&mut self, headers: &mut SessionHeaders) -> Result<super::Completion> {
        if matches!(self.state(), QueryState::Cancelled) {
            return Ok(super::Completion::Cancelled);
        }
        while !self.finished {
            if self.cancel.requested.load(Ordering::SeqCst) || self.cancelled {
                return Ok(super::Completion::Cancelled);
            }
            // A normal preview retains only its bounded prefix. Direct callers
            // must read all rows through fetch before asking for completion.
            anyhow::ensure!(
                self.prefix.is_some()
                    || (self.data.is_none()
                        && self
                            .segments
                            .as_ref()
                            .is_none_or(|reader| !reader.has_input())),
                "Trino export still has unread rows"
            );
            self.advance(headers)?;
        }
        Ok(super::Completion::Finished {
            has_results: !self.columns.is_empty(),
        })
    }
}

struct TrinoSession {
    http: Arc<Http>,
    headers: SessionHeaders,
    catalog: String,
    operation: Option<Operation>,
    keep_alive: Option<Operation>,
    export_mode: TrinoSpooling,
}

impl Connector for TrinoConnector {
    fn connect(&self, profile: &Profile, secret: Secret) -> Result<Box<dyn Session>> {
        self.connect_controlled(profile, secret, &ConnectionControl::default())
    }

    fn connect_controlled(
        &self,
        profile: &Profile,
        secret: Secret,
        control: &ConnectionControl,
    ) -> Result<Box<dyn Session>> {
        control.check()?;
        profile.validate()?;
        let http = Arc::new(Http::new(profile, secret, &self.trust)?);
        control.register(http.transport.clone())?;
        let mut session = TrinoSession {
            http,
            headers: SessionHeaders::new(profile)?,
            catalog: profile.database.clone(),
            operation: None,
            keep_alive: None,
            export_mode: TrinoSpooling::Off,
        };
        // The protocol has no separate login. Probe through the statement endpoint
        // to check authentication and session properties before returning a session.
        session.execute("SELECT 1")?;
        super::wait_for_completion(
            &mut session,
            Some(
                std::time::Instant::now()
                    + std::time::Duration::from_secs(profile.lifecycle.response_timeout_seconds),
            ),
        )?;
        session.close_operation()?;
        control.check()?;
        Ok(Box::new(session))
    }
}

impl TrinoSession {
    fn start(&mut self, sql: &str, direct: bool) -> Result<Operation> {
        self.start_controlled(sql, direct, &ConnectionControl::default())
    }

    fn start_controlled(
        &mut self,
        sql: &str,
        direct: bool,
        control: &ConnectionControl,
    ) -> Result<Operation> {
        control.check()?;
        anyhow::ensure!(
            !direct || sql.len() <= 1024 * 1024,
            "Trino export SQL exceeds 1 MiB"
        );
        crate::sql::validate_single_for(sql, DatabaseType::Trino)?;
        let range = crate::sql::statement_ranges_for(sql, DatabaseType::Trino)
            .into_iter()
            .next()
            .context("Enter a Trino statement")?;
        let sql = &sql[range];
        let sql = sql.strip_suffix(';').unwrap_or(sql);
        let enabled = direct && self.export_mode != TrinoSpooling::Off;
        let allowance = if enabled {
            let allowance = match control.allowance() {
                Some(allowance) => allowance,
                None => {
                    crate::export::budget::GLOBAL.allowance(512 * crate::export::budget::MIB)?
                }
            };
            anyhow::ensure!(
                allowance.capacity() >= 512 * crate::export::budget::MIB,
                "Trino spooling requires 512 MiB transport headroom"
            );
            Some(allowance)
        } else {
            control.allowance()
        };
        let heartbeat = heartbeat::Heartbeat::new(self.http.clone())?;
        let cancel = Arc::new(Cancel {
            http: self.http.clone(),
            next: Mutex::new(Cursor::default()),
            requested: Arc::new(AtomicBool::new(false)),
            threads: Arc::default(),
            active: Mutex::new(true),
            requests: self.http.begin_requests(allowance.clone())?,
            cleanup: Mutex::default(),
            changed: Condvar::new(),
            heartbeat,
        });
        let mut operation = match Operation::new(cancel.clone(), direct) {
            Ok(operation) => operation,
            Err(error) => {
                cancel.requests.close();
                self.http.end_requests(&cancel.requests);
                *cancel.active.lock().unwrap() = false;
                return Err(error);
            }
        };
        operation._allowance = allowance.clone();
        if enabled {
            let allowance = allowance.expect("enabled spooling allowance");
            let segment_control =
                self.http
                    .segment_control(&operation.cancel, allowance, control.cancel_flag());
            operation.segments = Some(segment_reader::Reader::new(
                self.export_mode,
                segment_control,
            ));
        }
        if let Err(error) = control.register(operation.cancel.clone()) {
            operation.cancel.requests.close();
            self.http.end_requests(&operation.cancel.requests);
            *operation.cancel.active.lock().unwrap() = false;
            return Err(error);
        }
        let (page, response) = match self.http.page_encoded(
            reqwest::Method::POST,
            &self.http.statement,
            Some(sql),
            &self.headers,
            enabled,
        ) {
            Ok(response) => response,
            Err(error) => {
                operation.cancel.abort_transport();
                self.http.end_requests(&operation.cancel.requests);
                *operation.cancel.active.lock().unwrap() = false;
                return Err(error);
            }
        };
        let result = operation
            .accept_response(page, &response, &mut self.headers)
            .and_then(|()| operation.retain_prefix());
        if result.is_err() {
            let _ = operation.cancel.cancel();
            *operation.cancel.active.lock().unwrap() = false;
        }
        result?;
        Ok(operation)
    }

    fn stop(operation: &mut Option<Operation>) -> Result<()> {
        if let Some(operation) = operation.take() {
            let result = operation.cancel.cancel();
            *operation.cancel.active.lock().unwrap() = false;
            result?;
        }
        Ok(())
    }
}

impl Session for TrinoSession {
    fn execute(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        anyhow::ensure!(
            !self.http.transport.is_closed(),
            "Trino session is closed; reconnect"
        );
        self.close_keep_alive()?;
        self.close_operation()?;
        let operation = self.start(sql, false)?;
        let cancel = operation.cancel.clone();
        self.operation = Some(operation);
        Ok(cancel)
    }
    fn configure_export(&mut self, transfer: &crate::model::transfer::Transfer) -> Result<()> {
        transfer.validate()?;
        self.export_mode = transfer.settings().trino_spooling;
        Ok(())
    }
    fn execute_export(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.execute_export_controlled(sql, &ConnectionControl::default())
    }
    fn execute_export_controlled(
        &mut self,
        sql: &str,
        control: &ConnectionControl,
    ) -> Result<Arc<dyn Cancellation>> {
        anyhow::ensure!(
            !self.http.transport.is_closed(),
            "Trino session is closed; reconnect"
        );
        self.close_keep_alive()?;
        self.close_operation()?;
        let operation = self.start_controlled(sql, true, control)?;
        let cancel = operation.cancel.clone();
        self.operation = Some(operation);
        Ok(cancel)
    }
    fn finish_execution(&mut self) -> Result<super::Completion> {
        self.operation
            .as_mut()
            .context("No Trino operation")?
            .finish(&mut self.headers)
    }
    fn progress_percentage(&self) -> Option<f64> {
        self.operation
            .as_ref()
            .and_then(|operation| operation.progress)
    }
    fn execute_metadata(&mut self, request: &MetadataRequest) -> Result<Arc<dyn Cancellation>> {
        self.execute(&metadata_sql(&self.catalog, request))
    }
    fn poll(&mut self) -> Result<QueryState> {
        self.keep_alive
            .as_mut()
            .or(self.operation.as_mut())
            .context("No Trino operation")?
            .poll(&mut self.headers)
    }
    fn columns(&mut self) -> Result<Vec<Column>> {
        Ok(self
            .operation
            .as_ref()
            .context("No Trino operation")?
            .columns
            .clone())
    }
    fn fetch(&mut self, count: usize) -> Result<Batch> {
        self.operation
            .as_mut()
            .context("No Trino operation")?
            .fetch(count, &mut self.headers)
    }
    fn commit_export_rows(&mut self, rows: usize) -> Result<bool> {
        self.operation
            .as_mut()
            .context("No Trino operation")?
            .commit_export_rows(rows)
    }
    fn export_cleanup_warning(&self) -> Option<String> {
        let count = self.operation.as_ref()?.segments.as_ref()?.ack_failures;
        (count > 0).then(|| format!("Trino could not acknowledge {count} committed segments. Storage cleanup will use their expiry."))
    }
    fn transport_cancellation(&self) -> Option<Arc<dyn Cancellation>> {
        Some(self.http.transport.clone())
    }
    fn result_limited(&self) -> bool {
        self.operation.as_ref().is_some_and(|operation| {
            operation
                .prefix
                .as_ref()
                .is_some_and(|prefix| prefix.limited)
                && operation.exhausted
        })
    }
    fn close_operation(&mut self) -> Result<()> {
        Self::stop(&mut self.operation)
    }
    fn execute_keep_alive(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        anyhow::ensure!(
            self.operation
                .as_ref()
                .is_none_or(|operation| operation.finished),
            "Trino cannot run keepalive during a user statement"
        );
        self.close_keep_alive()?;
        let operation = self.start(sql, false)?;
        let cancel = operation.cancel.clone();
        self.keep_alive = Some(operation);
        Ok(cancel)
    }
    fn close_keep_alive(&mut self) -> Result<()> {
        Self::stop(&mut self.keep_alive)
    }
    fn close(&mut self) -> Result<()> {
        let result = (|| -> Result<()> {
            self.http.disable_authentication();
            self.close_keep_alive()?;
            self.close_operation()?;
            if self.headers.in_transaction() {
                self.execute_keep_alive("ROLLBACK")?;
                super::wait_for_completion(
                    self,
                    Some(std::time::Instant::now() + self.http.timeout),
                )?;
                self.close_keep_alive()?;
            }
            Ok(())
        })();
        self.http.transport.close_all();
        result
    }
}

impl Drop for TrinoSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn metadata_sql(catalog: &str, request: &MetadataRequest) -> String {
    let catalog = DatabaseType::Trino.quote_identifier(catalog);
    let literal = |text: &str| format!("'{}'", text.replace('\'', "''"));
    let filter = |schema: &str, relation: &Option<String>| {
        let mut filter = format!("table_schema = {}", literal(schema));
        if let Some(relation) = relation {
            filter.push_str(&format!(" AND table_name = {}", literal(relation)));
        }
        filter
    };
    match request {
        MetadataRequest::Schemas => format!(
            "SELECT schema_name AS \"TABLE_SCHEM\" FROM {catalog}.information_schema.schemata WHERE schema_name <> 'information_schema' ORDER BY schema_name"
        ),
        MetadataRequest::Relations { schema, relation } => format!(
            "SELECT table_schema AS \"TABLE_SCHEM\", table_name AS \"TABLE_NAME\", CASE WHEN table_type = 'VIEW' THEN 'VIEW' ELSE 'TABLE' END AS \"TABLE_TYPE\", CAST(NULL AS varchar) AS \"REMARKS\" FROM {catalog}.information_schema.tables WHERE {} ORDER BY table_name",
            filter(schema, relation)
        ),
        MetadataRequest::Columns { schema, relation } => format!(
            "SELECT table_schema AS \"TABLE_SCHEM\", table_name AS \"TABLE_NAME\", column_name AS \"COLUMN_NAME\", data_type AS \"TYPE_NAME\", CAST(NULL AS varchar) AS \"REMARKS\", ordinal_position AS \"ORDINAL_POSITION\" FROM {catalog}.information_schema.columns WHERE {} ORDER BY table_name, ordinal_position",
            filter(schema, relation)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_quotes_catalogs_and_exact_object_names() {
        let sql = metadata_sql(
            "odd\"catalog",
            &MetadataRequest::Columns {
                schema: "a'b".into(),
                relation: Some("t'x".into()),
            },
        );
        assert!(sql.contains("\"odd\"\"catalog\".information_schema.columns"));
        assert!(sql.contains("table_schema = 'a''b' AND table_name = 't''x'"));
        assert!(
            metadata_sql("tpch", &MetadataRequest::Schemas).contains("information_schema.schemata")
        );
    }
}
