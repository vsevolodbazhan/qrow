//! How the running copy of Qrow was installed, and so how it can update.

use super::version::{Channel, Version};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// A file that a packager puts next to the executable, or in the
/// `Contents/Resources` folder of the macOS app, to stop self-updates.
pub const NO_SELF_UPDATE_MARKER: &str = "qrow-no-self-update";

/// The operating systems of release builds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Os {
    MacOs,
    Linux,
    Windows,
}

impl Os {
    pub fn current() -> Option<Self> {
        match std::env::consts::OS {
            "macos" => Some(Self::MacOs),
            "linux" => Some(Self::Linux),
            "windows" => Some(Self::Windows),
            _ => None,
        }
    }
}

/// The installation of the running copy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Install {
    /// The Linux release archive, extracted into a writable prefix such as
    /// `~/.local`. Qrow replaces the files.
    Archive { prefix: PathBuf },
    /// The app from a macOS DMG, in a writable folder. Qrow replaces the app.
    AppBundle { app: PathBuf },
    /// The Windows installer. Qrow runs the installer of the new version.
    WindowsInstaller { folder: PathBuf },
    /// Another program owns the files. Qrow only reports the update.
    Managed(Manager),
}

/// Who updates a copy that Qrow does not update itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Manager {
    Flatpak,
    Snap,
    Homebrew {
        cask: String,
    },
    Scoop {
        app: String,
    },
    /// The Windows portable archive. The user downloads the new archive.
    WindowsPortable,
    /// A system or packager installation, or a folder the user cannot write.
    Other,
}

impl Install {
    pub fn updates_itself(&self) -> bool {
        !matches!(self, Self::Managed(_))
    }

    /// The release asset that updates this installation on `arch`
    /// (`std::env::consts::ARCH`), or `None` when no asset fits.
    pub fn asset_name(&self, version: &Version, arch: &str) -> Option<String> {
        let (linux_or_windows, mac) = match arch {
            "x86_64" => ("x86_64", "x86_64"),
            "aarch64" => ("aarch64", "arm64"),
            _ => return None,
        };
        match self {
            Self::Archive { .. } => Some(format!("Qrow-{version}-linux-{linux_or_windows}.tar.gz")),
            Self::AppBundle { .. } => Some(format!("Qrow-{version}-{mac}.dmg")),
            Self::WindowsInstaller { .. } => Some(format!(
                "Qrow-{version}-windows-{linux_or_windows}-setup.exe"
            )),
            Self::Managed(Manager::WindowsPortable) => {
                Some(format!("Qrow-{version}-windows-{linux_or_windows}.zip"))
            }
            Self::Managed(_) => None,
        }
    }
}

/// The facts that decide the installation kind.
pub struct Probe<'a> {
    pub os: Os,
    pub channel: Channel,
    /// The resolved path of the running executable.
    pub executable: &'a Path,
    pub env: &'a dyn Fn(&str) -> Option<OsString>,
    /// Folders that can hold Homebrew casks, for example `/opt/homebrew`.
    pub homebrew_prefixes: &'a [PathBuf],
    /// Folders where Homebrew puts the apps of casks: `/Applications` and
    /// `~/Applications`.
    pub homebrew_app_folders: &'a [PathBuf],
}

/// Decides how the running copy was installed.
pub fn detect(probe: &Probe) -> Install {
    if (probe.env)("FLATPAK_ID").is_some() {
        return Install::Managed(Manager::Flatpak);
    }
    if (probe.env)("SNAP").is_some() {
        return Install::Managed(Manager::Snap);
    }
    let Some(folder) = probe.executable.parent() else {
        return Install::Managed(Manager::Other);
    };
    match probe.os {
        Os::MacOs => detect_macos(probe, folder),
        Os::Linux => detect_linux(folder),
        Os::Windows => detect_windows(probe.executable, folder),
    }
}

fn detect_macos(probe: &Probe, folder: &Path) -> Install {
    // The executable is Qrow.app/Contents/MacOS/qrow.
    let app = folder
        .parent()
        .filter(|contents| contents.file_name() == Some("Contents".as_ref()))
        .and_then(Path::parent)
        .filter(|app| app.extension() == Some("app".as_ref()));
    let Some(app) = app else {
        return Install::Managed(Manager::Other);
    };
    if app
        .join("Contents/Resources")
        .join(NO_SELF_UPDATE_MARKER)
        .exists()
    {
        return Install::Managed(Manager::Other);
    }
    let cask = match probe.channel {
        Channel::Stable => "qrow",
        Channel::Nightly => "qrow@nightly",
    };
    // Both casks install Qrow.app into the Homebrew app folder. A copy in
    // another folder is a separate download, even when the cask exists.
    let in_app_folder = probe
        .homebrew_app_folders
        .iter()
        .any(|folder| app == folder.join("Qrow.app"));
    if in_app_folder
        && probe
            .homebrew_prefixes
            .iter()
            .any(|prefix| prefix.join("Caskroom").join(cask).is_dir())
    {
        return Install::Managed(Manager::Homebrew { cask: cask.into() });
    }
    match app.parent() {
        Some(parent) if writable(parent) && writable(app) => Install::AppBundle { app: app.into() },
        _ => Install::Managed(Manager::Other),
    }
}

fn detect_linux(folder: &Path) -> Install {
    // The archive puts the executable in PREFIX/bin and the launcher in
    // PREFIX/share/applications.
    let prefix = folder
        .file_name()
        .filter(|name| *name == "bin")
        .and(folder.parent());
    match prefix {
        Some(prefix)
            if !folder.join(NO_SELF_UPDATE_MARKER).exists()
                && prefix.join("share/applications/qrow.desktop").is_file()
                && writable(folder)
                && writable(&prefix.join("share")) =>
        {
            Install::Archive {
                prefix: prefix.into(),
            }
        }
        _ => Install::Managed(Manager::Other),
    }
}

fn detect_windows(executable: &Path, folder: &Path) -> Install {
    let components: Vec<_> = executable
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    if let Some(index) = components
        .windows(2)
        .position(|pair| pair[0] == "scoop" && pair[1] == "apps")
        && let Some(app) = components.get(index + 2)
    {
        return Install::Managed(Manager::Scoop { app: app.clone() });
    }
    if folder.join(NO_SELF_UPDATE_MARKER).exists() {
        return Install::Managed(Manager::Other);
    }
    // Inno Setup puts its uninstaller next to the executable.
    if folder.join("unins000.exe").is_file() {
        return Install::WindowsInstaller {
            folder: folder.into(),
        };
    }
    Install::Managed(Manager::WindowsPortable)
}

/// True when the current user can create files in `folder`.
pub(crate) fn writable(folder: &Path) -> bool {
    tempfile::Builder::new()
        .prefix(".qrow-write-check")
        .tempfile_in(folder)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn detect_in(
        os: Os,
        channel: Channel,
        executable: &Path,
        homebrew: &[PathBuf],
        apps: &[PathBuf],
    ) -> Install {
        detect(&Probe {
            os,
            channel,
            executable,
            env: &|_| None,
            homebrew_prefixes: homebrew,
            homebrew_app_folders: apps,
        })
    }

    fn file(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn flatpak_and_snap_manage_their_copies() {
        let executable = Path::new("/app/bin/qrow");
        for (name, manager) in [("FLATPAK_ID", Manager::Flatpak), ("SNAP", Manager::Snap)] {
            let install = detect(&Probe {
                os: Os::Linux,
                channel: Channel::Stable,
                executable,
                env: &|key| (key == name).then(|| "set".into()),
                homebrew_prefixes: &[],
                homebrew_app_folders: &[],
            });
            assert_eq!(install, Install::Managed(manager));
        }
    }

    #[test]
    fn a_linux_archive_in_a_writable_prefix_updates_itself() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join(".local");
        file(&prefix.join("bin/qrow"));
        let executable = prefix.join("bin/qrow");
        assert_eq!(
            detect_in(Os::Linux, Channel::Stable, &executable, &[], &[]),
            Install::Managed(Manager::Other),
            "a prefix without the launcher is not the release archive"
        );
        file(&prefix.join("share/applications/qrow.desktop"));
        assert_eq!(
            detect_in(Os::Linux, Channel::Stable, &executable, &[], &[]),
            Install::Archive {
                prefix: prefix.clone()
            }
        );
        file(&prefix.join("bin").join(NO_SELF_UPDATE_MARKER));
        assert_eq!(
            detect_in(Os::Linux, Channel::Stable, &executable, &[], &[]),
            Install::Managed(Manager::Other)
        );
        let elsewhere = root.path().join("opt/qrow");
        file(&elsewhere);
        assert_eq!(
            detect_in(Os::Linux, Channel::Stable, &elsewhere, &[], &[]),
            Install::Managed(Manager::Other)
        );
    }

    #[test]
    fn a_macos_app_updates_itself_unless_homebrew_owns_it() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("Applications/Qrow.app");
        let executable = app.join("Contents/MacOS/qrow");
        file(&executable);
        assert_eq!(
            detect_in(Os::MacOs, Channel::Stable, &executable, &[], &[]),
            Install::AppBundle { app: app.clone() }
        );
        let homebrew = root.path().join("homebrew");
        fs::create_dir_all(homebrew.join("Caskroom/qrow@nightly")).unwrap();
        let prefixes = [homebrew];
        let apps = [root.path().join("Applications")];
        assert_eq!(
            detect_in(Os::MacOs, Channel::Nightly, &executable, &prefixes, &apps),
            Install::Managed(Manager::Homebrew {
                cask: "qrow@nightly".into()
            })
        );
        // The nightly cask does not own a stable copy.
        assert_eq!(
            detect_in(Os::MacOs, Channel::Stable, &executable, &prefixes, &apps),
            Install::AppBundle { app: app.clone() }
        );
        // A copy outside the Homebrew app folder is a separate download.
        let downloaded = root.path().join("Downloads/Qrow.app");
        file(&downloaded.join("Contents/MacOS/qrow"));
        assert_eq!(
            detect_in(
                Os::MacOs,
                Channel::Nightly,
                &downloaded.join("Contents/MacOS/qrow"),
                &prefixes,
                &apps
            ),
            Install::AppBundle { app: downloaded }
        );
        file(&app.join("Contents/Resources").join(NO_SELF_UPDATE_MARKER));
        assert_eq!(
            detect_in(Os::MacOs, Channel::Stable, &executable, &[], &[]),
            Install::Managed(Manager::Other)
        );
        let loose = root.path().join("bin/qrow");
        file(&loose);
        assert_eq!(
            detect_in(Os::MacOs, Channel::Stable, &loose, &[], &[]),
            Install::Managed(Manager::Other)
        );
    }

    #[test]
    fn windows_tells_scoop_the_installer_and_the_portable_archive_apart() {
        let root = tempfile::tempdir().unwrap();
        let scoop = root
            .path()
            .join("Users/me/Scoop/apps/qrow-nightly/current/qrow.exe");
        file(&scoop);
        assert_eq!(
            detect_in(Os::Windows, Channel::Nightly, &scoop, &[], &[]),
            Install::Managed(Manager::Scoop {
                app: "qrow-nightly".into()
            })
        );
        let installed = root.path().join("Programs/Qrow");
        file(&installed.join("qrow.exe"));
        assert_eq!(
            detect_in(
                Os::Windows,
                Channel::Stable,
                &installed.join("qrow.exe"),
                &[],
                &[]
            ),
            Install::Managed(Manager::WindowsPortable)
        );
        file(&installed.join("unins000.exe"));
        assert_eq!(
            detect_in(
                Os::Windows,
                Channel::Stable,
                &installed.join("qrow.exe"),
                &[],
                &[]
            ),
            Install::WindowsInstaller {
                folder: installed.clone()
            }
        );
    }

    #[test]
    fn names_the_release_asset_of_each_installation() {
        let version = Version::parse("0.6.0-nightly.20261112.4").unwrap();
        let cases = [
            (
                Install::Archive {
                    prefix: "/p".into(),
                },
                "aarch64",
                "Qrow-0.6.0-nightly.20261112.4-linux-aarch64.tar.gz",
            ),
            (
                Install::AppBundle {
                    app: "/A.app".into(),
                },
                "aarch64",
                "Qrow-0.6.0-nightly.20261112.4-arm64.dmg",
            ),
            (
                Install::AppBundle {
                    app: "/A.app".into(),
                },
                "x86_64",
                "Qrow-0.6.0-nightly.20261112.4-x86_64.dmg",
            ),
            (
                Install::WindowsInstaller {
                    folder: "C:/Q".into(),
                },
                "x86_64",
                "Qrow-0.6.0-nightly.20261112.4-windows-x86_64-setup.exe",
            ),
            (
                Install::Managed(Manager::WindowsPortable),
                "aarch64",
                "Qrow-0.6.0-nightly.20261112.4-windows-aarch64.zip",
            ),
        ];
        for (install, arch, name) in cases {
            assert_eq!(install.asset_name(&version, arch).as_deref(), Some(name));
        }
        assert_eq!(
            Install::Managed(Manager::Snap).asset_name(&version, "x86_64"),
            None
        );
        assert_eq!(
            Install::Archive {
                prefix: "/p".into()
            }
            .asset_name(&version, "riscv64"),
            None
        );
    }
}
