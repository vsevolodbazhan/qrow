//! The real Qrow window, headless, against disposable LDAP, Kyuubi, and Spark
//! servers. Run with `./qtest run e2e`, which provides the servers.
#[path = "../support/mod.rs"]
mod support;

mod connections;
mod queries;
