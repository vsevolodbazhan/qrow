//! The web searches that a harness reports, in one shape for both harnesses.
use super::WebLink;
use serde_json::Value;

/// The most links of one web search that Qrow keeps.
const MAX_SEARCH_LINKS: usize = 20;
/// The longest search text or title that Qrow keeps, in characters.
const MAX_TEXT_CHARS: usize = 500;
/// The longest link that Qrow keeps, in bytes.
const MAX_URL_BYTES: usize = 2048;

/// Keeps the hits with a web URL, once each, up to the limit.
pub(super) fn collect_links<'a>(hits: impl Iterator<Item = &'a Value>) -> Vec<WebLink> {
    let mut links: Vec<WebLink> = Vec::new();
    for hit in hits {
        let Some(url) = hit.get("url").and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if !(url.starts_with("https://") || url.starts_with("http://"))
            || url.len() > MAX_URL_BYTES
            || url.chars().any(char::is_whitespace)
            || links.iter().any(|link| link.url == url)
        {
            continue;
        }
        let title = hit
            .get("title")
            .and_then(Value::as_str)
            .map(clean_text)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| url.to_owned());
        links.push(WebLink {
            title,
            url: url.to_owned(),
        });
        if links.len() == MAX_SEARCH_LINKS {
            break;
        }
    }
    links
}

/// One line of text, at most [`MAX_TEXT_CHARS`] characters.
pub(super) fn clean_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TEXT_CHARS)
        .collect()
}
