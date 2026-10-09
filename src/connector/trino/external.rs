//! Trino 483 external client authentication. See the upstream
//! ExternalAuthenticator, ExternalAuthentication, and HttpTokenPoller.
use super::transport::{Runtime, Transport};
use crate::external_auth::{Browser, Cancelled, Failure};
use anyhow::{Result, anyhow};
use reqwest::{
    Client, Method, StatusCode,
    header::{HeaderMap, WWW_AUTHENTICATE},
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use url::Url;
use zeroize::Zeroizing;

const MAX_POLL_BYTES: usize = 64 * 1024;
const PAUSE: Duration = Duration::from_millis(100);

pub(super) struct Challenge {
    pub redirect: Option<Url>,
    pub token: Url,
}
/// Every URL in this protocol must stay on the configured HTTPS coordinator.
pub(super) fn safe_url(text: &str, coordinator: &Url) -> Result<Url> {
    let url = Url::parse(text).map_err(|_| Failure("Trino returned an invalid sign-in URL"))?;
    let credentials = text.split_once("://").is_some_and(|(_, authority)| {
        authority
            .split(['/', '?', '#'])
            .next()
            .is_some_and(|authority| authority.contains('@'))
    });
    anyhow::ensure!(
        !credentials
            && url.scheme() == "https"
            && url.origin() == coordinator.origin()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        Failure("Trino returned an unsafe sign-in URL")
    );
    Ok(url)
}

pub(super) fn challenge(headers: &HeaderMap, coordinator: &Url) -> Result<Challenge> {
    for header in headers.get_all(WWW_AUTHENTICATE) {
        let text = header
            .to_str()
            .map_err(|_| Failure("Trino returned a malformed authentication challenge"))?;
        for (scheme, params) in challenges(text)? {
            if !scheme.eq_ignore_ascii_case("bearer") {
                continue;
            }
            if let Some(token) = params.get("x_token_server") {
                return Ok(Challenge {
                    token: safe_url(token, coordinator)?,
                    redirect: params
                        .get("x_redirect_server")
                        .map(|text| safe_url(text, coordinator))
                        .transpose()?,
                });
            }
            if params.contains_key("x_redirect_server") {
                return Err(Failure("Trino authentication challenge has no token server").into());
            }
        }
    }
    Err(Failure("Trino did not provide an external authentication challenge").into())
}

/// Split only commas outside quoted strings. An auth-param continues the
/// current challenge; a scheme begins another challenge. Quoted-pairs are
/// decoded after splitting, so commas and escaped quotes stay in the value.
fn challenges(text: &str) -> Result<Vec<(String, HashMap<String, String>)>> {
    let malformed = || {
        anyhow!(Failure(
            "Trino returned a malformed authentication challenge"
        ))
    };
    let mut parts = Vec::new();
    let (mut quoted, mut escaped, mut start) = (false, false, 0);
    for (index, byte) in text.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            quoted = !quoted;
        } else if byte == b',' && !quoted {
            parts.push(&text[start..index]);
            start = index + 1;
        }
    }
    if quoted || escaped {
        return Err(malformed());
    }
    parts.push(&text[start..]);
    let mut result: Vec<(String, HashMap<String, String>)> = Vec::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let token_end = part.bytes().take_while(|byte| token_char(*byte)).count();
        if token_end == 0 {
            return Err(malformed());
        }
        let rest = part[token_end..].trim_start();
        let param = if rest.starts_with('=') {
            part
        } else {
            result.push((part[..token_end].to_owned(), HashMap::new()));
            rest
        };
        if param.is_empty() {
            continue;
        }
        let (scheme, params) = result.last_mut().ok_or_else(malformed)?;
        // Other schemes can have token68 credentials, rather than parameters.
        if !scheme.eq_ignore_ascii_case("bearer") {
            continue;
        }
        let (name, value) = param.split_once('=').ok_or_else(malformed)?;
        let name = name.trim();
        if name.is_empty() || !name.bytes().all(token_char) {
            return Err(malformed());
        }
        let value = value.trim();
        let decoded = if value.starts_with('"') {
            if !value.ends_with('"') || value.len() < 2 {
                return Err(malformed());
            }
            let mut decoded = String::new();
            let mut chars = value[1..value.len() - 1].chars();
            while let Some(ch) = chars.next() {
                match ch {
                    '\\' => decoded.push(chars.next().ok_or_else(malformed)?),
                    '"' => return Err(malformed()),
                    c if c.is_control() => return Err(malformed()),
                    c => decoded.push(c),
                }
            }
            decoded
        } else {
            if value.is_empty() || !value.bytes().all(token_char) {
                return Err(malformed());
            }
            value.to_owned()
        };
        if params.insert(name.to_ascii_lowercase(), decoded).is_some() {
            return Err(malformed());
        }
    }
    Ok(result)
}
fn token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

#[derive(Deserialize)]
struct Poll {
    token: Option<String>,
    #[serde(rename = "nextUri")]
    next_uri: Option<String>,
    error: Option<String>,
}
fn check(cancel: &AtomicBool, deadline: Instant) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        return Err(Cancelled.into());
    }
    anyhow::ensure!(
        Instant::now() < deadline,
        Failure("Trino browser sign-in timed out")
    );
    Ok(())
}
async fn cancellable<T>(
    future: impl std::future::Future<Output = Result<T>>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<T> {
    tokio::pin!(future);
    loop {
        check(cancel, deadline)?;
        tokio::select! { result = &mut future => return result, _ = tokio::time::sleep(Duration::from_millis(25)) => {} }
    }
}
async fn send(
    client: &Client,
    method: Method,
    url: &Url,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<reqwest::Response> {
    cancellable(
        async {
            client
                .request(method, url.clone())
                .header("Accept", "application/json")
                .header("User-Agent", "Qrow TrinoTokenPoller")
                .timeout(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_secs(10)),
                )
                .send()
                .await
                .map_err(|_| anyhow!(Failure("Trino sign-in polling request failed")))
        },
        cancel,
        deadline,
    )
    .await
}
fn transient(status: StatusCode) -> bool {
    status.is_server_error() || matches!(status.as_u16(), 408 | 429)
}
async fn pause(cancel: &AtomicBool, deadline: Instant) -> Result<()> {
    cancellable(
        async {
            tokio::time::sleep(PAUSE).await;
            Ok(())
        },
        cancel,
        deadline,
    )
    .await
}

pub(super) fn authenticate(
    client: Client,
    coordinator: Url,
    challenge: Challenge,
    browser: Browser,
    cancel: Arc<AtomicBool>,
    timeout: Duration,
    transport: Arc<Transport>,
) -> Result<Zeroizing<String>> {
    let deadline = Instant::now() + timeout;
    check(&cancel, deadline)?;
    if let Some(redirect) = challenge.redirect {
        // The system browser owns subsequent navigation through the IdP. No
        // database credentials are sent by this callback or the poll client.
        browser(redirect.as_str()).map_err(|_| Failure("Cannot open the sign-in browser"))?;
    }
    check(&cancel, deadline)?;
    let runtime = Runtime::new()?;
    struct Owned(Arc<Transport>);
    impl Drop for Owned {
        fn drop(&mut self) {
            self.0.close_all();
        }
    }
    let _owned = Owned(transport.clone());
    runtime.block_on(transport.run(async {
        let mut url = challenge.token;
        loop {
            check(&cancel, deadline)?;
            let response = send(&client, Method::GET, &url, &cancel, deadline).await;
            let mut response = match response {
                Ok(response) if transient(response.status()) => {
                    pause(&cancel, deadline).await?;
                    continue;
                }
                Ok(response) => response,
                Err(error) => {
                    check(&cancel, deadline)?;
                    if error.is::<Cancelled>() {
                        return Err(error);
                    }
                    pause(&cancel, deadline).await?;
                    continue;
                }
            };
            anyhow::ensure!(
                response.status().is_success(),
                Failure("Trino sign-in polling was rejected")
            );
            let body = cancellable(
                async {
                    let mut bytes = Zeroizing::new(Vec::new());
                    while let Some(chunk) = response
                        .chunk()
                        .await
                        .map_err(|_| Failure("Trino sign-in polling response failed"))?
                    {
                        anyhow::ensure!(
                            bytes.len().saturating_add(chunk.len()) <= MAX_POLL_BYTES,
                            Failure("Trino sign-in response is too large")
                        );
                        bytes.extend_from_slice(&chunk);
                    }
                    Ok(bytes)
                },
                &cancel,
                deadline,
            )
            .await;
            let bytes = match body {
                Ok(bytes) => bytes,
                Err(error) => {
                    check(&cancel, deadline)?;
                    if error.to_string() == "Trino sign-in polling response failed" {
                        pause(&cancel, deadline).await?;
                        continue;
                    }
                    return Err(error);
                }
            };
            let poll: Poll = serde_json::from_slice(&bytes)
                .map_err(|_| Failure("Trino returned an invalid sign-in response"))?;
            if let Some(token) = poll.token {
                let token = Zeroizing::new(token);
                anyhow::ensure!(
                    !token.is_empty()
                        && reqwest::header::HeaderValue::from_str(&format!(
                            "Bearer {}",
                            token.as_str()
                        ))
                        .is_ok(),
                    Failure("Trino returned an invalid authentication token")
                );
                // Acknowledge the current polling URL, not the initial URL.
                let ack_deadline = deadline.min(Instant::now() + Duration::from_secs(4));
                loop {
                    match send(&client, Method::DELETE, &url, &cancel, ack_deadline).await {
                        Ok(response) if response.status().is_success() => break,
                        Ok(response) if transient(response.status()) => {
                            pause(&cancel, ack_deadline).await?
                        }
                        Err(_) => {
                            check(&cancel, ack_deadline)?;
                            pause(&cancel, ack_deadline).await?;
                        }
                        _ => return Err(Failure("Trino sign-in acknowledgment failed").into()),
                    }
                }
                return Ok(token);
            }
            if poll.error.is_some() {
                return Err(Failure("Trino rejected the browser sign-in").into());
            }
            url = safe_url(
                poll.next_uri.as_deref().ok_or(Failure(
                    "Trino sign-in response has no token or polling URL",
                ))?,
                &coordinator,
            )?;
            pause(&cancel, deadline).await?;
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(value: &str) -> Result<Challenge> {
        let mut headers = HeaderMap::new();
        headers.append(WWW_AUTHENTICATE, value.parse().unwrap());
        challenge(
            &headers,
            &Url::parse("https://trino.example:443/v1/statement").unwrap(),
        )
    }
    #[test]
    fn multiple_challenges_and_quoted_pairs() {
        let input = "Basic realm=\"a,b\", BEARER realm=\"say \\\"hi\\\"\", x_redirect_server=\"https://trino.example/login?id=a,b\", X_TOKEN_SERVER=\"https://trino.example/token?id=abc\", Negotiate";
        let parsed = parse(input).unwrap();
        assert_eq!(parsed.redirect.unwrap().query(), Some("id=a,b"));
        assert_eq!(parsed.token.path(), "/token");
        let mut headers = HeaderMap::new();
        headers.append(WWW_AUTHENTICATE, "Basic realm=\"one\"".parse().unwrap());
        headers.append(
            WWW_AUTHENTICATE,
            "Bearer x_token_server=\"https://trino.example/token\""
                .parse()
                .unwrap(),
        );
        assert!(
            challenge(&headers, &Url::parse("https://trino.example/").unwrap())
                .unwrap()
                .redirect
                .is_none()
        );
    }
    #[test]
    fn malformed_and_unsafe_challenges_are_rejected_without_secret_details() {
        for input in [
            "Bearer x_token_server=\"unfinished",
            "Bearer x_redirect_server=\"https://trino.example/login\"",
            "Basic realm=\"only basic\"",
            "Bearer x_token_server=\"https://trino.example/token\", x_token_server=\"https://trino.example/other\"",
            "Bearer x_token_server=\"https://trino.example/token\"junk",
        ] {
            assert!(parse(input).is_err(), "{input}");
        }
        for url in [
            "http://trino.example/token",
            "https://other.example/token",
            "https://trino.example:444/token",
            "https://user:secret@trino.example/token",
            "https://trino.example/token#secret",
            "/relative",
        ] {
            let error = parse(&format!("Bearer x_token_server=\"{url}\""))
                .err()
                .unwrap()
                .to_string();
            assert!(!error.contains(url));
        }
    }
}
