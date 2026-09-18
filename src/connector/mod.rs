pub mod hive;
pub mod protocol;
pub mod sasl;
#[allow(clippy::all)]
#[rustfmt::skip]
pub mod t_c_l_i_service;

use crate::model::{Batch, Column, Profile};
use anyhow::Result;
use std::sync::Arc;
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
