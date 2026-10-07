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
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[derive(Debug, PartialEq)]
pub enum QueryState {
    Running,
    Finished { has_results: bool },
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
    /// Start a catalog request as the current operation. Read its rows with
    /// [`poll`](Self::poll), [`columns`](Self::columns), and
    /// [`fetch`](Self::fetch), like the rows of a query.
    fn execute_metadata(&mut self, request: &MetadataRequest) -> Result<Arc<dyn Cancellation>>;
    fn poll(&mut self) -> Result<QueryState>;
    fn columns(&mut self) -> Result<Vec<Column>>;
    fn fetch(&mut self, count: usize) -> Result<Batch>;
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
            QueryState::Running => {
                anyhow::ensure!(
                    deadline.is_none_or(|deadline| Instant::now() < deadline),
                    "The operation did not end in time"
                );
                thread::sleep(POLL_INTERVAL);
            }
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
}
