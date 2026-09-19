//! Records the Git commit of the build so the application can report it.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=QROW_COMMIT");
    // A packager without Git history supplies the commit through the
    // environment. A checkout resolves it here.
    let commit = std::env::var("QROW_COMMIT")
        .ok()
        .filter(|commit| !commit.trim().is_empty())
        .or_else(commit_from_git)
        .unwrap_or_default();
    println!("cargo:rustc-env=QROW_COMMIT={}", commit.trim());
}

/// The short commit of `HEAD`, or `None` outside a Git checkout.
fn commit_from_git() -> Option<String> {
    // Cargo must run this script again when HEAD or any reference moves.
    for name in ["HEAD", "refs", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name])
            && Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    git(&["rev-parse", "--short=12", "HEAD"])
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}
