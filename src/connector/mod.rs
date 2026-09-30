pub mod hive;
pub mod protocol;
pub mod sasl;
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

pub trait Session: Send {
    fn execute(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>>;
    fn poll(&mut self) -> Result<QueryState>;
    fn columns(&mut self) -> Result<Vec<Column>>;
    fn fetch(&mut self, count: usize) -> Result<Batch>;
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

pub trait Connector: Send + Sync {
    fn connect(&self, profile: &Profile, password: Zeroizing<String>) -> Result<Box<dyn Session>>;
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
