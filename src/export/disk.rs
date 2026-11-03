//! Spools and final outputs share reservations on each filesystem.
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

const FLOOR: u64 = 1024 * 1024 * 1024;
const CHUNK: usize = 8 * 1024 * 1024;
static VOLUMES: LazyLock<Arc<Reservations>> = LazyLock::new(|| Arc::new(Reservations::default()));

#[derive(Default)]
struct Reservations(Mutex<HashMap<u64, u64>>);

struct Reservation {
    owner: Arc<Reservations>,
    volume: u64,
    bytes: u64,
}

impl Reservations {
    fn reserve(
        self: &Arc<Self>,
        volume: u64,
        bytes: u64,
        available: impl FnOnce() -> io::Result<u64>,
        floor: u64,
    ) -> io::Result<Reservation> {
        let mut counts = self.0.lock().unwrap();
        let reserved = counts.get(&volume).copied().unwrap_or(0);
        let total = reserved
            .checked_add(bytes)
            .ok_or_else(|| io::Error::other("Export disk reservation overflow."))?;
        // Read free space under the same lock as reservations. A concurrent
        // writer cannot base its decision on this writer's unreserved chunk.
        if available()?.saturating_sub(total) < floor {
            return Err(io::Error::other(
                "Export stopped to keep 1 GiB of disk space free.",
            ));
        }
        counts.insert(volume, total);
        Ok(Reservation {
            owner: self.clone(),
            volume,
            bytes,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut counts = self.owner.0.lock().unwrap();
        let count = counts.get_mut(&self.volume).unwrap();
        *count -= self.bytes;
        if *count == 0 {
            counts.remove(&self.volume);
        }
    }
}

/// The wrapper checks growth before the underlying file receives each chunk.
pub struct DiskWriter<W> {
    inner: W,
    directory: PathBuf,
    volume: u64,
    reservations: Arc<Reservations>,
}

impl<W> DiskWriter<W> {
    pub fn new(inner: W, directory: &Path) -> io::Result<Self> {
        Ok(Self {
            inner,
            directory: directory.to_path_buf(),
            volume: directory.metadata()?.dev(),
            reservations: VOLUMES.clone(),
        })
    }

    pub fn into_inner(self) -> W {
        self.inner
    }

    fn error(&self, error: io::Error) -> io::Error {
        io::Error::new(
            error.kind(),
            format!(
                "Export disk error on the volume containing {}: {error}",
                self.directory.display()
            ),
        )
    }
}

impl<W: Write> Write for DiskWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let bytes = &bytes[..bytes.len().min(CHUNK)];
        let _reservation = self
            .reservations
            .reserve(
                self.volume,
                bytes.len() as u64,
                || fs2::available_space(&self.directory),
                FLOOR,
            )
            .map_err(|error| self.error(error))?;
        self.inner.write(bytes).map_err(|error| self.error(error))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().map_err(|error| self.error(error))
    }
}

impl DiskWriter<File> {
    pub(crate) fn sync_all(&self) -> io::Result<()> {
        self.inner.sync_all().map_err(|error| self.error(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simultaneous_output_and_spool_reservations_share_a_volume() {
        let reservations = Arc::new(Reservations::default());
        let first = reservations.reserve(1, 40, || Ok(100), 50).unwrap();
        assert!(reservations.reserve(1, 20, || Ok(100), 50).is_err());
        let other = reservations.reserve(2, 40, || Ok(100), 50).unwrap();
        drop(first);
        let next = reservations.reserve(1, 50, || Ok(100), 50).unwrap();
        drop(next);
        drop(other);
        assert!(reservations.0.lock().unwrap().is_empty());
    }

    #[test]
    fn write_failure_names_the_volume_and_releases_the_reservation() {
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("No space left on device"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let mut writer = DiskWriter::new(Full, directory.path()).unwrap();
        writer.reservations = Arc::new(Reservations::default());
        let volume = writer.volume;
        let error = writer.write_all(b"data").unwrap_err().to_string();
        assert!(error.contains(&directory.path().display().to_string()));
        assert!(error.contains("No space left"));
        assert!(!writer.reservations.0.lock().unwrap().contains_key(&volume));
    }
}
