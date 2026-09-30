//! The disposable servers that `./qtest run e2e` provides. A test fails, and
//! does not skip, when it runs without them.
use super::{MemoryCredentials, TestApp, cell, connection_row, label};
use gpui_kit::TestAppContext;
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

impl Kyuubi {
    /// Connections to the fixture with these names, one tab each, and their
    /// passwords. The first tab has `sql`.
    pub fn connections(&self, names: &[&str], sql: &str) -> (Workspace, MemoryCredentials) {
        let credentials = MemoryCredentials::default();
        let profiles: Vec<_> = names.iter().map(|name| self.profile(name)).collect();
        let tabs = profiles
            .iter()
            .enumerate()
            .map(|(index, profile)| {
                credentials.set_password(profile.id, PASSWORD).unwrap();
                let mut tab = SavedTab::new(1, Some(profile.id));
                if index == 0 {
                    tab.sql = sql.into();
                }
                tab
            })
            .collect();
        (
            Workspace {
                profiles,
                tabs,
                ..Workspace::default()
            },
            credentials,
        )
    }
}

/// How often the executor recorded `token.state`: started, interrupted,
/// completed, or ended. The fixture keeps the evidence outside Qrow.
pub fn evidence(token: &str, state: &str) -> usize {
    super::evidence::count(token, state)
        .unwrap_or_else(|error| panic!("Could not read the evidence {token}.{state}: {error}"))
}

/// A query that blocks one executor task for `milliseconds` and records
/// evidence under `token`. Register `qrow_block` in the session first.
pub fn blocking(token: &str, milliseconds: u64) -> String {
    format!(
        "SELECT qrow_block(id, '{token}', CAST({milliseconds} AS BIGINT)) AS value FROM range(1)"
    )
}

pub const REGISTER_BLOCKING: &str =
    "CREATE TEMPORARY FUNCTION qrow_block AS 'io.qrow.fixture.Blocking'";

pub fn token(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

impl TestApp {
    /// Replaces the SQL of the active tab and runs it.
    pub fn run_sql(&self, cx: &mut TestAppContext, sql: &str) {
        self.type_sql(cx, sql);
        self.click(cx, "run");
    }

    /// Waits until the status bar starts with `prefix`, like "Complete".
    pub fn wait_status(&self, cx: &mut TestAppContext, prefix: &str) {
        self.wait_until(
            cx,
            &format!("the status {prefix}"),
            QUERY_TIMEOUT,
            |window, _| {
                label(window, "query-status").is_some_and(|status| status.starts_with(prefix))
            },
        );
    }

    /// Runs `sql` and waits until it completes.
    pub fn run_complete(&self, cx: &mut TestAppContext, sql: &str) {
        self.run_sql(cx, sql);
        self.wait_status(cx, "Complete");
    }

    /// Waits until the result cell shows `text`. Column 0 holds the row number.
    pub fn wait_cell(&self, cx: &mut TestAppContext, row: usize, column: usize, text: &str) {
        self.wait_until(
            cx,
            &format!("the cell {text}"),
            QUERY_TIMEOUT,
            |window, _| cell(window, row, column).as_deref() == Some(text),
        );
    }

    /// Selects a connection in the sidebar.
    pub fn select_connection(&self, cx: &mut TestAppContext, profile: &Profile) {
        self.click(cx, connection_row(profile.id));
        let expected = format!("Current connection: {}", profile.name);
        self.wait_until(cx, &expected, QUERY_TIMEOUT, |window, _| {
            label(window, "current-connection").as_deref() == Some(expected.as_str())
        });
    }

    /// The text of Logs, through Copy All Logs.
    pub fn logs(&self, cx: &mut TestAppContext) -> String {
        self.click(cx, "output-panel-tab");
        self.wait_for(cx, "output-copy-all");
        self.click(cx, "output-copy-all");
        self.settle(cx);
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        self.click(cx, "results-panel-tab");
        text
    }

    /// Waits until the executor recorded `token.state`.
    pub fn wait_evidence(
        &self,
        cx: &mut TestAppContext,
        token: &str,
        state: &str,
        timeout: Duration,
    ) {
        self.wait_until(cx, &format!("{token}.{state}"), timeout, |_, _| {
            evidence(token, state) >= 1
        });
    }
}
