//! Read-only connectivity check using a profile already saved by the application.
use anyhow::{Context, Result};
use qrow::{
    connector::{Connector, QueryState, hive::HiveConnector},
    storage,
};
use std::{
    thread,
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    let name = std::env::args()
        .nth(1)
        .context("Usage: qrow-probe <saved-profile-name>")?;
    let workspace = storage::load(&storage::workspace_path())?;
    let profiles: Vec<_> = workspace
        .profiles
        .iter()
        .filter(|p| p.name == name)
        .collect();
    anyhow::ensure!(
        profiles.len() == 1,
        "Expected exactly one saved profile named {name}"
    );
    let profile = profiles[0];
    let mut session = HiveConnector.connect(profile, storage::password(profile.id)?)?;
    let result = (|| -> Result<()> {
        let cancel = session.execute("SELECT 1 AS qrow_connection_test")?;
        let started = Instant::now();
        loop {
            match session.poll()? {
                QueryState::Finished { has_results: true } => break,
                QueryState::Running if started.elapsed() < Duration::from_secs(60) => {
                    thread::sleep(Duration::from_millis(100))
                }
                state => {
                    let _ = cancel.cancel();
                    anyhow::bail!("Connectivity check did not complete: {state:?}");
                }
            }
        }
        let columns = session.columns()?;
        let batch = session.fetch(10)?;
        anyhow::ensure!(
            columns.len() == 1 && batch.rows == vec![vec![Some("1".into())]],
            "Unexpected SELECT 1 result"
        );
        println!("Connected. Session initialized and SELECT 1 returned the expected result.");
        Ok(())
    })();
    let cleanup = session.close();
    result?;
    cleanup
}
