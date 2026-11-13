//! The GitHub releases of Qrow and the choice of an update.

use super::version::{Channel, Version};
use serde::Deserialize;

/// A release from the GitHub releases API. Unknown fields are ignored.
#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
    /// When the release was published, as RFC 3339 UTC text, for example
    /// `2026-11-12T04:13:03Z`. Text order is time order.
    #[serde(default)]
    pub published_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

/// A published release that is newer than the running build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub version: Version,
    pub assets: Vec<Asset>,
}

impl Candidate {
    pub fn tag(&self) -> String {
        self.version.tag()
    }

    /// The asset with this name. Its URL must be a download of this release
    /// under `downloads`, so a changed feed cannot send Qrow elsewhere.
    pub fn asset(&self, name: &str, downloads: &str) -> Option<&Asset> {
        let prefix = format!("{}/{}/", downloads.trim_end_matches('/'), self.tag());
        self.assets.iter().find(|asset| {
            asset.name == name && asset.browser_download_url == format!("{prefix}{name}")
        })
    }
}

/// The version of a published release on `channel`: not a draft, a
/// canonical tag, and the prerelease flag of its channel.
fn published_version(release: &Release, channel: Channel) -> Option<Version> {
    let version = Version::parse(&release.tag_name)?;
    let valid = !release.draft
        && version.channel() == channel
        && release.prerelease == (channel == Channel::Nightly)
        && release.tag_name == version.tag();
    valid.then_some(version)
}

/// True when `releases` has a published release of `channel`.
pub fn has_channel(releases: &[Release], channel: Channel) -> bool {
    releases
        .iter()
        .any(|release| published_version(release, channel).is_some())
}

/// When the release of `current` was published, if `releases` has it.
pub fn published_at(releases: &[Release], current: &Version) -> Option<String> {
    releases
        .iter()
        .find(|release| published_version(release, current.channel()).as_ref() == Some(current))
        .and_then(|release| release.published_at.clone())
}

/// The newest published release of `channel` that is newer than `current`.
/// Drafts and tags that do not match their version are ignored.
///
/// In the channel of `current`, a higher version is newer. A release of the
/// other channel is newer only when it was published after the release of
/// `current`, because a stable and a nightly release of one version can come
/// in either order. So a switch never installs an older build. Without the
/// publish times, only a higher `MAJOR.MINOR.PATCH` counts.
pub fn newest(releases: &[Release], current: &Version, channel: Channel) -> Option<Candidate> {
    let switching = channel != current.channel();
    let since = switching.then(|| published_at(releases, current)).flatten();
    releases
        .iter()
        .filter_map(|release| {
            let version = published_version(release, channel)?;
            let newer = if !switching {
                version.is_newer_than(current)
            } else {
                match (&since, &release.published_at) {
                    (Some(since), Some(at)) => at > since,
                    _ => version.core() > current.core(),
                }
            };
            newer.then(|| Candidate {
                version,
                assets: release.assets.clone(),
            })
        })
        .max_by(|left, right| left.version.cmp(&right.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOWNLOADS: &str = "https://github.com/vsevolodbazhan/qrow/releases/download";

    fn release(tag: &str, prerelease: bool, draft: bool) -> Release {
        Release {
            tag_name: tag.into(),
            draft,
            prerelease,
            assets: vec![Asset {
                name: "SHA256SUMS".into(),
                browser_download_url: format!("{DOWNLOADS}/{tag}/SHA256SUMS"),
                size: 10,
            }],
            published_at: None,
        }
    }

    /// A release published at `at`, for example `2026-11-12T04:00:00Z`.
    fn published(tag: &str, at: &str) -> Release {
        let nightly = tag.contains("-nightly.");
        Release {
            published_at: Some(at.into()),
            ..release(tag, nightly, false)
        }
    }

    fn newest_same(releases: &[Release], current: &Version) -> Option<Candidate> {
        newest(releases, current, current.channel())
    }

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn chooses_the_newest_release_of_the_same_channel() {
        let releases = [
            release("v0.4.0", false, false),
            release("v0.6.0", false, false),
            release("v0.5.1", false, false),
            release("v0.7.0", false, true),
            release("v0.9.0-nightly.20261112.3", true, false),
        ];
        let candidate = newest_same(&releases, &version("0.5.0")).unwrap();
        assert_eq!(candidate.version, version("0.6.0"));
        let nightly = newest_same(&releases, &version("0.5.0-nightly.20261101.1")).unwrap();
        assert_eq!(nightly.tag(), "v0.9.0-nightly.20261112.3");
    }

    #[test]
    fn ignores_the_current_and_older_releases() {
        let releases = [
            release("v0.5.0", false, false),
            release("v0.4.0", false, false),
        ];
        assert_eq!(newest_same(&releases, &version("0.5.0")), None);
    }

    #[test]
    fn ignores_releases_whose_flags_or_tags_do_not_match_their_version() {
        let releases = [
            // A stable version marked as a prerelease, and the reverse.
            release("v0.6.0", true, false),
            release("v0.6.0-nightly.20261112.1", false, false),
            // Tags that read as versions but are not the canonical tag.
            release("0.7.0", false, false),
            release("v0.8.0-beta.1", true, false),
        ];
        assert_eq!(newest_same(&releases, &version("0.5.0")), None);
        assert_eq!(
            newest_same(&releases, &version("0.5.0-nightly.20261101.1")),
            None
        );
    }

    #[test]
    fn only_published_releases_of_the_channel_count_as_the_channel() {
        let stable = Channel::Stable;
        assert!(has_channel(&[release("v0.4.0", false, false)], stable));
        for ignored in [
            release("v0.4.0", false, true),
            release("v0.4.0", true, false),
            release("0.4.0", false, false),
            release("v0.4.0-nightly.20261112.1", true, false),
        ] {
            assert!(
                !has_channel(std::slice::from_ref(&ignored), stable),
                "{}",
                ignored.tag_name
            );
        }
    }

    #[test]
    fn reads_the_github_release_json() {
        let json = r#"[{"tag_name":"v0.6.0","draft":false,"prerelease":false,"name":"Qrow 0.6.0",
            "assets":[{"name":"SHA256SUMS","size":12,"browser_download_url":
            "https://github.com/vsevolodbazhan/qrow/releases/download/v0.6.0/SHA256SUMS"}]}]"#;
        let releases: Vec<Release> = serde_json::from_str(json).unwrap();
        let candidate = newest_same(&releases, &version("0.5.0")).unwrap();
        assert_eq!(candidate.asset("SHA256SUMS", DOWNLOADS).unwrap().size, 12);
    }

    #[test]
    fn accepts_only_assets_under_the_release_download_url() {
        let mut candidate =
            newest_same(&[release("v0.6.0", false, false)], &version("0.5.0")).unwrap();
        assert!(candidate.asset("SHA256SUMS", DOWNLOADS).is_some());
        assert!(
            candidate
                .asset("SHA256SUMS", "https://example.com/download")
                .is_none()
        );
        candidate.assets[0].browser_download_url = format!("{DOWNLOADS}/v0.5.0/SHA256SUMS");
        assert!(candidate.asset("SHA256SUMS", DOWNLOADS).is_none());
        assert!(candidate.asset("Other", DOWNLOADS).is_none());
    }

    #[test]
    fn a_switch_to_nightly_takes_the_newest_nightly_published_after_this_release() {
        let releases = [
            published("v0.5.0-nightly.20261113.3", "2026-11-13T04:00:00Z"),
            published("v0.5.0", "2026-11-12T10:00:00Z"),
            // Built before the stable release of the same version: older.
            published("v0.5.0-nightly.20261111.1", "2026-11-11T04:00:00Z"),
        ];
        let candidate = newest(&releases, &version("0.5.0"), Channel::Nightly).unwrap();
        assert_eq!(candidate.tag(), "v0.5.0-nightly.20261113.3");
        let only_older = [releases[1].clone(), releases[2].clone()];
        assert_eq!(
            newest(&only_older, &version("0.5.0"), Channel::Nightly),
            None
        );
    }

    #[test]
    fn a_switch_to_stable_waits_for_a_stable_release_after_this_nightly() {
        let current = version("0.5.0-nightly.20261113.3");
        let mut releases = vec![
            published("v0.5.0-nightly.20261113.3", "2026-11-13T04:00:00Z"),
            published("v0.5.0", "2026-11-12T10:00:00Z"),
        ];
        // The stable release is older than this nightly: wait.
        assert_eq!(newest(&releases, &current, Channel::Stable), None);
        releases.insert(0, published("v0.5.1", "2026-11-20T10:00:00Z"));
        let candidate = newest(&releases, &current, Channel::Stable).unwrap();
        assert_eq!(candidate.tag(), "v0.5.1");
    }

    #[test]
    fn without_publish_times_a_switch_needs_a_higher_base_version() {
        let releases = [
            release("v0.5.0-nightly.20261113.3", true, false),
            release("v0.6.0-nightly.20261120.1", true, false),
        ];
        let candidate = newest(&releases, &version("0.5.0"), Channel::Nightly).unwrap();
        assert_eq!(candidate.tag(), "v0.6.0-nightly.20261120.1");
        assert_eq!(
            newest(&releases[..1], &version("0.5.0"), Channel::Nightly),
            None
        );
    }

    #[test]
    fn reads_the_publish_time_of_the_running_release() {
        let releases = [published("v0.5.0", "2026-11-12T10:00:00Z")];
        assert_eq!(
            published_at(&releases, &version("0.5.0")).as_deref(),
            Some("2026-11-12T10:00:00Z")
        );
        assert_eq!(published_at(&releases, &version("0.4.0")), None);
    }
}
