//! Records the Git commit of the build so the application can report it,
//! and embeds the icon and version resources of the Windows executable.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=QROW_COMMIT");
    println!("cargo:rerun-if-env-changed=QROW_RELEASE_VERSION");

    let release_version = std::env::var("QROW_RELEASE_VERSION")
        .ok()
        .filter(|version| !version.trim().is_empty())
        .or_else(|| std::env::var("CARGO_PKG_VERSION").ok())
        .unwrap_or_default();
    println!(
        "cargo:rustc-env=QROW_RELEASE_VERSION={}",
        release_version.trim()
    );
    // Only builds with an explicit release version, such as those of the
    // release workflow, check for updates. They follow the channel of that
    // version.
    let update_channel = match std::env::var("QROW_RELEASE_VERSION") {
        Ok(version) if version.trim().contains("-nightly.") => "nightly",
        Ok(version) if !version.trim().is_empty() => "stable",
        _ => "",
    };
    println!("cargo:rustc-env=QROW_UPDATE_CHANNEL={update_channel}");
    // Build scripts run on the host. Select by the target, so that a cross
    // build from Linux and a native Windows build both embed the resources.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_resources(release_version.trim());
    }

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

/// Embeds the icon as resource 1, which GPUI loads for the window class, and
/// a version resource for File Properties.
fn embed_windows_resources(version: &str) {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = root.join("assets/app-icons/windows/qrow.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let [major, minor, patch] = windows_version(version);
    let icon = icon.to_string_lossy().replace('\\', "\\\\");
    let version = version.replace('"', "");
    let script = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Qrow\0"
      VALUE "FileDescription", "Qrow\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "qrow\0"
      VALUE "LegalCopyright", "MIT License\0"
      VALUE "OriginalFilename", "qrow.exe\0"
      VALUE "ProductName", "Qrow\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let path = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("qrow.rc");
    std::fs::write(&path, script).expect("write the Windows resource script");
    embed_resource::compile_for(&path, ["qrow"], embed_resource::NONE)
        .manifest_required()
        .expect("compile the Windows resources");
}

/// The numeric part of a version such as `0.4.0-nightly.20261110.12`, for the
/// fixed fields of a Windows version resource. Each field holds 16 bits.
fn windows_version(version: &str) -> [u16; 3] {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let mut fields = core
        .split('.')
        .map(|field| field.parse::<u16>().unwrap_or(0));
    [(); 3].map(|()| fields.next().unwrap_or(0))
}
