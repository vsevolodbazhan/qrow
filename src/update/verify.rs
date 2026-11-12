//! Signature and checksum checks of release files.
//!
//! The release workflow signs `SHA256SUMS` with minisign. The trusted comment
//! of the signature names the release tag, so the signed checksums of one
//! release cannot pass for another release.

use anyhow::{Context, Result, bail, ensure};
use std::collections::HashMap;

/// The public key of the Qrow release key, in minisign format.
pub const RELEASE_PUBLIC_KEY: &str = include_str!("../../assets/release/minisign.pub");

/// A SHA-256 digest.
pub type Digest = [u8; 32];

/// The verified checksums of one release, by asset name.
#[derive(Debug)]
pub struct Checksums(HashMap<String, Digest>);

impl Checksums {
    pub fn digest(&self, name: &str) -> Option<&Digest> {
        self.0.get(name)
    }
}

/// The trusted comment that the release workflow signs for `tag`.
pub fn trusted_comment(tag: &str) -> String {
    format!("Qrow release {tag}")
}

/// Verifies the signature of `sums` with `public_key` and reads the
/// checksums. The signature must be for the release `tag`.
pub fn verify_checksums(
    sums: &[u8],
    signature: &str,
    public_key: &str,
    tag: &str,
) -> Result<Checksums> {
    let key = minisign_verify::PublicKey::decode(public_key.trim())
        .context("Could not read the release public key")?;
    let signature = minisign_verify::Signature::decode(signature)
        .context("Could not read the release signature")?;
    key.verify(sums, &signature, false)
        .context("The release signature is not valid")?;
    ensure!(
        signature.trusted_comment() == trusted_comment(tag),
        "The release signature is for another release"
    );
    parse_checksums(sums)
}

/// Reads `sha256sum` output: a lowercase digest, two separator characters,
/// and a file name, optionally with a `./` prefix.
fn parse_checksums(sums: &[u8]) -> Result<Checksums> {
    let text = std::str::from_utf8(sums).context("The release checksums are not UTF-8")?;
    let mut checksums = HashMap::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let (hex, name) = line
            .split_at_checked(64)
            .context("A release checksum line is too short")?;
        let name = name
            .strip_prefix("  ")
            .or_else(|| name.strip_prefix(" *"))
            .context("A release checksum line has no file name")?;
        let name = name.strip_prefix("./").unwrap_or(name);
        ensure!(
            !name.is_empty() && !name.contains(['/', '\\']),
            "A release checksum names a path: {name}"
        );
        let digest = decode_hex(hex).context("A release checksum is not a SHA-256 digest")?;
        if checksums.insert(name.to_owned(), digest).is_some() {
            bail!("The release checksums name {name} twice");
        }
    }
    Ok(Checksums(checksums))
}

fn decode_hex(hex: &str) -> Option<Digest> {
    let mut digest = [0; 32];
    if hex.len() != 64 {
        return None;
    }
    for (byte, pair) in digest.iter_mut().zip(hex.as_bytes().chunks(2)) {
        let pair = std::str::from_utf8(pair).ok()?;
        if pair.bytes().any(|c| c.is_ascii_uppercase()) {
            return None;
        }
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(digest)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use minisign::KeyPair;

    /// A synthetic key pair and its minisign public key text.
    pub(crate) struct TestKey {
        pair: KeyPair,
        pub(crate) public: String,
    }

    impl TestKey {
        pub(crate) fn new() -> Self {
            let pair = KeyPair::generate_unencrypted_keypair().unwrap();
            let public = pair.pk.to_box().unwrap().into_string();
            Self { pair, public }
        }

        pub(crate) fn sign(&self, data: &[u8], comment: &str) -> String {
            minisign::sign(
                Some(&self.pair.pk),
                &self.pair.sk,
                data,
                Some(comment),
                None,
            )
            .unwrap()
            .into_string()
        }
    }

    pub(crate) fn sha256(data: &[u8]) -> Digest {
        ring::digest::digest(&ring::digest::SHA256, data)
            .as_ref()
            .try_into()
            .unwrap()
    }

    pub(crate) fn hex(digest: &Digest) -> String {
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn sums(files: &[(&str, &[u8])]) -> Vec<u8> {
        files
            .iter()
            .map(|(name, data)| format!("{}  ./{name}\n", hex(&sha256(data))))
            .collect::<String>()
            .into_bytes()
    }

    #[test]
    fn the_release_public_key_is_a_minisign_key() {
        minisign_verify::PublicKey::decode(RELEASE_PUBLIC_KEY.trim()).unwrap();
    }

    #[test]
    fn accepts_checksums_signed_for_the_release() {
        let key = TestKey::new();
        let sums = sums(&[("Qrow-0.6.0-linux-x86_64.tar.gz", b"archive")]);
        let signature = key.sign(&sums, "Qrow release v0.6.0");
        let checksums = verify_checksums(&sums, &signature, &key.public, "v0.6.0").unwrap();
        assert_eq!(
            checksums.digest("Qrow-0.6.0-linux-x86_64.tar.gz"),
            Some(&sha256(b"archive"))
        );
        assert_eq!(checksums.digest("./Qrow-0.6.0-linux-x86_64.tar.gz"), None);
    }

    #[test]
    fn rejects_another_key_changed_checksums_and_another_release() {
        let key = TestKey::new();
        let sums = sums(&[("Qrow.dmg", b"dmg")]);
        let signature = key.sign(&sums, "Qrow release v0.6.0");
        let other = TestKey::new();
        assert!(verify_checksums(&sums, &signature, &other.public, "v0.6.0").is_err());
        let mut changed = sums.clone();
        changed[0] = if changed[0] == b'0' { b'1' } else { b'0' };
        assert!(verify_checksums(&changed, &signature, &key.public, "v0.6.0").is_err());
        let error = verify_checksums(&sums, &signature, &key.public, "v0.7.0").unwrap_err();
        assert_eq!(
            error.to_string(),
            "The release signature is for another release"
        );
        assert!(verify_checksums(&sums, "not a signature", &key.public, "v0.6.0").is_err());
    }

    #[test]
    fn rejects_malformed_checksum_lines() {
        let digest = hex(&sha256(b"x"));
        for text in [
            "short  name\n".to_owned(),
            format!("{digest} name\n"),
            format!("{digest}  \n"),
            format!("{digest}  ../name\n"),
            format!("{digest}  dir/name\n"),
            format!("{}  name\n", digest.to_uppercase()),
            format!("{digest}  name\n{digest}  ./name\n"),
        ] {
            assert!(parse_checksums(text.as_bytes()).is_err(), "{text:?}");
        }
        let binary_mode = format!("{digest} *name\n");
        assert!(
            parse_checksums(binary_mode.as_bytes())
                .unwrap()
                .digest("name")
                .is_some()
        );
    }
}
