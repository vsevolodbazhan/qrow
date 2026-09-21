//! Identity of the running build, reported by the About dialog.

/// The package version from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The version shown to users. Release builds can include a channel label.
pub const RELEASE_VERSION: &str = env!("QROW_RELEASE_VERSION");

/// The short Git commit recorded at build time. Empty for a build made outside
/// a checkout and without `QROW_COMMIT`.
pub const COMMIT: &str = env!("QROW_COMMIT");

/// The copyright holder of Qrow. Keep it equal to the [LICENSE](../LICENSE).
pub const COPYRIGHT: &str = "Copyright © 2026 Vsevolod Bazhan";

/// The release version with its commit, for example
/// `0.1.0-nightly.20260921.7 (dcc75d4f1a2b)`. A build without a known commit
/// reports the version alone.
pub fn version_label() -> String {
    label(RELEASE_VERSION, COMMIT)
}

fn label(version: &str, commit: &str) -> String {
    if commit.is_empty() {
        version.to_owned()
    } else {
        format!("{version} ({commit})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_includes_a_known_commit() {
        assert_eq!(label("0.1.0", "dcc75d4f1a2b"), "0.1.0 (dcc75d4f1a2b)");
    }

    #[test]
    fn label_falls_back_to_the_version_alone() {
        assert_eq!(label("0.1.0", ""), "0.1.0");
    }

    #[test]
    fn label_includes_a_nightly_release_version() {
        assert_eq!(
            label("0.1.0-nightly.20260921.7", "dcc75d4fd874"),
            "0.1.0-nightly.20260921.7 (dcc75d4fd874)"
        );
    }

    #[test]
    fn build_reports_a_version_and_a_plausible_commit() {
        assert!(!VERSION.is_empty());
        assert!(!RELEASE_VERSION.is_empty());
        assert!(version_label().starts_with(RELEASE_VERSION));
        assert!(
            COMMIT.is_empty()
                || (COMMIT.len() == 12 && COMMIT.chars().all(|c| c.is_ascii_hexdigit())),
            "unexpected commit: {COMMIT}"
        );
    }
}
