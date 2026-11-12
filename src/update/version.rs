//! Release versions and their order within a channel.

use std::{cmp::Ordering, fmt};

/// The release channel of a build or of the update setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Stable,
    Nightly,
}

/// A release version: `MAJOR.MINOR.PATCH` for stable releases and
/// `MAJOR.MINOR.PATCH-nightly.YYYYMMDD.RUN` for nightly releases.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Version {
    core: [u64; 3],
    /// The build date and the workflow run number of a nightly release.
    nightly: Option<(u64, u64)>,
}

impl Version {
    /// Reads a version with an optional `v` prefix, for example a release tag.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let (core, suffix) = match text.split_once('-') {
            Some((core, suffix)) => (core, Some(suffix)),
            None => (text, None),
        };
        let mut parts = core.split('.').map(number);
        let core = [parts.next()??, parts.next()??, parts.next()??];
        if parts.next().is_some() {
            return None;
        }
        let nightly = match suffix {
            None => None,
            Some(suffix) => {
                let mut parts = suffix.split('.');
                if parts.next()? != "nightly" {
                    return None;
                }
                let date = parts
                    .next()
                    .filter(|date| date.len() == 8)
                    .and_then(number)?;
                let run = parts.next().and_then(number)?;
                if parts.next().is_some() {
                    return None;
                }
                Some((date, run))
            }
        };
        Some(Self { core, nightly })
    }

    pub fn channel(&self) -> Channel {
        if self.nightly.is_some() {
            Channel::Nightly
        } else {
            Channel::Stable
        }
    }

    /// True when `self` is a later release of the same channel. Versions of
    /// different channels are never newer than each other.
    pub fn is_newer_than(&self, other: &Self) -> bool {
        self.channel() == other.channel() && self.cmp(other) == Ordering::Greater
    }

    /// The `MAJOR.MINOR.PATCH` part.
    pub fn core(&self) -> [u64; 3] {
        self.core
    }

    /// The tag of the GitHub release, for example `v0.5.0`.
    pub fn tag(&self) -> String {
        format!("v{self}")
    }
}

/// Only ASCII digits, without a sign or leading zeros, except for zero.
fn number(text: &str) -> Option<u64> {
    let valid = !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    valid.then(|| text.parse().ok()).flatten()
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| self.nightly.cmp(&other.nightly))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.core;
        write!(f, "{major}.{minor}.{patch}")?;
        if let Some((date, run)) = self.nightly {
            write!(f, "-nightly.{date}.{run}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("invalid version {text}"))
    }

    #[test]
    fn reads_stable_and_nightly_versions_and_tags() {
        assert_eq!(version("0.5.0").channel(), Channel::Stable);
        assert_eq!(version("v0.5.0").to_string(), "0.5.0");
        let nightly = version("v0.5.0-nightly.20261112.42");
        assert_eq!(nightly.channel(), Channel::Nightly);
        assert_eq!(nightly.to_string(), "0.5.0-nightly.20261112.42");
        assert_eq!(nightly.tag(), "v0.5.0-nightly.20261112.42");
    }

    #[test]
    fn rejects_other_version_forms() {
        for text in [
            "",
            "v",
            "0.5",
            "0.5.0.1",
            "0.5.x",
            "01.5.0",
            "+1.0.0",
            "0.5.0-beta.1",
            "0.5.0-nightly",
            "0.5.0-nightly.2026111.1",
            "0.5.0-nightly.20261112",
            "0.5.0-nightly.20261112.1.2",
            "0.5.0-nightly.20261112.-1",
            "0.5.0 ",
        ] {
            assert_eq!(Version::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn orders_versions_numerically_within_a_channel() {
        assert!(version("0.10.0").is_newer_than(&version("0.9.9")));
        assert!(version("1.0.0").is_newer_than(&version("0.99.99")));
        assert!(!version("0.5.0").is_newer_than(&version("0.5.0")));
        assert!(!version("0.4.9").is_newer_than(&version("0.5.0")));
        let older = version("0.5.0-nightly.20261111.9");
        assert!(version("0.5.0-nightly.20261112.1").is_newer_than(&older));
        assert!(version("0.5.0-nightly.20261111.10").is_newer_than(&older));
        assert!(version("0.6.0-nightly.20261101.1").is_newer_than(&older));
    }

    #[test]
    fn never_moves_between_channels() {
        let stable = version("0.5.0");
        let nightly = version("0.6.0-nightly.20261112.1");
        assert!(!nightly.is_newer_than(&stable));
        assert!(!version("1.0.0").is_newer_than(&nightly));
    }
}
