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
    #[serde(default)]
    pub lifecycle: ConnectionLifecycle,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConnectionLifecycle {
    pub idle_seconds: u64,
    /// Zero disables keep-alive and enables idle disconnection.
    pub keep_alive_seconds: u64,
    pub keep_alive_sql: String,
}

impl Default for ConnectionLifecycle {
    fn default() -> Self {
        Self {
            idle_seconds: 900,
            keep_alive_seconds: 0,
            keep_alive_sql: "SELECT 1".into(),
        }
    }
}

impl ConnectionLifecycle {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=604_800).contains(&self.idle_seconds),
            "Idle timeout must be between 1 and 604800 seconds."
        );
        anyhow::ensure!(
            self.keep_alive_seconds <= 604_800,
            "Keep-alive interval must be between 0 and 604800 seconds."
        );
        if self.keep_alive_seconds > 0 {
            crate::sql::validate_single(&self.keep_alive_sql)?;
        }
        Ok(())
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "Spark".into(),
            host: String::new(),
            port: 10009,
            username: String::new(),
            database: "avia".into(),
            parameters: BTreeMap::new(),
            lifecycle: ConnectionLifecycle::default(),
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
        self.lifecycle.validate()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_uses_a_neutral_spark_name() {
        assert_eq!(Profile::default().name, "Spark");
    }

    #[test]
    fn old_profiles_restore_with_idle_disconnect_and_no_heartbeat() {
        let original = Profile::default();
        let mut json = serde_json::to_value(&original).unwrap();
        json.as_object_mut().unwrap().remove("lifecycle");
        let restored: Profile = serde_json::from_value(json).unwrap();
        assert_eq!(restored.id, original.id);
        assert_eq!(restored.lifecycle, ConnectionLifecycle::default());
        assert_eq!(restored.lifecycle.keep_alive_seconds, 0);
    }

    #[test]
    fn lifecycle_rejects_invalid_timers_and_multiple_heartbeat_statements() {
        let mut policy = ConnectionLifecycle {
            idle_seconds: 0,
            ..Default::default()
        };
        assert!(policy.validate().is_err());
        policy.idle_seconds = 900;
        policy.keep_alive_seconds = 604_801;
        assert!(policy.validate().is_err());
        policy.keep_alive_seconds = 300;
        policy.keep_alive_sql = "SELECT 1; SELECT 2".into();
        assert!(policy.validate().is_err());
        policy.keep_alive_sql = "".into();
        assert!(policy.validate().is_err());
        policy.keep_alive_sql = "SELECT 42".into();
        assert!(policy.validate().is_ok());
        let restored: ConnectionLifecycle =
            serde_json::from_str(&serde_json::to_string(&policy).unwrap()).unwrap();
        assert_eq!(restored, policy);
    }
}
