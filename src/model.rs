use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const PREVIEW_ROWS: usize = 1_000;
pub const MAX_RESULT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RESULT_ROWS: usize = 100_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub database: String,
    pub parameters: BTreeMap<String, String>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "Kyuubi".into(),
            host: String::new(),
            port: 10009,
            username: String::new(),
            database: "avia".into(),
            parameters: BTreeMap::new(),
        }
    }
}

impl Profile {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.name.trim().is_empty(), "Give this connection a name.");
        anyhow::ensure!(!self.host.trim().is_empty(), "Enter a host.");
        anyhow::ensure!(self.port != 0, "Port must be between 1 and 65535.");
        anyhow::ensure!(!self.username.trim().is_empty(), "Enter a username.");
        anyhow::ensure!(
            !self.username.contains('\0'),
            "Username contains a null character."
        );
        anyhow::ensure!(
            !self.database.trim().is_empty(),
            "Enter an initial database."
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedTab {
    pub id: Uuid,
    pub title: String,
    pub sql: String,
    pub profile: Option<Uuid>,
}

impl SavedTab {
    pub fn new(number: usize, profile: Option<Uuid>) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: format!("Query {number}"),
            sql: String::new(),
            profile,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub version: u32,
    pub profiles: Vec<Profile>,
    pub tabs: Vec<SavedTab>,
    pub active_tab: usize,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            version: 1,
            profiles: vec![],
            tabs: vec![SavedTab::new(1, None)],
            active_tab: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub data_type: String,
}

pub type Row = Vec<Option<String>>;

#[derive(Default, Debug)]
pub struct Batch {
    pub rows: Vec<Row>,
    pub more: bool,
}
