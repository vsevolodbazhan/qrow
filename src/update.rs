//! Updates of release builds from the GitHub releases of Qrow.
//!
//! A release build follows its channel. It reads the release list, chooses
//! the newest release of its channel, and downloads the signed checksums of
//! that release. It then downloads the asset for its installation and
//! compares the digest with the verified checksum. Only then can the asset
//! replace the installed files. A copy that a package manager installed only
//! reports the update.

#[cfg(target_os = "linux")]
pub mod archive;
pub mod feed;
mod fetch;
pub mod install;
pub mod verify;
pub mod version;

pub use feed::Candidate;
pub use install::{Install, Manager, Os};
pub use version::{Channel, Version};

use anyhow::{Context, Result, bail};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_FEED_BYTES: u64 = 4 << 20;
const MAX_CHECKSUM_BYTES: u64 = 64 << 10;
const MAX_ASSET_BYTES: u64 = 1 << 30;

/// The channel of this build. Only builds of the release workflow have one;
/// other builds never check for updates.
pub fn build_channel() -> Option<Channel> {
    match env!("QROW_UPDATE_CHANNEL") {
        "stable" => Some(Channel::Stable),
        "nightly" => Some(Channel::Nightly),
        _ => None,
    }
}

/// The version of this build when it can update: a release build whose
/// version belongs to its channel.
pub fn running_version() -> Option<Version> {
    let version = Version::parse(crate::build_info::RELEASE_VERSION)?;
    (Some(version.channel()) == build_channel()).then_some(version)
}

/// Where the releases come from, and the key that signs them.
#[derive(Clone, Debug)]
pub struct Source {
    /// The GitHub API URL of the release list.
    pub releases: String,
    /// The URL prefix of release downloads; the tag and the asset name follow.
    pub downloads: String,
    /// The minisign public key of the release key.
    pub public_key: String,
    /// Refuse plain HTTP. Only tests with a local server turn this off.
    pub https_only: bool,
}

impl Source {
    pub fn github() -> Self {
        Self {
            releases: "https://api.github.com/repos/vsevolodbazhan/qrow/releases?per_page=30"
                .into(),
            downloads: "https://github.com/vsevolodbazhan/qrow/releases/download".into(),
            public_key: verify::RELEASE_PUBLIC_KEY.into(),
            https_only: true,
        }
    }
}

/// A verified update asset in a temporary folder. Dropping it removes the file.
#[derive(Debug)]
pub struct Download {
    pub version: Version,
    pub path: PathBuf,
    _folder: tempfile::TempDir,
}

/// Checks for updates and downloads them. Each call blocks its thread.
pub struct Updater {
    client: reqwest::Client,
    source: Source,
}

impl Updater {
    pub fn new(source: Source, trust: &crate::tls::Trust) -> Result<Self> {
        let client = reqwest::Client::builder()
            .use_preconfigured_tls((*trust.client_config()?).clone())
            .https_only(source.https_only)
            .user_agent(format!("Qrow/{}", crate::build_info::RELEASE_VERSION))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .build()
            .context("Could not prepare the update client")?;
        Ok(Self { client, source })
    }

    /// The newest release of the channel of `current` that is newer than
    /// `current`, or `None`.
    pub fn check(&self, current: &Version) -> Result<Option<Candidate>> {
        block_on(async {
            let body = fetch::bytes(&self.client, &self.source.releases, MAX_FEED_BYTES).await?;
            let releases: Vec<feed::Release> =
                serde_json::from_slice(&body).context("Could not read the release list")?;
            Ok(feed::newest(&releases, current))
        })
    }

    /// Downloads the asset of `candidate` for `install` into a new folder in
    /// `folder`, and verifies it with the signed checksums of the release.
    pub fn download(
        &self,
        candidate: &Candidate,
        install: &Install,
        arch: &str,
        folder: &Path,
    ) -> Result<Download> {
        let Some(name) = install.asset_name(&candidate.version, arch) else {
            bail!("No release package fits this installation");
        };
        let tag = candidate.tag();
        let asset = |name: &str| {
            candidate
                .asset(name, &self.source.downloads)
                .with_context(|| format!("Release {tag} has no {name}"))
        };
        let (sums, signature, package) = (
            asset("SHA256SUMS")?,
            asset("SHA256SUMS.minisig")?,
            asset(&name)?,
        );
        let folder = tempfile::Builder::new()
            .prefix("qrow-update-")
            .tempdir_in(folder)
            .context("Could not create a folder for the update")?;
        let path = folder.path().join(&name);
        block_on(async {
            let sums =
                fetch::bytes(&self.client, &sums.browser_download_url, MAX_CHECKSUM_BYTES).await?;
            let signature = fetch::bytes(
                &self.client,
                &signature.browser_download_url,
                MAX_CHECKSUM_BYTES,
            )
            .await?;
            let signature =
                String::from_utf8(signature).context("Could not read the release signature")?;
            let checksums =
                verify::verify_checksums(&sums, &signature, &self.source.public_key, &tag)?;
            let expected = *checksums
                .digest(&name)
                .with_context(|| format!("The signed checksums of {tag} do not list {name}"))?;
            anyhow::ensure!(
                package.size <= MAX_ASSET_BYTES,
                "The update package is too large"
            );
            let mut output =
                std::fs::File::create_new(&path).context("Could not save the update")?;
            let digest = fetch::file(
                &self.client,
                &package.browser_download_url,
                package.size,
                &mut output,
            )
            .await?;
            anyhow::ensure!(
                digest == expected,
                "The update package does not match its signed checksum"
            );
            Ok(())
        })?;
        Ok(Download {
            version: candidate.version.clone(),
            path,
            _folder: folder,
        })
    }
}

/// Installs a verified download. The new version starts at the next launch.
pub fn apply(download: &Download, install: &Install) -> Result<()> {
    match install {
        #[cfg(target_os = "linux")]
        Install::Archive { prefix } => archive::install(&download.path, prefix),
        Install::Managed(_) => bail!("Another program updates this copy of Qrow"),
        _ => bail!("Updates of this installation are not available yet"),
    }
}

fn block_on<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Could not start the update client")?
        .block_on(future)
}

#[cfg(test)]
mod tests;
