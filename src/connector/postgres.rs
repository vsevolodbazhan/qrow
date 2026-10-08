//! Postgres sessions. Values use the server's text representation.
mod tls;

use super::{Cancellation, Connector, MetadataRequest, QueryError, QueryState, Secret, Session};
use crate::{
    model::{
        Authentication, Batch, Column, MAX_RESULT_BYTES, MAX_RESULT_ROWS, PostgresSslMode, Profile,
        Row,
    },
    tls::Trust,
};
use anyhow::{Context, Result};
use futures_util::{StreamExt, pin_mut};
use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Seek, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use tokio::runtime::Runtime;
use tokio_postgres::{Client, Config, NoTls, SimpleQueryMessage, config::SslMode};
use tokio_postgres_rustls::MakeRustlsConnect;

#[derive(Default)]
pub struct PostgresConnector {
    trust: Trust,
}

impl PostgresConnector {
    pub fn new(trust: Trust) -> Self {
        Self { trust }
    }
}

struct Cancel {
    requested: AtomicBool,
    finished: AtomicBool,
    gate: Arc<tokio::sync::Mutex<()>>,
    token: tokio_postgres::CancelToken,
    closed: Arc<AtomicBool>,
    tls: Option<MakeRustlsConnect>,
    timeout: Duration,
}

impl Cancel {
    async fn send(&self) -> Result<()> {
        let _guard = self.gate.lock().await;
        if self.finished.load(Ordering::SeqCst) || self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        tokio::time::timeout(self.timeout, async {
            match &self.tls {
                Some(tls) => self.token.cancel_query(tls.clone()).await,
                None => self.token.cancel_query(NoTls).await,
            }
        })
        .await
        .context("Postgres did not answer the cancellation request in time")?
        .context("Could not cancel the Postgres query")
    }

    async fn until<T>(
        &self,
        future: impl std::future::Future<Output = std::result::Result<T, tokio_postgres::Error>>,
    ) -> Result<T> {
        self.check()?;
        pin_mut!(future);
        loop {
            tokio::select! {
                result = &mut future => return result.map_err(query_error),
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if self.requested.load(Ordering::SeqCst) { self.send().await?; }
                }
            }
        }
    }

    fn check(&self) -> Result<()> {
        if self.requested.load(Ordering::SeqCst) {
            return Err(QueryError("57014: Query cancelled".into()).into());
        }
        Ok(())
    }
}

impl Cancellation for Cancel {
    fn cancel(&self) -> Result<()> {
        if self.finished.load(Ordering::SeqCst) || self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.requested.store(true, Ordering::SeqCst);
        // Cancellation opens its own transport and can outlive the session.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(self.send())
    }
}

struct Results {
    columns: Vec<Column>,
    limited: bool,
    exhausted: bool,
    rows: BufReader<File>,
}

struct Operation {
    cancel: Arc<Cancel>,
    task: tokio::task::JoinHandle<()>,
    receiver: mpsc::Receiver<Result<Results>>,
    results: Option<Results>,
    cancelled: bool,
}

struct PostgresSession {
    client: Arc<Client>,
    connection: tokio::task::JoinHandle<()>,
    runtime: Runtime,
    cancel: Arc<Cancel>,
    operation: Option<Operation>,
    keep_alive: Option<Operation>,
}

fn query_error(error: tokio_postgres::Error) -> anyhow::Error {
    if let Some(database) = error.as_db_error() {
        QueryError(format!(
            "{}: {}",
            database.code().code(),
            database.message()
        ))
        .into()
    } else {
        error.into()
    }
}

impl Connector for PostgresConnector {
    fn connect(&self, profile: &Profile, secret: Secret) -> Result<Box<dyn Session>> {
        anyhow::ensure!(
            profile.authentication == Authentication::Password,
            "Postgres connections use password authentication."
        );
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let timeout = Duration::from_secs(profile.lifecycle.response_timeout_seconds);
        let ssl_mode = profile.postgres_ssl_mode();
        let mut config = Config::new();
        config
            .host(&profile.host)
            .port(profile.port)
            .user(&profile.username)
            .dbname(&profile.database)
            .password(secret.value()?.as_str())
            .connect_timeout(timeout)
            .application_name("Qrow")
            .ssl_mode(if ssl_mode != PostgresSslMode::Disable {
                SslMode::Require
            } else {
                SslMode::Disable
            });
        let tls = match ssl_mode {
            PostgresSslMode::Disable => None,
            PostgresSslMode::Require => Some(MakeRustlsConnect::new(tls::encryption_only()?)),
            PostgresSslMode::VerifyFull => Some(MakeRustlsConnect::new(
                (*self.trust.client_config()?).clone(),
            )),
        };
        let (client, connection) = runtime.block_on(async {
            tokio::time::timeout(timeout, async {
                match &tls {
                    Some(tls) => {
                        let (client, connection) = config.connect(tls.clone()).await?;
                        Ok::<_, tokio_postgres::Error>((
                            client,
                            tokio::spawn(async move {
                                let _ = connection.await;
                            }),
                        ))
                    }
                    None => {
                        let (client, connection) = config.connect(NoTls).await?;
                        Ok((
                            client,
                            tokio::spawn(async move {
                                let _ = connection.await;
                            }),
                        ))
                    }
                }
            })
            .await
            .context("Postgres did not answer the connection request in time")?
            .with_context(|| {
                format!(
                    "Could not connect to Postgres database \"{}\"",
                    profile.database
                )
            })
        })?;
        // Parameters remain separate from SQL, including names and values with quotes.
        runtime.block_on(async {
            for (name, value) in &profile.parameters {
                tokio::time::timeout(
                    timeout,
                    client.query(
                        "SELECT pg_catalog.set_config($1, $2, false)",
                        &[name, value],
                    ),
                )
                .await
                .context("Postgres did not answer the session settings request in time")?
                .with_context(|| format!("Could not set Postgres session parameter \"{name}\""))?;
            }
            Ok::<_, anyhow::Error>(())
        })?;
        let cancel = Arc::new(Cancel {
            requested: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            gate: Arc::new(tokio::sync::Mutex::new(())),
            token: client.cancel_token(),
            closed: Arc::new(AtomicBool::new(false)),
            tls,
            timeout,
        });
        Ok(Box::new(PostgresSession {
            client: Arc::new(client),
            connection,
            runtime,
            cancel,
            operation: None,
            keep_alive: None,
        }))
    }
}

impl PostgresSession {
    fn start(&self, sql: &str) -> Operation {
        let (sender, receiver) = mpsc::channel();
        let client = self.client.clone();
        let sql = sql.to_owned();
        let cancel = Arc::new(Cancel {
            requested: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            gate: self.cancel.gate.clone(),
            token: self.cancel.token.clone(),
            closed: self.cancel.closed.clone(),
            tls: self.cancel.tls.clone(),
            timeout: self.cancel.timeout,
        });
        let requested = cancel.clone();
        let task = self.runtime.spawn(async move {
            let result = async {
                let mut writer = BufWriter::new(tempfile::tempfile()?);
                { let _guard = requested.gate.lock().await; requested.check()?; }
                let statement = requested.until(client.prepare(&sql)).await?;
                let types: Vec<_> = statement
                    .columns()
                    .iter()
                    .map(|column| column.type_().name().to_owned())
                    .collect();
                requested.check()?;
                let stream = requested.until(client.simple_query_raw(&sql)).await?;
                pin_mut!(stream);
                let mut columns = Vec::new();
                let mut rows = 0;
                let mut bytes: usize = 0;
                let mut limited = false;
                let mut cancellation_tick = tokio::time::interval(Duration::from_millis(100));
                loop {
                    let message = tokio::select! {
                        message = stream.next() => message,
                        _ = cancellation_tick.tick() => {
                            if requested.requested.load(Ordering::SeqCst) { requested.send().await?; }
                            continue;
                        }
                    };
                    let Some(message) = message else { break; };
                    match message.map_err(query_error)? {
                        SimpleQueryMessage::RowDescription(description) => {
                            columns = description
                                .iter()
                                .enumerate()
                                .map(|(index, column)| Column {
                                    name: column.name().into(),
                                    data_type: types
                                        .get(index)
                                        .cloned()
                                        .unwrap_or_else(|| "text".into()),
                                })
                                .collect();
                        }
                        SimpleQueryMessage::Row(row) => {
                            let row_bytes = row.len().saturating_mul(std::mem::size_of::<Option<String>>())
                                .saturating_add((0..row.len()).filter_map(|i| row.get(i)).map(str::len).sum::<usize>());
                            if rows >= MAX_RESULT_ROWS || bytes.saturating_add(row_bytes) > MAX_RESULT_BYTES {
                                limited = true;
                            }
                            // Inspect borrowed values before retaining them. Binary lengths
                            // keep JSON escaping from expanding the temporary file.
                            if !limited {
                                writer.write_all(&(row.len() as u64).to_le_bytes())?;
                                for index in 0..row.len() {
                                    match row.get(index) {
                                        None => writer.write_all(&u64::MAX.to_le_bytes())?,
                                        Some(value) => {
                                            writer.write_all(&(value.len() as u64).to_le_bytes())?;
                                            writer.write_all(value.as_bytes())?;
                                        }
                                    }
                                }
                                bytes += row_bytes;
                                rows += 1;
                            }
                        }
                        _ => {}
                    }
                }
                writer.flush()?;
                let mut file = writer.into_inner()?;
                file.rewind()?;
                Ok::<_, anyhow::Error>(Results {
                    columns,
                    limited,
                    exhausted: false,
                    rows: BufReader::new(file),
                })
            }
            .await;
            requested.finished.store(true, Ordering::SeqCst);
            let _ = sender.send(result);
        });
        Operation {
            cancel,
            task,
            receiver,
            results: None,
            cancelled: false,
        }
    }
}

impl Session for PostgresSession {
    fn execute(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.close_operation()?;
        crate::sql::validate_single_for(sql, crate::model::DatabaseType::Postgres)?;
        let operation = self.start(sql);
        let cancel = operation.cancel.clone();
        self.operation = Some(operation);
        Ok(cancel)
    }

    fn execute_metadata(&mut self, request: &MetadataRequest) -> Result<Arc<dyn Cancellation>> {
        self.execute(&metadata_sql(request))
    }

    fn poll(&mut self) -> Result<QueryState> {
        let operation = self
            .keep_alive
            .as_mut()
            .or(self.operation.as_mut())
            .context("No Postgres operation")?;
        if operation.cancelled {
            return Ok(QueryState::Cancelled);
        }
        if operation.results.is_none() {
            match operation.receiver.try_recv() {
                Ok(Ok(results)) => operation.results = Some(results),
                Ok(Err(error))
                    if error
                        .downcast_ref::<QueryError>()
                        .is_some_and(|error| error.0.starts_with("57014:")) =>
                {
                    operation.cancelled = true;
                    return Ok(QueryState::Cancelled);
                }
                Ok(Err(error)) => return Err(error),
                Err(mpsc::TryRecvError::Empty) => return Ok(QueryState::Running),
                Err(mpsc::TryRecvError::Disconnected) => {
                    anyhow::bail!("Postgres operation stopped before it returned a result")
                }
            }
        }
        Ok(QueryState::Finished {
            has_results: !operation.results.as_ref().unwrap().columns.is_empty(),
        })
    }

    fn columns(&mut self) -> Result<Vec<Column>> {
        Ok(self
            .operation
            .as_ref()
            .and_then(|operation| operation.results.as_ref())
            .context("Postgres query has no result")?
            .columns
            .clone())
    }

    fn fetch(&mut self, count: usize) -> Result<Batch> {
        let results = self
            .operation
            .as_mut()
            .and_then(|operation| operation.results.as_mut())
            .context("Postgres query has no result")?;
        let mut rows = Vec::new();
        for _ in 0..count {
            if results.rows.fill_buf()?.is_empty() {
                results.exhausted = rows.is_empty();
                break;
            }
            let mut length = [0; 8];
            results.rows.read_exact(&mut length)?;
            let count = u64::from_le_bytes(length) as usize;
            let mut row: Row = Vec::with_capacity(count);
            for _ in 0..count {
                results.rows.read_exact(&mut length)?;
                let length = u64::from_le_bytes(length);
                if length == u64::MAX {
                    row.push(None);
                } else {
                    let mut bytes = vec![0; length as usize];
                    results.rows.read_exact(&mut bytes)?;
                    row.push(Some(String::from_utf8(bytes)?));
                }
            }
            rows.push(row);
        }
        Ok(Batch { rows })
    }

    fn result_limited(&self) -> bool {
        self.operation
            .as_ref()
            .and_then(|operation| operation.results.as_ref())
            .is_some_and(|results| results.limited && results.exhausted)
    }

    fn close_operation(&mut self) -> Result<()> {
        if let Some(operation) = self.operation.take() {
            if !operation.task.is_finished() {
                operation.cancel.cancel()?;
            }
            self.runtime.block_on(async {
                tokio::time::timeout(self.cancel.timeout, operation.task)
                    .await
                    .context("Postgres did not finish operation cleanup in time")?
                    .context("Postgres operation task failed")
            })?;
        }
        Ok(())
    }

    fn execute_keep_alive(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.close_keep_alive()?;
        crate::sql::validate_single_for(sql, crate::model::DatabaseType::Postgres)?;
        let operation = self.start(sql);
        let cancel = operation.cancel.clone();
        self.keep_alive = Some(operation);
        Ok(cancel)
    }

    fn close_keep_alive(&mut self) -> Result<()> {
        if let Some(operation) = self.keep_alive.take() {
            if !operation.task.is_finished() {
                operation.cancel.cancel()?;
            }
            self.runtime.block_on(async {
                tokio::time::timeout(self.cancel.timeout, operation.task)
                    .await
                    .context("Postgres did not finish operation cleanup in time")?
                    .context("Postgres operation task failed")
            })?;
        }
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        self.cancel.closed.store(true, Ordering::SeqCst);
        self.connection.abort();
        if let Some(operation) = self.operation.take() {
            operation.task.abort();
        }
        if let Some(operation) = self.keep_alive.take() {
            operation.task.abort();
        }
        Ok(())
    }
}

impl Drop for PostgresSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn literal(value: &str) -> String {
    format!("E'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

fn metadata_sql(request: &MetadataRequest) -> String {
    let (schema, relation) = match request {
        MetadataRequest::Schemas => return "SELECT nspname AS \"TABLE_SCHEM\" FROM pg_catalog.pg_namespace WHERE left(nspname, 3) <> 'pg_' AND nspname <> 'information_schema' AND pg_catalog.has_schema_privilege(oid, 'USAGE') ORDER BY nspname".into(),
        MetadataRequest::Relations { schema, relation } | MetadataRequest::Columns { schema, relation } => (schema, relation),
    };
    let filter = format!(
        "n.nspname = {}{}",
        literal(schema),
        relation
            .as_ref()
            .map(|relation| format!(" AND c.relname = {}", literal(relation)))
            .unwrap_or_default()
    );
    match request {
        MetadataRequest::Relations { .. } => format!(
            "SELECT n.nspname AS \"TABLE_SCHEM\", c.relname AS \"TABLE_NAME\", CASE WHEN c.relkind IN ('v', 'm') THEN 'VIEW' ELSE 'TABLE' END AS \"TABLE_TYPE\", pg_catalog.obj_description(c.oid, 'pg_class') AS \"REMARKS\" FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE {filter} AND c.relkind IN ('r','p','v','m','f') ORDER BY c.relname"
        ),
        MetadataRequest::Columns { .. } => format!(
            "SELECT n.nspname AS \"TABLE_SCHEM\", c.relname AS \"TABLE_NAME\", a.attname AS \"COLUMN_NAME\", pg_catalog.format_type(a.atttypid, a.atttypmod) AS \"TYPE_NAME\", pg_catalog.col_description(c.oid, a.attnum) AS \"REMARKS\", a.attnum::int AS \"ORDINAL_POSITION\" FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid = a.attrelid JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE {filter} AND c.relkind IN ('r','p','v','m','f') AND a.attnum > 0 AND NOT a.attisdropped ORDER BY c.relname, a.attnum"
        ),
        MetadataRequest::Schemas => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_names_are_exact_literals() {
        assert_eq!(literal("a'b\\c"), "E'a''b\\\\c'");
        let sql = metadata_sql(&MetadataRequest::Columns {
            schema: "odd'name".into(),
            relation: Some("t%_".into()),
        });
        assert!(sql.contains("n.nspname = E'odd''name' AND c.relname = E't%_'"));
        crate::sql::validate_single(&sql).unwrap();
    }
}
