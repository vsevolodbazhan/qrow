//! The saved form of an [`Index`]. Qrow saves the index of a manifest, so
//! that a launch loads it in a few milliseconds instead of parsing the
//! manifest again.
//!
//! A saved index is the magic, the format version, the SHA-256 digest of
//! the payload, and the payload in postcard. The digest finds a damaged
//! file. A file of another format version is not an error for the caller:
//! it parses the manifest again.

use super::Index;
use serde::{Deserialize, Serialize};
use std::{fmt, path::Path, time::SystemTime};

const MAGIC: &[u8; 8] = b"QROWDBT\0";

/// The version of the saved form. Change it when a change to [`Index`] or
/// [`Saved`] changes the payload.
pub const FORMAT_VERSION: u32 = 1;

const HEADER_BYTES: usize = MAGIC.len() + 4 + DIGEST_BYTES;
const DIGEST_BYTES: usize = 32;

/// The size and modification time of a manifest. The spans of an index are
/// correct only while the manifest has the stamp of the index.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Stamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

impl Stamp {
    pub fn of(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

/// An index and the stamp of the manifest that it comes from.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Saved {
    pub stamp: Stamp,
    pub index: Index,
}

#[derive(Debug, Eq, PartialEq)]
pub enum LoadError {
    /// The file is not a saved index.
    NotSaved,
    /// The file has another format version. Parse the manifest again.
    OtherVersion(u32),
    /// The file is damaged.
    Damaged(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSaved => formatter.write_str("The file is not a saved dbt index"),
            Self::OtherVersion(version) => {
                write!(
                    formatter,
                    "The saved dbt index has format version {version}"
                )
            }
            Self::Damaged(message) => {
                write!(formatter, "The saved dbt index is damaged: {message}")
            }
        }
    }
}

impl std::error::Error for LoadError {}

pub fn encode(saved: &Saved) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_BYTES);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&[0; DIGEST_BYTES]);
    let mut bytes = postcard::to_extend(saved, bytes).expect("an index serializes");
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes[HEADER_BYTES..]);
    bytes[HEADER_BYTES - DIGEST_BYTES..HEADER_BYTES].copy_from_slice(digest.as_ref());
    bytes
}

pub fn decode(bytes: &[u8]) -> Result<Saved, LoadError> {
    let Some(rest) = bytes.strip_prefix(MAGIC) else {
        return Err(LoadError::NotSaved);
    };
    let (version, rest) = rest.split_first_chunk::<4>().ok_or(LoadError::NotSaved)?;
    let version = u32::from_le_bytes(*version);
    if version != FORMAT_VERSION {
        return Err(LoadError::OtherVersion(version));
    }
    let (digest, payload) = rest
        .split_first_chunk::<DIGEST_BYTES>()
        .ok_or_else(|| LoadError::Damaged("the header is short".into()))?;
    if ring::digest::digest(&ring::digest::SHA256, payload).as_ref() != digest {
        return Err(LoadError::Damaged("the digest does not agree".into()));
    }
    let saved: Saved =
        postcard::from_bytes(payload).map_err(|error| LoadError::Damaged(error.to_string()))?;
    saved.index.check().map_err(LoadError::Damaged)?;
    Ok(saved)
}
