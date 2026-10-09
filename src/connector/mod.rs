pub mod hive;
pub mod postgres;
pub mod protocol;
pub mod sasl;
pub mod trino;
#[allow(clippy::all)]
#[rustfmt::skip]
pub mod t_c_l_i_service;

use crate::model::{Batch, Column, Profile};
use anyhow::Result;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[derive(Debug, PartialEq)]
pub enum QueryState {
    Running,
    /// The schema is ready and rows can be fetched while SQL still runs.
    Streaming {
        has_results: bool,
    },
    Finished {
        has_results: bool,
    },
    Cancelled,
}

#[derive(Debug)]
pub struct QueryError(pub String);
impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for QueryError {}

pub trait Cancellation: Send + Sync {
    fn cancel(&self) -> Result<()>;
    fn cancel_with_deadline(&self, _deadline: Instant) -> Result<()> {
        self.cancel()
    }
    /// Interrupt a stuck transport when cancellation cleanup exceeds its deadline.
    /// The owning worker must discard the session after this call.
    fn abort_transport(&self) {}
}

type RegisterTransport = Arc<dyn Fn(Arc<dyn Cancellation>) -> Result<()> + Send + Sync>;

/// Cancellation of setup before the server returns an operation handle.
#[derive(Clone, Default)]
pub struct ConnectionControl {
    cancelled: Option<Arc<AtomicBool>>,
    register: Option<RegisterTransport>,
}

impl ConnectionControl {
    pub fn new(cancelled: Arc<AtomicBool>, register: RegisterTransport) -> Self {
        Self {
            cancelled: Some(cancelled),
            register: Some(register),
        }
    }

    pub fn check(&self) -> Result<()> {
        anyhow::ensure!(
            !self
                .cancelled
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst)),
            crate::export::Cancelled,
        );
        Ok(())
    }

    pub fn register(&self, transport: Arc<dyn Cancellation>) -> Result<()> {
        let result = self.check().and_then(|()| {
            if let Some(register) = &self.register {
                register(transport.clone())?;
            }
            self.check()
        });
        if result.is_err() {
            transport.abort_transport();
        }
        result
    }
}

/// A catalog request. The connector answers it with a result set that uses
/// the JDBC `DatabaseMetaData` column names:
///
/// - [`Schemas`](Self::Schemas): `TABLE_SCHEM`.
/// - [`Relations`](Self::Relations): `TABLE_SCHEM`, `TABLE_NAME`, `TABLE_TYPE`,
///   and `REMARKS`.
/// - [`Columns`](Self::Columns): `TABLE_SCHEM`, `TABLE_NAME`, `COLUMN_NAME`,
///   `TYPE_NAME`, `REMARKS`, and `ORDINAL_POSITION`.
///
/// Names select exact objects. A connector can return more rows than the
/// names select, so the caller keeps only the rows with the requested names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetadataRequest {
    Schemas,
    Relations {
        schema: String,
        relation: Option<String>,
    },
    Columns {
        schema: String,
        relation: Option<String>,
    },
}

pub trait Session: Send {
    fn execute(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>>;
    /// Start one execution whose result is consumed without preview limits.
    fn execute_export(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.execute(sql)
    }
    /// Wait for server completion and protocol cleanup after reading rows.
    fn finish_execution(&mut self) -> Result<Completion> {
        wait_for_completion(self, None)
    }
    /// Start a catalog request as the current operation. Read its rows with
    /// [`poll`](Self::poll), [`columns`](Self::columns), and
    /// [`fetch`](Self::fetch), like the rows of a query.
    fn execute_metadata(&mut self, request: &MetadataRequest) -> Result<Arc<dyn Cancellation>>;
    fn poll(&mut self) -> Result<QueryState>;
    fn columns(&mut self) -> Result<Vec<Column>>;
    fn fetch(&mut self, count: usize) -> Result<Batch>;
    /// Settings captured for the current result. Export must not change them.
    /// Interrupt setup or schema reads before an operation cancellation exists.
    fn transport_cancellation(&self) -> Option<Arc<dyn Cancellation>> {
        None
    }
    fn export_context(&self) -> crate::export::Context {
        crate::export::Context::default()
    }
    /// The server's query progress, when the current response provides it.
    fn progress_percentage(&self) -> Option<f64> {
        None
    }
    /// Whether the connector stopped retaining rows at a result limit.
    fn result_limited(&self) -> bool {
        false
    }
    fn close_operation(&mut self) -> Result<()>;
    /// Start maintenance SQL without replacing the user's result cursor.
    fn execute_keep_alive(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>>;
    fn close_keep_alive(&mut self) -> Result<()>;
    fn close(&mut self) -> Result<()>;
}

/// The time between two status requests for a running operation.
pub const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How an operation ended.
#[derive(Debug, PartialEq)]
pub enum Completion {
    Finished { has_results: bool },
    Cancelled,
}

/// Asks for the status of the current operation every [`POLL_INTERVAL`]
/// until it ends. Fails when `deadline` passes first.
pub fn wait_for_completion<S: Session + ?Sized>(
    session: &mut S,
    deadline: Option<Instant>,
) -> Result<Completion> {
    loop {
        match session.poll()? {
            QueryState::Finished { has_results } => {
                return Ok(Completion::Finished { has_results });
            }
            QueryState::Cancelled => return Ok(Completion::Cancelled),
            QueryState::Running | QueryState::Streaming { .. } => {
                anyhow::ensure!(
                    deadline.is_none_or(|deadline| Instant::now() < deadline),
                    "The operation did not end in time"
                );
                thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

/// Wait for a result schema or a completed command. Streaming does not mean
/// that SQL ended; callers must finish execution before releasing the session.
pub fn wait_for_result<S: Session + ?Sized>(
    session: &mut S,
    deadline: Option<Instant>,
) -> Result<QueryState> {
    loop {
        match session.poll()? {
            QueryState::Running => {
                anyhow::ensure!(
                    deadline.is_none_or(|deadline| Instant::now() < deadline),
                    "The result schema did not arrive in time"
                );
                thread::sleep(POLL_INTERVAL);
            }
            state => return Ok(state),
        }
    }
}

/// Supplies access tokens for one connection. Each call returns a token
/// that is valid now, and refreshes it when necessary.
pub trait TokenSource: Send + Sync {
    fn access_token(&self) -> Result<Zeroizing<String>>;
}

/// What authenticates a connection: a password, or the access tokens of a
/// sign-in. Cancellation opens a second transport, so the session keeps it.
#[derive(Clone)]
pub enum Secret {
    Password(Arc<Zeroizing<String>>),
    Token(Arc<dyn TokenSource>),
    External(crate::external_auth::Source),
}

impl Secret {
    pub fn password(password: impl Into<String>) -> Self {
        Self::Password(Arc::new(Zeroizing::new(password.into())))
    }

    /// Bind a worker's cancellation and progress before opening its session.
    pub fn with_control(self, control: crate::external_auth::Control) -> Self {
        match self {
            Self::External(source) => Self::External(source.with_control(control)),
            secret => secret,
        }
    }

    /// The password, or an access token that is valid now.
    pub fn value(&self) -> Result<Zeroizing<String>> {
        match self {
            Self::Password(password) => Ok(Zeroizing::new(password.as_str().to_owned())),
            Self::Token(source) => source.access_token(),
            Self::External(_) => {
                anyhow::bail!("This connector does not support external authentication")
            }
        }
    }
}

impl From<Zeroizing<String>> for Secret {
    fn from(password: Zeroizing<String>) -> Self {
        Self::Password(Arc::new(password))
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Password(_) => "Secret::Password(..)",
            Self::Token(_) => "Secret::Token(..)",
            Self::External(_) => "Secret::External(..)",
        })
    }
}

pub trait Connector: Send + Sync {
    fn connect(&self, profile: &Profile, secret: Secret) -> Result<Box<dyn Session>>;
    fn connect_controlled(
        &self,
        profile: &Profile,
        secret: Secret,
        control: &ConnectionControl,
    ) -> Result<Box<dyn Session>> {
        control.check()?;
        let session = self.connect(profile, secret)?;
        if let Some(transport) = session.transport_cancellation() {
            control.register(transport)?;
        }
        control.check()?;
        Ok(session)
    }
}

/// Include the diagnostic that Thrift omits from its Display implementation.
pub fn error_message(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    let detail = match error.downcast_ref::<thrift::Error>() {
        Some(thrift::Error::Protocol(error)) => &error.message,
        Some(thrift::Error::Transport(error)) => &error.message,
        _ => return message,
    };
    if detail.is_empty() || message.contains(detail) {
        message
    } else {
        format!("{message}: {detail}")
    }
}

/// Routes a profile to the connector for its database type.
#[derive(Default)]
pub struct DatabaseConnector {
    hive: hive::HiveConnector,
    postgres: postgres::PostgresConnector,
    trino: trino::TrinoConnector,
}

impl DatabaseConnector {
    pub fn new(trust: crate::tls::Trust) -> Self {
        Self {
            hive: hive::HiveConnector::new(trust.clone()),
            postgres: postgres::PostgresConnector::new(trust.clone()),
            trino: trino::TrinoConnector::new(trust),
        }
    }
}

impl Connector for DatabaseConnector {
    fn connect_controlled(
        &self,
        profile: &Profile,
        secret: Secret,
        control: &ConnectionControl,
    ) -> Result<Box<dyn Session>> {
        match profile.database_type {
            crate::model::DatabaseType::Kyuubi => {
                self.hive.connect_controlled(profile, secret, control)
            }
            crate::model::DatabaseType::Postgres => {
                self.postgres.connect_controlled(profile, secret, control)
            }
            crate::model::DatabaseType::Trino => {
                self.trino.connect_controlled(profile, secret, control)
            }
        }
    }

    fn connect(&self, profile: &Profile, secret: Secret) -> Result<Box<dyn Session>> {
        match profile.database_type {
            crate::model::DatabaseType::Kyuubi => self.hive.connect(profile, secret),
            crate::model::DatabaseType::Postgres => self.postgres.connect(profile, secret),
            crate::model::DatabaseType::Trino => self.trino.connect(profile, secret),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A session that answers `poll` with a list of states.
    struct Scripted(VecDeque<QueryState>);

    impl Session for Scripted {
        fn execute(&mut self, _: &str) -> Result<Arc<dyn Cancellation>> {
            unreachable!()
        }
        fn execute_metadata(&mut self, _: &MetadataRequest) -> Result<Arc<dyn Cancellation>> {
            unreachable!()
        }
        fn poll(&mut self) -> Result<QueryState> {
            Ok(self.0.pop_front().unwrap_or(QueryState::Running))
        }
        fn columns(&mut self) -> Result<Vec<Column>> {
            unreachable!()
        }
        fn fetch(&mut self, _: usize) -> Result<Batch> {
            unreachable!()
        }
        fn close_operation(&mut self) -> Result<()> {
            unreachable!()
        }
        fn execute_keep_alive(&mut self, _: &str) -> Result<Arc<dyn Cancellation>> {
            unreachable!()
        }
        fn close_keep_alive(&mut self) -> Result<()> {
            unreachable!()
        }
        fn close(&mut self) -> Result<()> {
            unreachable!()
        }
    }

    #[test]
    fn waits_until_the_operation_ends() {
        let mut session = Scripted(VecDeque::from([
            QueryState::Running,
            QueryState::Streaming { has_results: true },
            QueryState::Finished { has_results: true },
        ]));
        let started = Instant::now();
        assert_eq!(
            wait_for_completion(&mut session, None).unwrap(),
            Completion::Finished { has_results: true }
        );
        assert!(started.elapsed() >= POLL_INTERVAL);
        let mut session = Scripted(VecDeque::from([QueryState::Cancelled]));
        assert_eq!(
            wait_for_completion(&mut session, None).unwrap(),
            Completion::Cancelled
        );
    }

    #[test]
    fn fails_when_the_deadline_passes() {
        let mut session = Scripted(VecDeque::new());
        let error = wait_for_completion(&mut session, Some(Instant::now())).unwrap_err();
        assert_eq!(error.to_string(), "The operation did not end in time");
    }

    #[test]
    fn schema_readiness_does_not_claim_execution_completion() {
        let mut session = Scripted(VecDeque::from([
            QueryState::Streaming { has_results: true },
            QueryState::Streaming { has_results: true },
            QueryState::Finished { has_results: true },
        ]));
        assert_eq!(
            wait_for_result(&mut session, None).unwrap(),
            QueryState::Streaming { has_results: true }
        );
        let started = Instant::now();
        assert_eq!(
            session.finish_execution().unwrap(),
            Completion::Finished { has_results: true }
        );
        assert!(started.elapsed() >= POLL_INTERVAL);
    }
}
