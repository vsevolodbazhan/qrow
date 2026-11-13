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
#[cfg(target_os = "macos")]
pub mod bundle;
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
/// Release pages to read before the check gives up. Each page has 100
/// releases, newest first.
const MAX_FEED_PAGES: usize = 10;
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
            releases: "https://api.github.com/repos/vsevolodbazhan/qrow/releases?per_page=100"
                .into(),
            downloads: "https://github.com/vsevolodbazhan/qrow/releases/download".into(),
            public_key: verify::RELEASE_PUBLIC_KEY.into(),
            https_only: true,
        }
    }
}

/// How this copy of Qrow updates: the release source, the running version,
/// the installation, and the folder for downloads.
#[derive(Clone, Debug)]
pub struct Setup {
    pub source: Source,
    pub current: Version,
    pub install: Install,
    /// `std::env::consts::ARCH` of this build.
    pub arch: String,
    /// The folder that holds downloads until they are installed.
    pub downloads: PathBuf,
}

impl Setup {
    /// The setup of the running release build, or `None` for a build without
    /// an update channel. `QROW_NO_UPDATES` turns updates off, for example
    /// for automated tests of a packaged release build.
    pub fn for_this_build(downloads: PathBuf) -> Option<Self> {
        if std::env::var_os("QROW_NO_UPDATES").is_some() {
            return None;
        }
        let current = running_version()?;
        let os = Os::current()?;
        let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        // Detection compares paths, so resolve them like the executable.
        let resolve = |path: PathBuf| path.canonicalize().unwrap_or(path);
        let homebrew_prefixes = ["/opt/homebrew", "/usr/local"].map(PathBuf::from);
        let homebrew_app_folders: Vec<_> = std::iter::once(PathBuf::from("/Applications"))
            .chain(home.map(|home| home.join("Applications")))
            .map(resolve)
            .collect();
        let install = install::detect(&install::Probe {
            os,
            channel: current.channel(),
            executable: &executable,
            env: &|name| std::env::var_os(name),
            homebrew_prefixes: &homebrew_prefixes,
            homebrew_app_folders: &homebrew_app_folders,
        });
        Some(Self {
            source: Source::github(),
            current,
            install,
            arch: std::env::consts::ARCH.into(),
            downloads,
        })
    }

    /// True when this copy can install an update itself. Other copies only
    /// report it.
    pub fn installs_updates(&self) -> bool {
        match self.install {
            Install::Archive { .. } => cfg!(target_os = "linux"),
            Install::AppBundle { .. } => cfg!(target_os = "macos"),
            Install::WindowsInstaller { .. } => cfg!(windows),
            Install::Managed(_) => false,
        }
    }

    /// The executable to start after an update, when it differs from the path
    /// that the system reports for the running process.
    pub fn restart_path(&self) -> Option<PathBuf> {
        match &self.install {
            // Linux reports a replaced executable as "PATH (deleted)".
            Install::Archive { prefix } => Some(prefix.join("bin/qrow")),
            _ => None,
        }
    }

    /// The web page of a release, for copies that the user updates by hand.
    pub fn release_page(&self, version: &Version) -> String {
        format!(
            "https://github.com/vsevolodbazhan/qrow/releases/tag/{}",
            version.tag()
        )
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
    pub fn check(&self, current: &Version, channel: Channel) -> Result<Option<Candidate>> {
        block_on(async {
            // The list is newest first. The release workflow only publishes a
            // commit that includes the previous release of its channel, so
            // within a channel a later release has a higher version. The first
            // page with a published release of the channel therefore has the
            // newest one. Releases of the other channel can fill whole pages
            // before it. A switch of channel also needs the release of
            // `current`, for its publish time.
            let switching = channel != current.channel();
            let mut releases: Vec<feed::Release> = Vec::new();
            let mut url = Some(self.source.releases.clone());
            for _ in 0..MAX_FEED_PAGES {
                let Some(page) = url.take() else { break };
                let (body, next) = fetch::page(&self.client, &page, MAX_FEED_BYTES).await?;
                let mut page: Vec<feed::Release> =
                    serde_json::from_slice(&body).context("Could not read the release list")?;
                releases.append(&mut page);
                let found_current = !switching || feed::published_at(&releases, current).is_some();
                if feed::has_channel(&releases, channel) && found_current {
                    break;
                }
                url = next;
            }
            Ok(feed::newest(&releases, current, channel))
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
        std::fs::create_dir_all(folder).context("Could not create the update folder")?;
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

/// What Qrow does when it quits after [`apply`].
#[derive(Debug)]
pub enum Finish {
    /// The new files are in place; start them.
    Restart,
    /// Run this installer, which replaces the files and starts Qrow.
    Run(std::process::Command),
}

/// Installs a verified download, or prepares its installer. The new version
/// starts after Qrow quits.
pub fn apply(download: &Download, install: &Install) -> Result<Finish> {
    match install {
        #[cfg(target_os = "linux")]
        Install::Archive { prefix } => {
            archive::install(&download.path, prefix)?;
            Ok(Finish::Restart)
        }
        #[cfg(target_os = "macos")]
        Install::AppBundle { app } => {
            bundle::install(&download.path, app)?;
            Ok(Finish::Restart)
        }
        #[cfg(windows)]
        Install::WindowsInstaller { folder } => {
            // The installer runs after Qrow quits, when the temporary download
            // is gone, so it moves next to that folder. The next launch
            // removes it.
            let name = download
                .path
                .file_name()
                .context("The installer has no name")?;
            let kept = download
                .path
                .parent()
                .and_then(Path::parent)
                .context("The installer has no download folder")?
                .join(name);
            std::fs::copy(&download.path, &kept).context("Could not keep the installer")?;
            let all_users = !install::writable(folder);
            Ok(Finish::Run(installer_command(&kept, folder, all_users)))
        }
        Install::Managed(_) => bail!("Another program updates this copy of Qrow"),
        // Only some platforms install this kind of copy; elsewhere the
        // download is not used.
        _ => {
            let _ = download;
            bail!("Updates of this installation are not available yet")
        }
    }
}

/// The silent run of the Windows installer `setup` into `folder`.
/// `/QROWRELAUNCH=1` makes it start Qrow when it is done. An install for all
/// users asks Windows for administrator rights.
pub fn installer_command(setup: &Path, folder: &Path, all_users: bool) -> std::process::Command {
    let mut command = std::process::Command::new(setup);
    command.args([
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/CLOSEAPPLICATIONS",
        "/QROWRELAUNCH=1",
        if all_users {
            "/ALLUSERS"
        } else {
            "/CURRENTUSER"
        },
    ]);
    command.arg(format!(
        "/DIR={}",
        without_verbatim_prefix(folder).display()
    ));
    command
}

/// `path` without the Windows verbatim prefix that `canonicalize` adds, as
/// in `\\?\C:\Qrow` or `\\?\UNC\server\share`. Inno Setup rejects a
/// folder with `?` in its name, so the installer quits without installing.
fn without_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{share}").into();
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.into(),
        _ => path.into(),
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
