//! Integration tests of the core library. They share one test binary, so a
//! change in the library links one binary instead of six.
mod backend;
mod catalog;
mod dbt;
#[path = "../support/dbt_manifest.rs"]
mod dbt_manifest;
mod dbt_worker;
mod hive_protocol;
mod keychain;
mod oidc;
#[path = "../support/oidc.rs"]
mod oidc_provider;
mod sql_properties;
mod storage;
mod tls;
mod workers;

mod postgres;
