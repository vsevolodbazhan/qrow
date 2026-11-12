//! Install a verified Linux release archive over the running copy.
//!
//! The archive holds `bin/` and `share/` for a prefix such as `~/.local`.
//! Qrow extracts it into a staging folder in the prefix, so that each file
//! moves into place with one rename on the same file system. The executable
//! moves last. Linux keeps the running executable open after the rename, so
//! the new version starts at the next launch.

use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
};

const MAX_ENTRIES: usize = 10_000;
const MAX_UNPACKED_BYTES: u64 = 1 << 30;
const EXECUTABLE: &str = "bin/qrow";

/// Replaces the files of the archive at `archive` in `prefix`.
pub fn install(archive: &Path, prefix: &Path) -> Result<()> {
    let staging = tempfile::Builder::new()
        .prefix(".qrow-update-")
        .tempdir_in(prefix)
        .with_context(|| format!("Could not create a staging folder in {}", prefix.display()))?;
    let files = unpack(archive, staging.path())?;
    ensure!(
        files.iter().any(|file| file == Path::new(EXECUTABLE)),
        "The update archive has no {EXECUTABLE}"
    );
    let (executable, others): (Vec<_>, Vec<_>) = files
        .into_iter()
        .partition(|file| file == Path::new(EXECUTABLE));
    for file in others.iter().chain(&executable) {
        let target = prefix.join(file);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Could not create {}", parent.display()))?;
        }
        fs::rename(staging.path().join(file), &target)
            .with_context(|| format!("Could not replace {}", target.display()))?;
    }
    Ok(())
}

/// Extracts regular files and folders under `bin/` and `share/` into
/// `staging`, and returns the relative paths of the files.
fn unpack(archive: &Path, staging: &Path) -> Result<Vec<PathBuf>> {
    let file = fs::File::open(archive).context("Could not open the update archive")?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut files = Vec::new();
    let mut unpacked = 0u64;
    for (index, entry) in archive.entries()?.enumerate() {
        ensure!(
            index < MAX_ENTRIES,
            "The update archive has too many entries"
        );
        let mut entry = entry.context("Could not read the update archive")?;
        let relative = relative_path(&entry.path()?)?;
        let kind = entry.header().entry_type();
        let Some(relative) = relative else {
            // The archive root, `./`.
            ensure!(kind.is_dir(), "The update archive has a file at its root");
            continue;
        };
        let target = staging.join(&relative);
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
        } else if kind.is_file() {
            let size = entry.size();
            unpacked = unpacked.saturating_add(size);
            ensure!(
                unpacked <= MAX_UNPACKED_BYTES,
                "The update archive is too large"
            );
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output = fs::File::create_new(&target)
                .with_context(|| format!("The update archive repeats {}", relative.display()))?;
            let copied = std::io::copy(&mut (&mut entry).take(size), &mut output)?;
            ensure!(copied == size, "The update archive is truncated");
            let mode = if entry.header().mode()? & 0o111 != 0 {
                0o755
            } else {
                0o644
            };
            fs::set_permissions(&target, fs::Permissions::from_mode(mode))?;
            files.push(relative);
        } else {
            bail!(
                "The update archive has a link or a special file: {}",
                relative.display()
            );
        }
    }
    Ok(files)
}

/// The path of an entry without `./`, or `None` for the archive root. Only
/// paths under `bin/` and `share/` are allowed.
fn relative_path(path: &Path) -> Result<Option<PathBuf>> {
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => relative.push(part),
            _ => bail!("The update archive has an unsafe path: {}", path.display()),
        }
    }
    let Some(Component::Normal(top)) = relative.components().next() else {
        return Ok(None);
    };
    ensure!(
        top == "bin" || top == "share",
        "The update archive has an unexpected path: {}",
        path.display()
    );
    Ok(Some(relative))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) enum Entry<'a> {
        Dir(&'a str),
        File(&'a str, &'a [u8], u32),
        Link(&'a str, &'a str),
    }

    /// A gzip tar archive like the one of the Linux package script.
    pub(crate) fn archive(entries: &[Entry]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for entry in entries {
            let mut header = tar::Header::new_gnu();
            match entry {
                Entry::Dir(path) => {
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_mode(0o755);
                    header.set_size(0);
                    raw_path(&mut header, path);
                    builder.append(&header, std::io::empty()).unwrap();
                }
                Entry::File(path, data, mode) => {
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_mode(*mode);
                    header.set_size(data.len() as u64);
                    raw_path(&mut header, path);
                    builder.append(&header, *data).unwrap();
                }
                Entry::Link(path, target) => {
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_size(0);
                    builder.append_link(&mut header, path, target).unwrap();
                }
            }
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    /// Writes the path as is. The tar builder rejects `..`, which the tests need.
    fn raw_path(header: &mut tar::Header, path: &str) {
        header.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
    }

    pub(crate) fn release_archive(executable: &[u8]) -> Vec<u8> {
        archive(&[
            Entry::Dir("./"),
            Entry::Dir("./bin/"),
            Entry::File("./bin/qrow", executable, 0o755),
            Entry::Dir("./share/applications/"),
            Entry::File(
                "./share/applications/qrow.desktop",
                b"[Desktop Entry]\n",
                0o644,
            ),
        ])
    }

    fn install_bytes(bytes: &[u8], prefix: &Path) -> Result<()> {
        let file = tempfile::NamedTempFile::new().unwrap();
        fs::write(file.path(), bytes).unwrap();
        install(file.path(), prefix)
    }

    #[test]
    fn replaces_the_files_and_keeps_the_executable_bit() {
        let prefix = tempfile::tempdir().unwrap();
        fs::create_dir_all(prefix.path().join("bin")).unwrap();
        fs::write(prefix.path().join("bin/qrow"), b"old").unwrap();
        fs::write(prefix.path().join("bin/other"), b"kept").unwrap();
        install_bytes(&release_archive(b"new"), prefix.path()).unwrap();
        assert_eq!(fs::read(prefix.path().join("bin/qrow")).unwrap(), b"new");
        assert_eq!(fs::read(prefix.path().join("bin/other")).unwrap(), b"kept");
        let mode = fs::metadata(prefix.path().join("bin/qrow"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        let launcher = prefix.path().join("share/applications/qrow.desktop");
        assert_eq!(
            fs::metadata(launcher).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let left: Vec<_> = fs::read_dir(prefix.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left.len(), 2, "no staging folder remains: {left:?}");
    }

    #[test]
    fn installs_an_archive_made_like_the_linux_package() {
        // scripts/package/linux.sh runs `tar -C PACKAGE -czf ARCHIVE .`.
        let package = tempfile::tempdir().unwrap();
        for (path, data) in [
            ("bin/qrow", "new"),
            ("share/applications/qrow.desktop", "x"),
        ] {
            let file = package.path().join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, data).unwrap();
        }
        fs::set_permissions(
            package.path().join("bin/qrow"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let output = tempfile::tempdir().unwrap();
        let archive = output.path().join("qrow.tar.gz");
        let status = std::process::Command::new("tar")
            .arg("-C")
            .arg(package.path())
            .arg("-czf")
            .arg(&archive)
            .arg(".")
            .status()
            .unwrap();
        assert!(status.success());
        let prefix = tempfile::tempdir().unwrap();
        install(&archive, prefix.path()).unwrap();
        assert_eq!(fs::read(prefix.path().join("bin/qrow")).unwrap(), b"new");
        let mode = fs::metadata(prefix.path().join("bin/qrow"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn rejects_unsafe_archives_without_changing_the_prefix() {
        let cases: [(&str, Vec<u8>); 6] = [
            (
                "parent",
                archive(&[Entry::File("bin/../../evil", b"x", 0o644)]),
            ),
            (
                "other folder",
                archive(&[
                    Entry::File("etc/qrow", b"x", 0o644),
                    Entry::File("bin/qrow", b"x", 0o755),
                ]),
            ),
            ("link", archive(&[Entry::Link("bin/qrow", "/bin/sh")])),
            ("root file", archive(&[Entry::File("qrow", b"x", 0o755)])),
            (
                "no executable",
                archive(&[Entry::File("share/qrow.png", b"x", 0o644)]),
            ),
            (
                "repeated",
                archive(&[
                    Entry::File("bin/qrow", b"1", 0o755),
                    Entry::File("bin/qrow", b"2", 0o755),
                ]),
            ),
        ];
        for (name, bytes) in cases {
            let prefix = tempfile::tempdir().unwrap();
            fs::create_dir_all(prefix.path().join("bin")).unwrap();
            fs::write(prefix.path().join("bin/qrow"), b"old").unwrap();
            assert!(install_bytes(&bytes, prefix.path()).is_err(), "{name}");
            assert_eq!(
                fs::read(prefix.path().join("bin/qrow")).unwrap(),
                b"old",
                "{name}"
            );
            assert_eq!(fs::read_dir(prefix.path()).unwrap().count(), 1, "{name}");
        }
        let prefix = tempfile::tempdir().unwrap();
        assert!(install_bytes(b"not gzip", prefix.path()).is_err());
    }
}
