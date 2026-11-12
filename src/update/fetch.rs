//! Bounded HTTP downloads of release metadata and assets.

use super::verify::Digest;
use anyhow::{Context, Result, ensure};
use std::io::Write;

/// Reads a response body of at most `limit` bytes.
pub(super) async fn bytes(client: &reqwest::Client, url: &str, limit: u64) -> Result<Vec<u8>> {
    Ok(page(client, url, limit).await?.0)
}

/// Reads one page of a paginated GitHub API list: the body of at most `limit`
/// bytes, and the URL of the next page from the `Link` header. A next page
/// must be on the same origin as `url`.
pub(super) async fn page(
    client: &reqwest::Client,
    url: &str,
    limit: u64,
) -> Result<(Vec<u8>, Option<String>)> {
    let mut response = send(client, url, limit).await?;
    let next = response
        .headers()
        .get(reqwest::header::LINK)
        .and_then(|value| value.to_str().ok())
        .and_then(next_link)
        .filter(|next| same_origin(url, next));
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.context("The download stopped")? {
        ensure!(
            body.len() as u64 + chunk.len() as u64 <= limit,
            "The response from {url} is too large"
        );
        body.extend_from_slice(&chunk);
    }
    Ok((body, next))
}

/// The `rel="next"` URL of an RFC 8288 `Link` header.
fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|link| {
        let (target, parameters) = link.trim().split_once(';')?;
        let target = target.trim().strip_prefix('<')?.strip_suffix('>')?;
        parameters
            .split(';')
            .any(|parameter| parameter.trim() == "rel=\"next\"")
            .then(|| target.to_owned())
    })
}

fn same_origin(left: &str, right: &str) -> bool {
    match (url::Url::parse(left), url::Url::parse(right)) {
        (Ok(left), Ok(right)) => left.origin() == right.origin(),
        _ => false,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_next_page_of_a_link_header() {
        let header = r#"<https://api.github.com/repositories/1/releases?page=2>; rel="next", <https://api.github.com/repositories/1/releases?page=9>; rel="last""#;
        assert_eq!(
            next_link(header).as_deref(),
            Some("https://api.github.com/repositories/1/releases?page=2")
        );
        assert_eq!(next_link(r#"<https://x/?page=1>; rel="prev""#), None);
        assert_eq!(next_link("garbage"), None);
    }

    #[test]
    fn follows_pages_only_on_the_same_origin() {
        let api = "https://api.github.com/repos/o/r/releases";
        assert!(same_origin(
            api,
            "https://api.github.com/repositories/1/releases?page=2"
        ));
        assert!(!same_origin(api, "https://example.com/releases?page=2"));
        assert!(!same_origin(api, "http://api.github.com/releases?page=2"));
    }
}
