//! Postgres sessions. Values use the server's text representation.
mod cancellation;
#[cfg(test)]
mod driver_tests;
mod results;
mod schema;
mod tls;
mod transport;
#[cfg(test)]
mod transport_tests;

use super::{
    Cancellation, ConnectionControl, Connector, MetadataRequest, QueryError, QueryState, Secret,
    Session,
};
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
    io::{BufWriter, Write},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use tokio::runtime::Runtime;
use tokio_postgres::{
    Client, Config, NoTls, SimpleQueryMessage, config::SslMode, tls::MakeTlsConnect,
};
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
    token: tokio_postgres::CancelToken,
    host: String,
    endpoint: cancellation::Endpoint,
    closed: Arc<AtomicBool>,
    tls: Option<MakeRustlsConnect>,
    timeout: Duration,
    transport: Arc<transport::Abort>,
    secondary: Mutex<Option<Arc<transport::Abort>>>,
    deadline: Mutex<Option<Instant>>,
    send_started: AtomicBool,
    completion: (Mutex<bool>, Condvar),
}

impl Cancel {
    async fn send(&self) -> Result<()> {
        if self.finished.load(Ordering::SeqCst) || self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        if self.send_started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let timeout = self
            .deadline
            .lock()
            .unwrap()
            .map_or(self.timeout, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
        let result = tokio::time::timeout(
            timeout,
            cancellation::send(
                &self.token,
                &self.host,
                &self.endpoint,
                self.tls.clone(),
                |socket| {
                    *self.secondary.lock().unwrap() = Some(socket.clone());
                    if self.closed.load(Ordering::SeqCst) {
                        socket.abort_transport();
                    }
                },
            ),
        )
        .await
        .context("Postgres did not answer the cancellation request in time")
        .and_then(|result| result.context("Could not cancel the Postgres query"))
        .and_then(|()| {
            anyhow::ensure!(
                !self.closed.load(Ordering::SeqCst),
                "Postgres cancellation transport was interrupted"
            );
            Ok(())
        });
        if result.is_err() {
            // A partially sent packet can still arrive. Do not reuse its backend.
            self.abort_transport();
        }
        self.secondary.lock().unwrap().take();
        result
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

    fn finish(&self) {
        self.finished.store(true, Ordering::SeqCst);
        *self.completion.0.lock().unwrap() = true;
        self.completion.1.notify_all();
    }
}

impl Cancellation for Cancel {
    fn cancel(&self) -> Result<()> {
        if self.finished.load(Ordering::SeqCst) || self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.requested.store(true, Ordering::SeqCst);
        // The operation owns and awaits every cancellation transport. A handle
        // must not send a packet after that operation has ended.
        Ok(())
    }
    fn cancel_with_deadline(&self, deadline: Instant) -> Result<()> {
        if self.finished.load(Ordering::SeqCst) || self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        {
            let mut current = self.deadline.lock().unwrap();
            *current = Some(current.map_or(deadline, |old| old.min(deadline)));
        }
        self.cancel()?;
        let mut finished = self.completion.0.lock().unwrap();
        while !*finished {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.abort_transport();
                anyhow::bail!("Postgres cancellation cleanup exceeded its deadline");
            }
            finished = self
                .completion
                .1
                .wait_timeout(finished, remaining)
                .unwrap()
                .0;
        }
        Ok(())
    }
    fn abort_transport(&self) {
        self.transport.abort_transport();
        if let Some(socket) = self.secondary.lock().unwrap().as_ref() {
            socket.abort_transport();
        }
    }
}

use results::{Mode, Output, Progress, Results};

struct Operation {
    cancel: Arc<Cancel>,
    task: tokio::task::JoinHandle<()>,
    receiver: mpsc::Receiver<Result<Results>>,
    results: Option<Results>,
    progress: Arc<Progress>,
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

async fn connect_client<
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
>(
    config: &Config,
    tls: Option<&MakeRustlsConnect>,
    host: &str,
    socket: S,
    transport: &Arc<transport::Abort>,
) -> Result<(Client, tokio::task::JoinHandle<()>)> {
    let closed = transport.closed.clone();
    match tls {
        Some(tls) => {
            let mut tls = tls.clone();
            let tls = <MakeRustlsConnect as MakeTlsConnect<S>>::make_tls_connect(&mut tls, host)?;
            let (client, connection) = config
                .connect_raw(
                    socket,
                    transport::GuardedTls {
                        tls,
                        limit: transport.limit.clone(),
                        boundary: transport.boundary.clone(),
                    },
                )
                .await?;
            Ok((
                client,
                tokio::spawn(async move {
                    let _ = connection.await;
                    closed.store(true, Ordering::SeqCst);
                }),
            ))
        }
        None => {
            let (client, connection) = config
                .connect_raw(
                    transport::Wire::controlled(
                        socket,
                        transport.limit.clone(),
                        transport.boundary.clone(),
                    ),
                    NoTls,
                )
                .await?;
            Ok((
                client,
                tokio::spawn(async move {
                    let _ = connection.await;
                    closed.store(true, Ordering::SeqCst);
                }),
            ))
        }
    }
}

impl Connector for PostgresConnector {
    fn connect(&self, profile: &Profile, secret: Secret) -> Result<Box<dyn Session>> {
        self.connect_session(profile, secret, None)
    }
    fn connect_controlled(
        &self,
        profile: &Profile,
        secret: Secret,
        control: &ConnectionControl,
    ) -> Result<Box<dyn Session>> {
        self.connect_session(profile, secret, Some(control))
    }
}

impl PostgresConnector {
    fn connect_session(
        &self,
        profile: &Profile,
        secret: Secret,
        control: Option<&ConnectionControl>,
    ) -> Result<Box<dyn Session>> {
        if let Some(control) = control {
            control.check()?;
        }
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
        let limit = if control.is_some() {
            transport::EXPORT_FRAME_BYTES
        } else {
            transport::PREVIEW_FRAME_BYTES
        };
        let (client, connection, transport) = runtime.block_on(async {
            tokio::time::timeout(timeout, async {
                #[cfg(unix)]
                if let [tokio_postgres::config::Host::Unix(directory)] = config.get_hosts() {
                    let path = directory.join(format!(".s.PGSQL.{}", profile.port));
                    let (socket, transport) = transport::Abort::unix(&path, limit).await?;
                    if let Some(control) = control {
                        control.register(transport.clone())?;
                    }
                    let connected =
                        connect_client(&config, tls.as_ref(), &profile.host, socket, &transport)
                            .await;
                    return match connected {
                        Ok((client, connection)) => Ok((client, connection, transport)),
                        Err(error) => {
                            transport.abort_transport();
                            Err(error)
                        }
                    };
                }
                let addresses =
                    tokio::net::lookup_host((profile.host.as_str(), profile.port)).await?;
                let mut last_error = None;
                for address in addresses {
                    if let Some(control) = control {
                        control.check()?;
                    }
                    let (socket, transport) = match transport::Abort::tcp(address, limit).await {
                        Ok(connected) => connected,
                        Err(error) => {
                            last_error = Some(error);
                            continue;
                        }
                    };
                    if let Some(control) = control {
                        control.register(transport.clone())?;
                    }
                    let connected =
                        connect_client(&config, tls.as_ref(), &profile.host, socket, &transport)
                            .await;
                    match connected {
                        Ok((client, connection)) => {
                            return Ok((client, connection, transport));
                        }
                        Err(error) => {
                            transport.abort_transport();
                            last_error = Some(error);
                        }
                    }
                }
                Err::<_, anyhow::Error>(
                    last_error.unwrap_or_else(|| anyhow::anyhow!("Postgres host has no addresses")),
                )
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
                if let Some(control) = control {
                    control.check()?;
                }
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
        if let Some(control) = control {
            control.check()?;
        }
        let cancel = Arc::new(Cancel {
            requested: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            token: client.cancel_token(),
            host: profile.host.clone(),
            endpoint: transport.endpoint()?,
            closed: transport.closed.clone(),
            tls,
            timeout,
            transport,
            secondary: Mutex::default(),
            deadline: Mutex::default(),
            send_started: AtomicBool::new(false),
            completion: (Mutex::new(false), Condvar::new()),
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

fn export_type(name: &str, modifier: i32) -> String {
    match name {
        "numeric" if modifier >= 4 => {
            let parameters = modifier - 4;
            let precision = (parameters >> 16) & 0xffff;
            let scale = ((parameters & 0x7ff) ^ 1024) - 1024;
            format!("numeric({precision},{scale})")
        }
        "timestamp" | "timestamptz" if modifier >= 0 => format!("{name}({modifier})"),
        _ => name.to_owned(),
    }
}

impl PostgresSession {
    fn close_owned_operation(&self, mut operation: Operation) -> Result<()> {
        if !operation.task.is_finished() {
            operation.cancel.cancel()?;
        }
        let result = self.runtime.block_on(async {
            tokio::time::timeout(self.cancel.timeout, &mut operation.task)
                .await
                .context("Postgres did not finish operation cleanup in time")?
                .context("Postgres operation task failed")
        });
        if result.is_err() {
            operation.cancel.abort_transport();
            operation.task.abort();
        }
        result?;
        anyhow::ensure!(
            !operation.cancel.closed.load(Ordering::SeqCst),
            "Postgres session is closed after operation cleanup"
        );
        Ok(())
    }
    fn execute_mode(&mut self, sql: &str, mode: Mode) -> Result<Arc<dyn Cancellation>> {
        crate::sql::validate_single_for(sql, crate::model::DatabaseType::Postgres)?;
        if mode == Mode::Export {
            anyhow::ensure!(
                sql.len() <= schema::MAX_SQL_BYTES,
                "Postgres export SQL exceeds the 1 MiB limit"
            );
        }
        self.close_operation()?;
        self.close_keep_alive()?;
        self.cancel.transport.limit.store(
            if mode == Mode::Export {
                transport::EXPORT_FRAME_BYTES
            } else {
                transport::PREVIEW_FRAME_BYTES
            },
            Ordering::SeqCst,
        );
        if mode == Mode::Export {
            self.client.clear_type_cache();
            let result = self.runtime.block_on(async {
                tokio::time::timeout(
                    self.cancel.timeout,
                    self.client
                        .reset_buffers_when_idle(self.cancel.transport.boundary.clone()),
                )
                .await
                .context("Postgres did not reach the export buffer boundary in time")?
                .context("Could not reset Postgres export buffers")
            });
            if result.is_err() {
                self.cancel.abort_transport();
            }
            result?;
        }
        let operation = self.start(sql, mode)?;
        let cancel = operation.cancel.clone();
        self.operation = Some(operation);
        Ok(cancel)
    }

    fn start(&self, sql: &str, mode: Mode) -> Result<Operation> {
        anyhow::ensure!(
            !self.cancel.closed.load(Ordering::SeqCst),
            "Postgres session is closed"
        );
        let (sender, receiver) = mpsc::channel();
        let client = self.client.clone();
        let sql = sql.to_owned();
        let cancel = Arc::new(Cancel {
            requested: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            token: self.cancel.token.clone(),
            host: self.cancel.host.clone(),
            endpoint: self.cancel.endpoint.clone(),
            closed: self.cancel.closed.clone(),
            tls: self.cancel.tls.clone(),
            timeout: self.cancel.timeout,
            transport: self.cancel.transport.clone(),
            secondary: Mutex::default(),
            deadline: Mutex::default(),
            send_started: AtomicBool::new(false),
            completion: (Mutex::new(false), Condvar::new()),
        });
        let requested = cancel.clone();
        let progress = Arc::new(Progress::default());
        let task_progress = progress.clone();
        let (mut output, rows) = results::source(mode)?;
        let task = self.runtime.spawn(async move {
            let mut source = Some(rows);
            let mut protocol_started = false;
            let result = async {
                requested.check()?;
                protocol_started = true;
                let statement = requested.until(client.prepare_bounded(&sql)).await?;
                let types = schema::types(&client, &requested, &statement).await?;
                let has_results = !statement.columns().is_empty();
                drop(statement);
                let context = if !has_results {
                    crate::export::Context::default()
                } else {
                    let description = requested.until(client.prepare_bounded(
                        "SELECT pg_catalog.current_setting('DateStyle'), pg_catalog.current_setting('IntervalStyle'), pg_catalog.current_setting('TimeZone')"
                    )).await?;
                    let settings = requested.until(client.query_one(&description, &[])).await?;
                    let values: [&str; 3] = [settings.try_get(0)?, settings.try_get(1)?, settings.try_get(2)?];
                    anyhow::ensure!(values.iter().all(|value| value.len() <= 64 * 1024), "Postgres export settings exceed their metadata limit");
                    crate::export::Context { postgres: Some(crate::export::PostgresContext {
                        date_style: values[0].into(), interval_style: values[1].into(), time_zone: values[2].into(),
                    }) }
                };
                requested.check()?;
                let stream = requested.until(client.simple_query_raw(&sql)).await?;
                pin_mut!(stream);
                let mut width = 0;
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
                            if mode == Mode::Export {
                                anyhow::ensure!(description.len() <= 4096, "Postgres export schema exceeds 4,096 columns");
                                let bytes = description.len().saturating_mul(std::mem::size_of::<Column>())
                                    .saturating_add(description.iter().map(|column| column.name().len()).sum::<usize>())
                                    .saturating_add(types.iter().map(String::len).sum::<usize>());
                                anyhow::ensure!(bytes <= crate::export::budget::MAX_ROW_BYTES, "Postgres export schema exceeds 16 MiB");
                            }
                            width = description.len();
                            let columns = description
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
                            let rows = source.take().context("Postgres returned more than one result set")?;
                            sender.send(Ok(Results { columns, exhausted: false, rows, context: context.clone() }))
                                .map_err(|_| anyhow::anyhow!("Postgres result consumer closed"))?;
                        }
                        SimpleQueryMessage::Row(row) => {
                            anyhow::ensure!(row.len() == width, "Postgres returned a row with the wrong width");
                            match &mut output {
                                Output::Prefix(prefix) => prefix.append(&row, &task_progress)?,
                                Output::Direct(sender) => {
                                    crate::export::budget::selected_row_bytes((0..row.len()).map(|index| row.get(index)))?;
                                    let owned: Row = (0..row.len()).map(|index| row.get(index).map(str::to_owned)).collect();
                                    let send = sender.send(owned);
                                    pin_mut!(send);
                                    loop {
                                        tokio::select! {
                                            sent = &mut send => {
                                                sent.map_err(|_| anyhow::anyhow!("Postgres export consumer closed"))?;
                                                break;
                                            }
                                            _ = cancellation_tick.tick() => {
                                                if requested.requested.load(Ordering::SeqCst) {
                                                    requested.send().await?;
                                                    requested.check()?;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if let Output::Prefix(prefix) = &mut output { prefix.commit(&task_progress)?; }
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = &result {
                // Wake consumers before server cleanup. Cancel retains ownership
                // until cancellation and the protocol barrier finish.
                task_progress.fail(error);
            }
            drop(output);
            let result = if result.is_err() && protocol_started && !requested.closed.load(Ordering::SeqCst) {
                // A local sink/limit failure can leave SQL running. Stop it before
                // the Sync barrier; a server SQL error has already stopped it.
                if result.as_ref().err().is_some_and(|error| !error.is::<QueryError>()) {
                    let _ = requested.cancel();
                    let _ = requested.send().await;
                }
                match tokio::time::timeout(requested.timeout, client.check_connection()).await {
                    Ok(Ok(())) => result,
                    _ => { requested.abort_transport(); result }
                }
            } else { result };
            requested.finish();
            task_progress.finish(&result);
            if let Some(rows) = source {
                let result = result.map(|()| Results {
                    columns: Vec::new(), exhausted: false, rows,
                    context: crate::export::Context::default(),
                });
                let _ = sender.send(result);
            }
        });
        Ok(Operation {
            cancel,
            task,
            receiver,
            results: None,
            progress,
            cancelled: false,
        })
    }
}

impl Session for PostgresSession {
    fn transport_cancellation(&self) -> Option<Arc<dyn Cancellation>> {
        Some(self.cancel.transport.clone())
    }
    fn execute(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.execute_mode(sql, Mode::Preview)
    }

    fn execute_export(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.execute_mode(sql, Mode::Export)
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
        let completed = match operation.progress.completed() {
            Ok(completed) => completed,
            Err(error) => {
                if error
                    .downcast_ref::<QueryError>()
                    .is_some_and(|error| error.0.starts_with("57014:"))
                {
                    if !operation.cancel.finished.load(Ordering::SeqCst) {
                        return Ok(QueryState::Running);
                    }
                    operation.cancelled = true;
                    return Ok(QueryState::Cancelled);
                }
                return Err(error);
            }
        };
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
        let has_results = !operation.results.as_ref().unwrap().columns.is_empty();
        Ok(if completed {
            QueryState::Finished { has_results }
        } else {
            QueryState::Streaming { has_results }
        })
    }

    fn export_context(&self) -> crate::export::Context {
        self.operation
            .as_ref()
            .and_then(|operation| operation.results.as_ref())
            .map(|results| results.context.clone())
            .unwrap_or_default()
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
        let operation = self
            .operation
            .as_mut()
            .context("Postgres query has no result")?;
        let results = operation
            .results
            .as_mut()
            .context("Postgres query has no result")?;
        results.fetch(count, &operation.progress, &self.runtime)
    }

    fn result_limited(&self) -> bool {
        self.operation.as_ref().is_some_and(|operation| {
            operation.progress.limited()
                && operation
                    .results
                    .as_ref()
                    .is_some_and(|results| results.exhausted)
        })
    }

    fn close_operation(&mut self) -> Result<()> {
        if let Some(operation) = self.operation.take() {
            self.close_owned_operation(operation)?;
        }
        Ok(())
    }

    fn execute_keep_alive(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        anyhow::ensure!(
            !self.cancel.closed.load(Ordering::SeqCst),
            "Postgres session is closed"
        );
        self.close_keep_alive()?;
        anyhow::ensure!(
            self.operation
                .as_ref()
                .is_none_or(|operation| operation.cancel.finished.load(Ordering::SeqCst)),
            "Postgres query is still running"
        );
        crate::sql::validate_single_for(sql, crate::model::DatabaseType::Postgres)?;
        self.cancel
            .transport
            .limit
            .store(transport::PREVIEW_FRAME_BYTES, Ordering::SeqCst);
        let operation = self.start(sql, Mode::Preview)?;
        let cancel = operation.cancel.clone();
        self.keep_alive = Some(operation);
        Ok(cancel)
    }

    fn close_keep_alive(&mut self) -> Result<()> {
        if let Some(operation) = self.keep_alive.take() {
            self.close_owned_operation(operation)?;
        }
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        self.cancel.abort_transport();
        self.connection.abort();
        if let Some(operation) = self.operation.take() {
            operation.cancel.abort_transport();
            operation.task.abort();
            operation.cancel.finish();
        }
        if let Some(operation) = self.keep_alive.take() {
            operation.cancel.abort_transport();
            operation.task.abort();
            operation.cancel.finish();
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
    fn export_type_keeps_exact_numeric_and_timestamp_modifiers() {
        for (precision, scale) in [(18, 2), (3, -2), (3, 5), (38, 38)] {
            let modifier = ((precision << 16) | (scale & 0x7ff)) + 4;
            assert_eq!(
                export_type("numeric", modifier),
                format!("numeric({precision},{scale})")
            );
        }
        assert_eq!(export_type("numeric", -1), "numeric");
        assert_eq!(export_type("timestamp", 6), "timestamp(6)");
        assert_eq!(export_type("timestamptz", 3), "timestamptz(3)");
        assert_eq!(export_type("date", -1), "date");
    }

    pub(super) fn idle_session() -> (PostgresSession, tokio::io::DuplexStream) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (client, server) = tokio::io::duplex(1024);
        let transport = transport::Abort::synthetic();
        let mut config = Config::new();
        // NoTls keeps the primary fixture plain. Prefer allows a secondary
        // TLS connector to exercise its SSL negotiation with this token.
        config
            .user("synthetic")
            .host("127.0.0.1")
            .ssl_mode(SslMode::Prefer);
        let ((client, connection), server) = runtime.block_on(async {
            let handshake = async {
                let mut server = server;
                let length = server.read_u32().await.unwrap();
                let mut startup = vec![0; length as usize - 4];
                server.read_exact(&mut startup).await.unwrap();
                // AuthenticationOk, synthetic BackendKeyData, ReadyForQuery.
                server
                    .write_all(&[
                        b'R', 0, 0, 0, 8, 0, 0, 0, 0, b'K', 0, 0, 0, 12, 0, 0, 0, 1, 0, 0, 0, 2,
                        b'Z', 0, 0, 0, 5, b'I',
                    ])
                    .await
                    .unwrap();
                server
            };
            let wire = transport::Wire::controlled(
                client,
                transport.limit.clone(),
                transport.boundary.clone(),
            );
            let (connection, server) = tokio::join!(config.connect_raw(wire, NoTls), handshake);
            (connection.unwrap(), server)
        });
        let cancel = Arc::new(Cancel {
            requested: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            token: client.cancel_token(),
            host: "127.0.0.1".into(),
            endpoint: cancellation::Endpoint::Tcp("127.0.0.1:0".parse().unwrap()),
            closed: transport.closed.clone(),
            tls: None,
            timeout: Duration::from_millis(10),
            transport,
            secondary: Mutex::default(),
            deadline: Mutex::default(),
            send_started: AtomicBool::new(false),
            completion: (Mutex::new(false), Condvar::new()),
        });
        let connection = runtime.spawn(async move {
            let _ = connection.await;
        });
        (
            PostgresSession {
                client: Arc::new(client),
                connection,
                runtime,
                cancel,
                operation: None,
                keep_alive: None,
            },
            server,
        )
    }

    #[test]
    fn cancellation_before_submission_does_not_open_a_transport() {
        let (mut session, _server) = idle_session();
        // This runtime does not run the query task until block_on below.
        let handle = session.execute("SELECT pg_sleep(30)").unwrap();
        handle.cancel().unwrap();
        assert!(!session.cancel.closed.load(Ordering::SeqCst));
        session
            .runtime
            .block_on(&mut session.operation.as_mut().unwrap().task)
            .unwrap();
        assert_eq!(session.poll().unwrap(), QueryState::Cancelled);
        assert_eq!(session.poll().unwrap(), QueryState::Cancelled);
        assert!(!session.cancel.closed.load(Ordering::SeqCst));
        let operation = session.operation.as_ref().unwrap();
        assert!(operation.cancel.finished.load(Ordering::SeqCst));
        operation.cancel.requested.store(false, Ordering::SeqCst);
        handle.cancel().unwrap();
        assert!(!operation.cancel.requested.load(Ordering::SeqCst));
    }

    #[test]
    fn cancellation_transport_failure_prevents_session_reuse() {
        use tokio::io::AsyncReadExt;
        let (mut session, _server) = idle_session();
        let listener = session
            .runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        Arc::get_mut(&mut session.cancel).unwrap().endpoint =
            cancellation::Endpoint::Tcp(listener.local_addr().unwrap());
        Arc::get_mut(&mut session.cancel).unwrap().timeout = Duration::from_secs(1);
        let received = AtomicBool::new(false);
        session.runtime.block_on(async {
            let send = session.cancel.send();
            let server = async {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut packet = [0; 16];
                stream.read_exact(&mut packet).await.unwrap();
                assert_eq!(
                    u32::from_be_bytes(packet[4..8].try_into().unwrap()),
                    80877102
                );
                received.store(true, Ordering::SeqCst);
                // Hold the socket open after receiving the complete CancelRequest.
                std::future::pending::<()>().await;
            };
            tokio::pin!(server);
            tokio::select! {
                result = send => assert!(result.unwrap_err().to_string().contains("in time")),
                _ = &mut server => unreachable!(),
            }
        });
        assert!(received.load(Ordering::SeqCst));
        assert!(
            session
                .execute("SELECT 42")
                .err()
                .unwrap()
                .to_string()
                .contains("closed")
        );
        assert!(
            session
                .execute_keep_alive("SELECT 1")
                .err()
                .unwrap()
                .to_string()
                .contains("closed")
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_supports_unix_sockets() {
        use tokio::io::AsyncReadExt;
        let (mut session, _server) = idle_session();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".s.PGSQL.5432");
        let listener = {
            let _enter = session.runtime.enter();
            tokio::net::UnixListener::bind(&path).unwrap()
        };
        Arc::get_mut(&mut session.cancel).unwrap().endpoint = cancellation::Endpoint::Unix(path);
        Arc::get_mut(&mut session.cancel).unwrap().timeout = Duration::from_secs(1);
        session.runtime.block_on(async {
            let server = async {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut packet = [0; 16];
                stream.read_exact(&mut packet).await.unwrap();
                assert_eq!(
                    u32::from_be_bytes(packet[4..8].try_into().unwrap()),
                    80877102
                );
                // Closing the accepted socket completes the cancellation transport.
            };
            let (result, ()) = tokio::join!(session.cancel.send(), server);
            result.unwrap();
        });
        assert!(!session.cancel.closed.load(Ordering::SeqCst));
    }

    #[test]
    fn cancellation_failure_during_cleanup_prevents_replacement_work() {
        use tokio::io::AsyncReadExt;
        for keep_alive in [false, true] {
            let (mut session, mut server) = idle_session();
            let listener = session
                .runtime
                .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
                .unwrap();
            Arc::get_mut(&mut session.cancel).unwrap().endpoint =
                cancellation::Endpoint::Tcp(listener.local_addr().unwrap());
            let cancellation_server = session.runtime.spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut packet = [0; 16];
                stream.read_exact(&mut packet).await.unwrap();
                std::future::pending::<()>().await;
            });
            if keep_alive {
                session.execute_keep_alive("SELECT 1").unwrap();
            } else {
                session.execute("SELECT 1").unwrap();
            }
            // Prove preparation was submitted before cleanup requests cancellation.
            assert_eq!(session.runtime.block_on(server.read_u8()).unwrap(), b'P');
            // The operation has a short transport timeout. Cleanup gets enough time
            // to join it after that failure and must then recheck session validity.
            Arc::get_mut(&mut session.cancel).unwrap().timeout = Duration::from_secs(1);
            let replacement = if keep_alive {
                session.execute_keep_alive("SELECT 2")
            } else {
                session.execute("SELECT 2")
            };
            assert!(replacement.err().unwrap().to_string().contains("closed"));
            assert!(session.operation.is_none());
            assert!(session.keep_alive.is_none());
            cancellation_server.abort();
        }
    }

    #[test]
    fn cancellation_deadline_aborts_primary_and_secondary_during_tls_or_eof() {
        use tokio::io::AsyncReadExt;
        for tls in [false, true] {
            let (mut session, _server) = idle_session();
            let primary_listener = session
                .runtime
                .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
                .unwrap();
            let (primary, transport) = session
                .runtime
                .block_on(transport::Abort::tcp(
                    primary_listener.local_addr().unwrap(),
                    transport::EXPORT_FRAME_BYTES,
                ))
                .unwrap();
            let (mut primary_peer, _) =
                session.runtime.block_on(primary_listener.accept()).unwrap();
            let listener = session
                .runtime
                .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
                .unwrap();
            let cancel = Arc::get_mut(&mut session.cancel).unwrap();
            cancel.endpoint = cancellation::Endpoint::Tcp(listener.local_addr().unwrap());
            cancel.closed = transport.closed.clone();
            cancel.transport = transport;
            cancel.timeout = Duration::from_secs(10);
            if tls {
                cancel.tls = Some(MakeRustlsConnect::new(tls::encryption_only().unwrap()));
            }
            let requested = session.cancel.clone();
            let send = session.runtime.spawn(async move { requested.send().await });
            let mut secondary_peer = session.runtime.block_on(async {
                let (mut peer, _) = listener.accept().await.unwrap();
                let mut packet = vec![0; if tls { 8 } else { 16 }];
                peer.read_exact(&mut packet).await.unwrap();
                assert_eq!(
                    u32::from_be_bytes(packet[4..8].try_into().unwrap()),
                    if tls { 80877103 } else { 80877102 }
                );
                // Hold SSL negotiation or CancelRequest EOF indefinitely.
                peer
            });
            let secondary = session.cancel.secondary.lock().unwrap().clone().unwrap();
            let requested = session.cancel.clone();
            let started = Instant::now();
            let deadline = started + Duration::from_millis(100);
            let cleanup = std::thread::spawn(move || requested.cancel_with_deadline(deadline));
            session.runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(2), async {
                    assert!(send.await.unwrap().is_err());
                    assert_eq!(primary_peer.read(&mut [0; 1]).await.unwrap(), 0);
                    assert_eq!(secondary_peer.read(&mut [0; 1]).await.unwrap(), 0);
                })
                .await
                .unwrap();
            });
            assert!(
                cleanup
                    .join()
                    .unwrap()
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("deadline")
            );
            assert!(started.elapsed() < Duration::from_secs(2));
            assert!(session.cancel.transport.closed.load(Ordering::SeqCst));
            assert!(secondary.closed.load(Ordering::SeqCst));
            assert!(
                session
                    .execute("SELECT 42")
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("closed")
            );
            drop(primary);
        }
    }

    #[test]
    fn sql_error_waits_for_sync_ready_for_query_before_session_reuse() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        async fn through_sync(server: &mut tokio::io::DuplexStream) {
            loop {
                let tag = server.read_u8().await.unwrap();
                let length = server.read_u32().await.unwrap() as usize;
                assert!((4..4096).contains(&length));
                server.read_exact(&mut vec![0; length - 4]).await.unwrap();
                if tag == b'S' {
                    break;
                }
            }
        }
        let (mut session, mut server) = idle_session();
        Arc::get_mut(&mut session.cancel).unwrap().timeout = Duration::from_secs(2);
        session.execute("SELECT missing_column").unwrap();
        session.runtime.block_on(async {
            through_sync(&mut server).await;
            let fields = b"SERROR\0C42703\0Msynthetic missing column\0\0";
            server.write_u8(b'E').await.unwrap();
            server.write_u32((fields.len() + 4) as u32).await.unwrap();
            server.write_all(fields).await.unwrap();
            // The original ReadyForQuery and the cleanup Sync response are held.
            through_sync(&mut server).await;
        });
        assert!(
            !session
                .operation
                .as_ref()
                .unwrap()
                .cancel
                .finished
                .load(Ordering::SeqCst)
        );
        assert!(
            session
                .poll()
                .unwrap_err()
                .to_string()
                .contains("42703: synthetic missing column")
        );
        session.runtime.block_on(async {
            server.write_all(b"Z\0\0\0\x05IZ\0\0\0\x05I").await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while !session.operation.as_ref().unwrap().task.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        });
        assert!(
            session
                .poll()
                .unwrap_err()
                .to_string()
                .contains("42703: synthetic missing column")
        );
        assert!(
            session
                .operation
                .as_ref()
                .unwrap()
                .cancel
                .finished
                .load(Ordering::SeqCst)
        );
        assert!(!session.cancel.closed.load(Ordering::SeqCst));
        session.close_operation().unwrap();
        session.execute("SELECT 42").unwrap();
    }

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
