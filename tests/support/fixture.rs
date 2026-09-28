//! The disposable servers that `./qtest run e2e` provides. A test fails, and
//! does not skip, when it runs without them.
use super::MemoryCredentials;
use qrow::{
    model::{Profile, SavedTab, Workspace},
    storage::Credentials,
};
use std::time::Duration;

/// A warm query answers in well under a second. The budget also covers a
/// fixture on a loaded CI runner.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(60);

/// A synthetic LDAP user of `tests/fixture/server/users.ldif`.
pub const USER: &str = "qrow";
pub const PASSWORD: &str = "qrow-test-password";

pub struct Kyuubi {
    port: u16,
}

impl Kyuubi {
    pub fn get() -> Self {
        let project = std::env::var("QROW_E2E_PROJECT")
            .expect("No server fixture. Run this test with `./qtest run e2e`.");
        assert!(
            project.starts_with("qrow-e2e-"),
            "Not a disposable fixture: {project}"
        );
        let port = std::env::var("QROW_E2E_PORT")
            .expect("The fixture did not give a port")
            .parse()
            .expect("The fixture port is not a number");
        Self { port }
    }

    /// A connection to the fixture as the synthetic user.
    pub fn profile(&self, name: &str) -> Profile {
        Profile {
            name: name.into(),
            host: "127.0.0.1".into(),
            port: self.port,
            username: USER.into(),
            database: "default".into(),
            parameters: [("spark.sql.session.timeZone".into(), "UTC".into())].into(),
            ..Profile::default()
        }
    }

    /// A workspace with one connection to the fixture and one tab with `sql`,
    /// and the password of that connection.
    pub fn workspace(&self, sql: &str, password: &str) -> (Workspace, MemoryCredentials) {
        let profile = self.profile("Spark");
        let credentials = MemoryCredentials::default();
        credentials.set_password(profile.id, password).unwrap();
        let mut tab = SavedTab::new(1, Some(profile.id));
        tab.sql = sql.into();
        let workspace = Workspace {
            profiles: vec![profile],
            tabs: vec![tab],
            ..Workspace::default()
        };
        (workspace, credentials)
    }
}
