//! Read-only connectivity check using a profile already saved by the application.
use anyhow::{Context, Result};
use qrow::{
    connector::{Completion, Connector, DatabaseConnector, wait_for_completion},
    model::Authentication,
    storage::{self, Credentials, Keychain},
};
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut workspace_name = None;
    let mut profile_name = None;
    while let Some(arg) = args.next() {
        if arg == "--workspace" {
            workspace_name = Some(
                args.next()
                    .context("Usage: qrow-probe [--workspace <name>] <saved-profile-name>")?,
            );
        } else if profile_name.is_none() {
            profile_name = Some(arg);
        } else {
            anyhow::bail!("Usage: qrow-probe [--workspace <name>] <saved-profile-name>");
        }
    }
    let name =
        profile_name.context("Usage: qrow-probe [--workspace <name>] <saved-profile-name>")?;
    let data_dir = storage::data_directory();
    let workspace_path = match workspace_name {
        Some(ws) => storage::resolve_workspace_path_by_name(&data_dir, &ws)?,
        None => storage::resolve_active_workspace_path(&data_dir)?,
    };
    let workspace = storage::load(&workspace_path)?;
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
    // A refresh replaces the refresh token in Keychain. Qrow does not
    // coordinate that between processes, so the probe uses passwords only.
    anyhow::ensure!(
        profile.authentication == Authentication::Password,
        "qrow-probe supports only connections with password authentication"
    );
    let mut session =
        DatabaseConnector::default().connect(profile, Keychain.password(profile.id)?.into())?;
    let result = (|| -> Result<()> {
        let cancel = session.execute("SELECT 1 AS qrow_connection_test")?;
        let deadline = Instant::now() + Duration::from_secs(60);
        match wait_for_completion(session.as_mut(), Some(deadline)) {
            Ok(Completion::Finished { has_results: true }) => {}
            state => {
                let _ = cancel.cancel();
                anyhow::bail!("Connectivity check did not complete: {state:?}");
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
