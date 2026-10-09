//! Trino's HTTP statement protocol and per-tab session state.
mod external;
mod protocol;
mod transport;
#[cfg(test)]
mod transport_tests;

use super::{
    Cancellation, ConnectionControl, Connector, MetadataRequest, QueryError, QueryState, Secret,
    Session,
};
use crate::{
    model::{Batch, Column, DatabaseType, MAX_RESULT_BYTES, MAX_RESULT_ROWS, Profile, Row},
    tls::Trust,
};
use anyhow::{Context, Result};
use protocol::{Http, SessionHeaders};
use serde::Deserialize;
use std::time::{Duration, Instant};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Seek, Write},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
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
    next_uri: Option<String>,
    columns: Option<Vec<TrinoColumn>>,
    data: Option<Vec<Vec<serde_json::Value>>>,
    error: Option<TrinoError>,
}

#[derive(Deserialize)]
struct TrinoColumn {
    name: String,
    #[serde(rename = "type")]
    data_type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrinoError {
    message: String,
    error_name: String,
}

#[derive(Default)]
struct Cursor {
    next: Option<Url>,
    terminal: bool,
}

struct Cancel {
    http: Arc<Http>,
    next: Mutex<Cursor>,
    requested: AtomicBool,
    active: Mutex<bool>,
    requests: Arc<transport::Transport>,
    cleanup: Mutex<Cleanup>,
    changed: Condvar,
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
        let result = self
            .delete_before(deadline)
            .and_then(|()| self.requests.wait_idle(deadline))
            .and_then(|()| {
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
    writer: Option<BufWriter<File>>,
    reader: Option<BufReader<File>>,
    rows: usize,
    bytes: usize,
    remaining: usize,
    exhausted: bool,
    limited: bool,
    finished: bool,
    cancelled: bool,
}

impl Operation {
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

    fn page(&mut self, page: Page) -> Result<()> {
        // Publish the new cursor before inspecting the result, so every failure
        // and a cancellation racing the request can still close the server query.
        let next = page
            .next_uri
            .as_deref()
            .map(|next| self.cancel.http.cursor(next))
            .transpose()?;
        let terminal = next.is_none();
        *self.cancel.next.lock().unwrap() = Cursor { next, terminal };
        if self.cancel.requested.load(Ordering::SeqCst) {
            self.cancelled = true;
            return Ok(());
        }
        if let Some(error) = page.error {
            if error.error_name == "USER_CANCELED" {
                self.cancelled = true;
                return Ok(());
            }
            return Err(QueryError(format!("{}: {}", error.error_name, error.message)).into());
        }
        if let Some(columns) = page.columns {
            self.columns = columns
                .into_iter()
                .map(|column| Column {
                    name: column.name,
                    data_type: column.data_type,
                })
                .collect();
        }
        let file = self
            .writer
            .as_mut()
            .context("Trino results are already complete")?;
        for values in page.data.unwrap_or_default() {
            if self.cancel.requested.load(Ordering::SeqCst) {
                self.cancelled = true;
                return Ok(());
            }
            anyhow::ensure!(
                values.len() == self.columns.len(),
                "Trino returned a row with the wrong number of columns"
            );
            let row: Row = values
                .into_iter()
                .map(|value| match value {
                    serde_json::Value::Null => None,
                    serde_json::Value::String(text) => Some(text),
                    value => Some(value.to_string()),
                })
                .collect();
            let bytes = row
                .len()
                .saturating_mul(std::mem::size_of::<Option<String>>())
                .saturating_add(row.iter().flatten().map(String::len).sum::<usize>());
            if self.rows >= MAX_RESULT_ROWS || self.bytes.saturating_add(bytes) > MAX_RESULT_BYTES {
                self.limited = true;
            }
            if !self.limited {
                file.write_all(&(row.len() as u64).to_le_bytes())?;
                for value in row {
                    match value {
                        None => file.write_all(&u64::MAX.to_le_bytes())?,
                        Some(value) => {
                            file.write_all(&(value.len() as u64).to_le_bytes())?;
                            file.write_all(value.as_bytes())?;
                        }
                    }
                }
                self.rows += 1;
                self.bytes += bytes;
            }
        }
        if self.cancel.next.lock().unwrap().next.is_none() {
            let mut file = self
                .writer
                .take()
                .context("No Trino result writer")?
                .into_inner()
                .map_err(|error| error.into_error())?;
            file.rewind()?;
            self.reader = Some(BufReader::new(file));
            self.finished = true;
            *self.cancel.active.lock().unwrap() = false;
            self.remaining = self.rows;
        }
        Ok(())
    }

    fn poll(&mut self, headers: &mut SessionHeaders) -> Result<QueryState> {
        if self.cancel.requested.load(Ordering::SeqCst) || self.cancelled {
            return Ok(QueryState::Cancelled);
        }
        if !self.finished {
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
                return Ok(QueryState::Cancelled);
            };
            headers.apply(&response)?;
            self.page(page)?;
        }
        if self.cancelled {
            Ok(QueryState::Cancelled)
        } else if self.finished {
            Ok(QueryState::Finished {
                has_results: !self.columns.is_empty(),
            })
        } else {
            Ok(QueryState::Running)
        }
    }

    fn fetch(&mut self, count: usize) -> Result<Batch> {
        anyhow::ensure!(self.finished, "Trino query is still running");
        let file = self.reader.as_mut().context("No Trino result reader")?;
        let mut rows = Vec::new();
        for _ in 0..count.min(self.remaining) {
            let mut buffer = [0; 8];
            file.read_exact(&mut buffer)?;
            let width = u64::from_le_bytes(buffer) as usize;
            let mut row = Vec::with_capacity(width);
            for _ in 0..width {
                file.read_exact(&mut buffer)?;
                let length = u64::from_le_bytes(buffer);
                if length == u64::MAX {
                    row.push(None);
                } else {
                    let mut bytes = vec![0; length as usize];
                    file.read_exact(&mut bytes)?;
                    row.push(Some(String::from_utf8(bytes)?));
                }
            }
            rows.push(row);
            self.remaining -= 1;
        }
        self.exhausted = rows.is_empty();
        Ok(Batch { rows })
    }
}

struct TrinoSession {
    http: Arc<Http>,
    headers: SessionHeaders,
    catalog: String,
    operation: Option<Operation>,
    keep_alive: Option<Operation>,
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
    fn start(&mut self, sql: &str) -> Result<Operation> {
        crate::sql::validate_single_for(sql, DatabaseType::Trino)?;
        let range = crate::sql::statement_ranges_for(sql, DatabaseType::Trino)
            .into_iter()
            .next()
            .context("Enter a Trino statement")?;
        let sql = &sql[range];
        let sql = sql.strip_suffix(';').unwrap_or(sql);
        let cancel = Arc::new(Cancel {
            http: self.http.clone(),
            next: Mutex::new(Cursor::default()),
            requested: AtomicBool::new(false),
            active: Mutex::new(true),
            requests: self.http.begin_requests(),
            cleanup: Mutex::default(),
            changed: Condvar::new(),
        });
        let mut operation = Operation {
            cancel,
            columns: Vec::new(),
            writer: Some(BufWriter::new(tempfile::tempfile()?)),
            reader: None,
            rows: 0,
            bytes: 0,
            remaining: 0,
            exhausted: false,
            limited: false,
            finished: false,
            cancelled: false,
        };
        let (page, response) = match self.http.page(
            reqwest::Method::POST,
            &self.http.statement,
            Some(sql),
            &self.headers,
        ) {
            Ok(response) => response,
            Err(error) => {
                operation.cancel.abort_transport();
                *operation.cancel.active.lock().unwrap() = false;
                return Err(error);
            }
        };
        let result = operation
            .page(page)
            .and_then(|()| self.headers.apply(&response));
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
        let operation = self.start(sql)?;
        let cancel = operation.cancel.clone();
        self.operation = Some(operation);
        Ok(cancel)
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
            .fetch(count)
    }
    fn transport_cancellation(&self) -> Option<Arc<dyn Cancellation>> {
        Some(self.http.transport.clone())
    }
    fn result_limited(&self) -> bool {
        self.operation
            .as_ref()
            .is_some_and(|operation| operation.limited && operation.exhausted)
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
        let operation = self.start(sql)?;
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
    use serde_json::json;
    #[test]
    fn result_byte_limits_stop_retention_and_remain_visible_after_fetch() -> Result<()> {
        let profile = Profile {
            database_type: DatabaseType::Trino,
            host: "localhost".into(),
            username: "qrow".into(),
            database: "tpch".into(),
            ..Profile::default()
        };
        let http = Arc::new(Http::new(
            &profile,
            Secret::password(""),
            &Trust::default(),
        )?);
        let mut operation = Operation {
            cancel: Arc::new(Cancel {
                requests: http.begin_requests(),
                http,
                next: Mutex::new(Cursor::default()),
                requested: AtomicBool::new(false),
                active: Mutex::new(true),
                cleanup: Mutex::default(),
                changed: Condvar::new(),
            }),
            columns: Vec::new(),
            writer: Some(BufWriter::new(tempfile::tempfile()?)),
            reader: None,
            rows: 0,
            bytes: MAX_RESULT_BYTES - 100,
            remaining: 0,
            exhausted: false,
            limited: false,
            finished: false,
            cancelled: false,
        };
        operation.page(serde_json::from_value(json!({"columns":[{"name":"value","type":"varchar"}],"data":[["small"],["x".repeat(100)],["later"]]}))?)?;
        assert!(operation.limited);
        assert_eq!(operation.rows, 1);
        assert_eq!(operation.fetch(10)?.rows, vec![vec![Some("small".into())]]);
        assert!(operation.fetch(1)?.rows.is_empty());
        assert_eq!(operation.remaining, 0);
        Ok(())
    }
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
