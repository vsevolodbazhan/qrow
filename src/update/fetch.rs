//! Bounded HTTP downloads of release metadata and assets.

use super::verify::Digest;
use anyhow::{Context, Result, ensure};
use std::io::Write;

/// Reads a response body of at most `limit` bytes.
pub(super) async fn bytes(client: &reqwest::Client, url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut response = send(client, url, limit).await?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.context("The download stopped")? {
        ensure!(
            body.len() as u64 + chunk.len() as u64 <= limit,
            "The response from {url} is too large"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Writes a response body of exactly `size` bytes to `output` and returns its
/// SHA-256 digest.
pub(super) async fn file(
    client: &reqwest::Client,
    url: &str,
    size: u64,
    output: &mut impl Write,
) -> Result<Digest> {
    let mut response = send(client, url, size).await?;
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut written = 0u64;
    while let Some(chunk) = response.chunk().await.context("The download stopped")? {
        written += chunk.len() as u64;
        ensure!(
            written <= size,
            "The download is larger than the release states"
        );
        digest.update(&chunk);
        output
            .write_all(&chunk)
            .context("Could not save the download")?;
    }
    ensure!(written == size, "The download is incomplete");
    output.flush().context("Could not save the download")?;
    Ok(digest
        .finish()
        .as_ref()
        .try_into()
        .expect("SHA-256 digests have 32 bytes"))
}

async fn send(client: &reqwest::Client, url: &str, limit: u64) -> Result<reqwest::Response> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Could not connect to {}", host(url)))?;
    let status = response.status();
    ensure!(status.is_success(), "{} returned HTTP {status}", host(url));
    if let Some(length) = response.content_length() {
        ensure!(length <= limit, "The response from {url} is too large");
    }
    Ok(response)
}

fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned())
}
