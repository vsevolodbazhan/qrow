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

/// True when a published release of `channel` is in `releases`.
pub fn has_channel(releases: &[Release], channel: Channel) -> bool {
    releases.iter().any(|release| {
        !release.draft
            && Version::parse(&release.tag_name).is_some_and(|version| version.channel() == channel)
    })
}

/// The newest published release of the channel of `current` that is newer
/// than `current`. Drafts, other channels, and tags that do not match their
/// version are ignored.
pub fn newest(releases: &[Release], current: &Version) -> Option<Candidate> {
    releases
        .iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let version = Version::parse(&release.tag_name)?;
            let channel_matches = release.prerelease == (version.channel() == Channel::Nightly);
            (channel_matches && release.tag_name == version.tag() && version.is_newer_than(current))
                .then(|| Candidate {
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
        }
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
        let candidate = newest(&releases, &version("0.5.0")).unwrap();
        assert_eq!(candidate.version, version("0.6.0"));
        let nightly = newest(&releases, &version("0.5.0-nightly.20261101.1")).unwrap();
        assert_eq!(nightly.tag(), "v0.9.0-nightly.20261112.3");
    }

    #[test]
    fn ignores_the_current_and_older_releases() {
        let releases = [
            release("v0.5.0", false, false),
            release("v0.4.0", false, false),
        ];
        assert_eq!(newest(&releases, &version("0.5.0")), None);
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
        assert_eq!(newest(&releases, &version("0.5.0")), None);
        assert_eq!(
            newest(&releases, &version("0.5.0-nightly.20261101.1")),
            None
        );
    }

    #[test]
    fn reads_the_github_release_json() {
        let json = r#"[{"tag_name":"v0.6.0","draft":false,"prerelease":false,"name":"Qrow 0.6.0",
            "assets":[{"name":"SHA256SUMS","size":12,"browser_download_url":
            "https://github.com/vsevolodbazhan/qrow/releases/download/v0.6.0/SHA256SUMS"}]}]"#;
        let releases: Vec<Release> = serde_json::from_str(json).unwrap();
        let candidate = newest(&releases, &version("0.5.0")).unwrap();
        assert_eq!(candidate.asset("SHA256SUMS", DOWNLOADS).unwrap().size, 12);
    }

    #[test]
    fn accepts_only_assets_under_the_release_download_url() {
        let mut candidate = newest(&[release("v0.6.0", false, false)], &version("0.5.0")).unwrap();
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
}
