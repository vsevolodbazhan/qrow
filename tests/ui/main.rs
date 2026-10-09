//! UI integration tests of the real Qrow window, without a database server.
#[path = "../support/mod.rs"]
mod support;

mod assistant_chat;
mod assistant_harness;
mod assistant_idle;
mod assistant_layout;
mod assistant_notes;
mod assistant_sql;
mod assistant_tabs;
mod assistant_threads;
mod assistant_transcript;
mod catalog;
mod connections;
mod dbt;
#[path = "../support/dbt_manifest.rs"]
mod dbt_manifest;
mod dialogs;
mod export;
mod metadata;
mod queries;
mod results;
mod sign_ins;
mod tabs;
