//! Install a verified macOS DMG over the running app.
//!
//! Qrow mounts the DMG read-only, copies its `Qrow.app` with `ditto` into a
//! staging folder beside the installed app, and swaps the two apps with
//! renames on the same volume. If the second rename fails, the first one is
//! undone, so the installed app stays complete. macOS keeps the running
//! executable open, so the new version starts at the next launch.

use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Replaces the app at `app` with the `Qrow.app` in the DMG at `dmg`.
pub fn install(dmg: &Path, app: &Path) -> Result<()> {
    let parent = app
        .parent()
        .context("The installed app has no parent folder")?;
    let mount = Mount::attach(dmg)?;
    let source = mount.path().join("Qrow.app");
    ensure!(
        source.join("Contents/Info.plist").is_file(),
        "The update disk image has no Qrow.app"
    );
    let staging = tempfile::Builder::new()
        .prefix(".qrow-update-")
        .tempdir_in(parent)
        .with_context(|| format!("Could not create a staging folder in {}", parent.display()))?;
    let new_app = staging.path().join("Qrow.app");
    run(
        Command::new("ditto").arg(&source).arg(&new_app),
        "copy the new app",
    )?;
    drop(mount);
    // The release app has an ad hoc signature; a broken copy fails here.
    run(
        Command::new("codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&new_app),
        "verify the new app",
    )?;
    swap(app, &new_app, &staging.path().join("Qrow-old.app"))
}

/// Moves `app` to `old` and `new_app` to `app`. Restores `app` when the
/// second move fails.
fn swap(app: &Path, new_app: &Path, old: &Path) -> Result<()> {
    fs::rename(app, old).with_context(|| format!("Could not move {}", app.display()))?;
    if let Err(error) = fs::rename(new_app, app) {
        fs::rename(old, app).with_context(|| {
            format!("Could not restore {} after a failed update", app.display())
        })?;
        return Err(error).with_context(|| format!("Could not replace {}", app.display()));
    }
    Ok(())
}

fn run(command: &mut Command, what: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("Could not {what}"))?;
    if !output.status.success() {
        bail!(
            "Could not {what}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// A read-only mount of a disk image that Finder does not show. Dropping it
/// detaches the image.
struct Mount {
    path: PathBuf,
    _folder: tempfile::TempDir,
}

impl Mount {
    fn attach(dmg: &Path) -> Result<Self> {
        let folder = tempfile::Builder::new()
            .prefix("qrow-update-mount-")
            .tempdir()?;
        let path = folder.path().join("volume");
        run(
            Command::new("hdiutil")
                .args([
                    "attach",
                    "-nobrowse",
                    "-readonly",
                    "-noautoopen",
                    "-quiet",
                    "-mountpoint",
                ])
                .arg(&path)
                .arg(dmg),
            "open the update disk image",
        )?;
        Ok(Self {
            path,
            _folder: folder,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        let detach = |force: bool| {
            let mut command = Command::new("hdiutil");
            command.args(["detach", "-quiet"]).arg(&self.path);
            if force {
                command.arg("-force");
            }
            command.status().is_ok_and(|status| status.success())
        };
        if !detach(false) && !detach(true) {
            eprintln!("Could not detach {}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal app bundle with an ad hoc signature, like the release app.
    fn app(folder: &Path, version: &str) -> PathBuf {
        let app = folder.join("Qrow.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(
            app.join("Contents/Info.plist"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
                 <key>CFBundleExecutable</key><string>qrow</string>\
                 <key>CFBundleIdentifier</key><string>dev.qrow.test</string>\
                 <key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>\n"
            ),
        )
        .unwrap();
        fs::copy("/usr/bin/true", app.join("Contents/MacOS/qrow")).unwrap();
        run(
            Command::new("codesign")
                .args(["--force", "--deep", "--sign", "-"])
                .arg(&app),
            "sign",
        )
        .unwrap();
        app
    }

    fn dmg(source: &Path, output: &Path) {
        run(
            Command::new("hdiutil")
                .args([
                    "create",
                    "-quiet",
                    "-format",
                    "UDZO",
                    "-volname",
                    "Qrow",
                    "-srcfolder",
                ])
                .arg(source)
                .arg(output),
            "create a disk image",
        )
        .unwrap();
    }

    #[test]
    fn replaces_the_app_with_the_one_in_the_disk_image() {
        let root = tempfile::tempdir().unwrap();
        let installed = app(&root.path().join("Applications"), "0.5.0");
        let release = root.path().join("release");
        app(&release, "0.6.0");
        let image = root.path().join("Qrow.dmg");
        dmg(&release, &image);
        install(&image, &installed).unwrap();
        let plist = fs::read_to_string(installed.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<string>0.6.0</string>"), "{plist}");
        // The staging folder and the old app are gone.
        let left: Vec<_> = fs::read_dir(root.path().join("Applications"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["Qrow.app"]);
    }

    #[test]
    fn an_image_without_the_app_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let installed = app(&root.path().join("Applications"), "0.5.0");
        let empty = root.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        fs::write(empty.join("README"), "no app").unwrap();
        let image = root.path().join("Empty.dmg");
        dmg(&empty, &image);
        assert!(install(&image, &installed).is_err());
        let plist = fs::read_to_string(installed.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<string>0.5.0</string>"));
    }

    #[test]
    fn a_failed_swap_restores_the_installed_app() {
        let root = tempfile::tempdir().unwrap();
        let installed = app(&root.path().join("Applications"), "0.5.0");
        let missing = root.path().join("missing/Qrow.app");
        let old = root.path().join("Applications/Qrow-old.app");
        assert!(swap(&installed, &missing, &old).is_err());
        assert!(installed.join("Contents/Info.plist").is_file());
        assert!(!old.exists());
    }
}
