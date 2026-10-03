//! The authorization-code flow with PKCE for a native client (RFC 8252),
//! and token refresh.
use super::{Failure, SignInError, http, jwt::BASE64URL};
use crate::{model::SignIn, tls::Trust};
use anyhow::{Context, Result};
use base64::Engine;
use ring::{digest, rand::SecureRandom};
use serde::Deserialize;
use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use url::Url;
use zeroize::Zeroizing;

/// How long Qrow waits for the browser to return to the callback.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const CALLBACK_PATH: &str = "/callback";
const CALLBACK_READ_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CALLBACK_REQUEST: usize = 16 * 1024;
/// The token lifetime that Qrow assumes when a provider does not state one.
const DEFAULT_EXPIRES_IN: u64 = 300;

/// The endpoints of a provider, from its discovery document.
#[derive(Clone, Debug)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: Url,
    pub token_endpoint: Url,
    pub jwks_uri: Url,
}

#[derive(Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    code_challenge_methods_supported: Option<Vec<String>>,
}

fn network(error: anyhow::Error) -> anyhow::Error {
    SignInError::new(Failure::Network, format!("{error:#}")).into()
}

/// Reads the discovery document of `issuer` and checks that the provider
/// uses the same issuer identifier and HTTPS endpoints.
pub fn discover(trust: &Trust, issuer: &str) -> Result<Discovery> {
    let url = Url::parse(&format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    ))
    .context("The issuer is not a valid URL")?;
    let response = http::get(trust, &url).map_err(network)?;
    anyhow::ensure!(
        response.status == 200,
        "The provider did not return its configuration (HTTP {}). Check the issuer",
        response.status
    );
    let document: DiscoveryDocument = serde_json::from_slice(&response.body)
        .context("The provider configuration is not valid")?;
    anyhow::ensure!(
        document.issuer == issuer,
        "The provider uses the issuer {}, not {issuer}",
        document.issuer
    );
    if let Some(methods) = &document.code_challenge_methods_supported {
        anyhow::ensure!(
            methods.iter().any(|method| method == "S256"),
            "The provider does not support PKCE with S256"
        );
    }
    let endpoint = |value: &str, name: &str| -> Result<Url> {
        let url = Url::parse(value).with_context(|| format!("The {name} is not a valid URL"))?;
        http::require_https(&url)?;
        Ok(url)
    };
    Ok(Discovery {
        authorization_endpoint: endpoint(
            &document.authorization_endpoint,
            "authorization endpoint",
        )?,
        token_endpoint: endpoint(&document.token_endpoint, "token endpoint")?,
        jwks_uri: endpoint(&document.jwks_uri, "key endpoint")?,
        issuer: document.issuer,
    })
}

pub fn fetch_keys(trust: &Trust, discovery: &Discovery) -> Result<super::jwt::Jwks> {
    let response = http::get(trust, &discovery.jwks_uri).map_err(network)?;
    anyhow::ensure!(
        response.status == 200,
        "The provider did not return its signing keys (HTTP {})",
        response.status
    );
    serde_json::from_slice(&response.body).context("The signing keys of the provider are invalid")
}

/// A random value with 256 bits of entropy, encoded for a URL.
fn random_value() -> Result<Zeroizing<String>> {
    let mut bytes = Zeroizing::new([0; 32]);
    ring::rand::SystemRandom::new()
        .fill(&mut bytes[..])
        .map_err(|_| anyhow::anyhow!("Could not create a random value"))?;
    Ok(Zeroizing::new(BASE64URL.encode(&bytes[..])))
}

pub fn code_challenge(verifier: &str) -> String {
    BASE64URL.encode(digest::digest(&digest::SHA256, verifier.as_bytes()))
}

/// One browser sign-in. It owns the loopback listener of its callback, and
/// closes it when it is dropped.
pub struct Attempt {
    listener: TcpListener,
    url: Url,
    redirect_uri: String,
    state: Zeroizing<String>,
    nonce: Zeroizing<String>,
    verifier: Zeroizing<String>,
    issuer: String,
}

impl std::fmt::Debug for Attempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attempt")
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

/// Listens on the first free port of `ports` on loopback only, or on an
/// available port when `ports` is empty.
fn bind_callback(ports: &[u16]) -> Result<TcpListener> {
    if ports.is_empty() {
        return TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .context("Could not listen for the sign-in callback on 127.0.0.1");
    }
    let mut last_error = None;
    for &port in ports {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            Ok(listener) => return Ok(listener),
            Err(error) => last_error = Some(error),
        }
    }
    let ports: Vec<String> = ports.iter().map(u16::to_string).collect();
    Err(anyhow::anyhow!(
        "Could not listen for the sign-in callback on 127.0.0.1, port {}: {}",
        ports.join(", "),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_default()
    ))
}

impl Attempt {
    /// Binds the callback listener to loopback only and builds the
    /// authorization URL with a new verifier, state, and nonce.
    pub fn prepare(config: &SignIn, discovery: &Discovery) -> Result<Self> {
        let listener = bind_callback(&config.callback_ports)?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let state = random_value()?;
        let nonce = random_value()?;
        let verifier = random_value()?;
        let mut url = discovery.authorization_endpoint.clone();
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("response_type", "code")
                .append_pair("client_id", &config.client_id)
                .append_pair("redirect_uri", &redirect_uri)
                .append_pair("scope", &config.requested_scopes().join(" "))
                .append_pair("state", &state)
                .append_pair("nonce", &nonce)
                .append_pair("code_challenge", &code_challenge(&verifier))
                .append_pair("code_challenge_method", "S256");
            if let Some(resource) = &config.resource {
                query.append_pair("resource", resource);
            }
        }
        Ok(Self {
            listener,
            url,
            redirect_uri,
            state,
            nonce,
            verifier,
            issuer: discovery.issuer.clone(),
        })
    }

    /// The URL to open in the browser.
    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Waits for the callback of this attempt and returns its authorization
    /// code. Requests with another path or state get an error page, and the
    /// wait continues.
    pub fn wait(&self, cancel: &AtomicBool, timeout: Duration) -> Result<Zeroizing<String>> {
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(SignInError::new(Failure::Cancelled, "Sign-in was cancelled").into());
            }
            if Instant::now() >= deadline {
                return Err(SignInError::new(
                    Failure::Other,
                    "The browser did not finish the sign-in in time",
                )
                .into());
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(code) = self.answer(stream)? {
                        return Ok(code);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(error).context("The sign-in callback failed"),
            }
        }
    }

    /// Returns the code of a valid callback, `None` to keep waiting, or the
    /// error that the provider returned.
    fn answer(&self, mut stream: TcpStream) -> Result<Option<Zeroizing<String>>> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(CALLBACK_READ_TIMEOUT))?;
        stream.set_write_timeout(Some(CALLBACK_READ_TIMEOUT))?;
        let Ok(target) = read_request_target(&mut stream) else {
            return Ok(None);
        };
        let Ok(url) = Url::parse(&format!("http://127.0.0.1{}", target.as_str())) else {
            respond(&mut stream, 400, "This is not a valid sign-in response.");
            return Ok(None);
        };
        if url.path() != CALLBACK_PATH {
            respond(&mut stream, 404, "Qrow is waiting for a sign-in response.");
            return Ok(None);
        }
        let mut state = None;
        let mut code = None;
        let mut error = None;
        let mut issuer = None;
        for (name, value) in url.query_pairs() {
            match name.as_ref() {
                "state" => state = Some(Zeroizing::new(value.into_owned())),
                "code" => code = Some(Zeroizing::new(value.into_owned())),
                "error" => error = Some(value.into_owned()),
                "iss" => issuer = Some(value.into_owned()),
                _ => {}
            }
        }
        if state.as_deref().map(String::as_str) != Some(self.state.as_str()) {
            respond(
                &mut stream,
                400,
                "This sign-in response does not belong to the current sign-in in Qrow.",
            );
            return Ok(None);
        }
        // RFC 9207: a response from another issuer is a mix-up attack.
        if issuer.as_ref().is_some_and(|issuer| *issuer != self.issuer) {
            respond(
                &mut stream,
                400,
                "The sign-in response came from another provider.",
            );
            anyhow::bail!("The sign-in response came from another provider");
        }
        if let Some(error) = error {
            respond(
                &mut stream,
                200,
                "The sign-in did not finish. Return to Qrow.",
            );
            let error: String = error
                .chars()
                .filter(|c| c.is_ascii_graphic())
                .take(80)
                .collect();
            if error == "access_denied" {
                anyhow::bail!("The provider denied the sign-in");
            }
            anyhow::bail!("The provider did not finish the sign-in ({error})");
        }
        let Some(code) = code.filter(|code| !code.is_empty()) else {
            respond(&mut stream, 400, "The sign-in response has no code.");
            anyhow::bail!("The sign-in response has no authorization code");
        };
        respond(
            &mut stream,
            200,
            "Qrow received the sign-in. You can close this tab and return to Qrow.",
        );
        Ok(Some(code))
    }
}

fn read_request_target(stream: &mut TcpStream) -> Result<Zeroizing<String>> {
    let mut data = Zeroizing::new(Vec::new());
    let mut buffer = [0; 2048];
    while !data.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer)?;
        anyhow::ensure!(read > 0, "The browser closed the connection");
        data.extend_from_slice(&buffer[..read]);
        anyhow::ensure!(
            data.len() <= MAX_CALLBACK_REQUEST,
            "The request is too large"
        );
    }
    let line = data
        .split(|byte| *byte == b'\n')
        .next()
        .and_then(|line| std::str::from_utf8(line).ok())
        .context("Invalid request")?;
    let mut parts = line.trim_end().split(' ');
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" || !target.starts_with('/') {
        respond(stream, 405, "Qrow is waiting for a sign-in response.");
        anyhow::bail!("Unexpected request");
    }
    Ok(Zeroizing::new(target.to_owned()))
}

fn respond(stream: &mut TcpStream, status: u16, message: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Method Not Allowed",
    };
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Qrow</title><p style=\"font-family: -apple-system, sans-serif\">{message}</p>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

/// A successful token response. The tokens are secrets.
pub struct Tokens {
    pub access_token: Zeroizing<String>,
    pub expires_in: u64,
    pub refresh_token: Option<Zeroizing<String>>,
    pub id_token: Option<Zeroizing<String>>,
    /// The granted scopes, when the provider states them.
    pub scope: Option<String>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct TokenDocument {
    access_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    scope: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

impl Drop for TokenDocument {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.id_token.zeroize();
    }
}

/// Exchanges the authorization code of `attempt`.
pub fn exchange_code(
    trust: &Trust,
    config: &SignIn,
    discovery: &Discovery,
    attempt: &Attempt,
    code: &str,
) -> Result<Tokens> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", attempt.redirect_uri.as_str()),
        ("client_id", config.client_id.as_str()),
        ("code_verifier", attempt.verifier.as_str()),
    ];
    if let Some(resource) = &config.resource {
        form.push(("resource", resource));
    }
    let tokens = token_request(trust, discovery, &form)?;
    anyhow::ensure!(
        tokens.id_token.is_some(),
        "The provider did not return an ID token. Check that the client uses OpenID Connect"
    );
    Ok(tokens)
}

/// Requests a new access token with a refresh token.
pub fn refresh(
    trust: &Trust,
    config: &SignIn,
    discovery: &Discovery,
    refresh_token: &str,
) -> Result<Tokens> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", config.client_id.as_str()),
    ];
    if let Some(resource) = &config.resource {
        form.push(("resource", resource));
    }
    token_request(trust, discovery, &form)
}

fn token_request(trust: &Trust, discovery: &Discovery, form: &[(&str, &str)]) -> Result<Tokens> {
    let response = http::post_form(trust, &discovery.token_endpoint, form).map_err(network)?;
    let document: Option<TokenDocument> = serde_json::from_slice(&response.body).ok();
    if response.status != 200 {
        let (error, description) = document
            .as_ref()
            .map(|document| (document.error.clone(), document.error_description.clone()))
            .unwrap_or_default();
        let printable = |text: Option<String>, limit| -> Option<String> {
            text.map(|text| {
                text.chars()
                    .filter(|c| !c.is_control())
                    .take(limit)
                    .collect()
            })
        };
        let error = printable(error, 80);
        let description = printable(description, 300);
        let detail = match (&error, &description) {
            (Some(error), Some(description)) => format!("{error}: {description}"),
            (Some(error), None) => error.clone(),
            _ => format!("HTTP {}", response.status),
        };
        let failure = if error.as_deref() == Some("invalid_grant") {
            Failure::SignInRequired
        } else if response.status >= 500 {
            Failure::Network
        } else {
            Failure::Other
        };
        return Err(SignInError::new(
            failure,
            format!("The provider rejected the token request ({detail})"),
        )
        .into());
    }
    let mut document = document.context("The token response is not valid")?;
    anyhow::ensure!(
        document
            .token_type
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("bearer")),
        "The provider returned a token type other than Bearer"
    );
    let access_token = document
        .access_token
        .take()
        .filter(|token| !token.is_empty() && !token.contains('\0'))
        .context("The provider returned no valid access token")?;
    Ok(Tokens {
        access_token: Zeroizing::new(access_token),
        expires_in: document.expires_in.unwrap_or(DEFAULT_EXPIRES_IN),
        refresh_token: document
            .refresh_token
            .take()
            .filter(|token| !token.is_empty())
            .map(Zeroizing::new),
        id_token: document.id_token.take().map(Zeroizing::new),
        scope: document.scope.take(),
    })
}

/// The configured scopes that the provider did not grant. `offline_access`
/// is optional: without it, the provider can omit the refresh token.
pub fn missing_scopes(config: &SignIn, granted: Option<&str>) -> Vec<String> {
    let Some(granted) = granted else {
        return vec![];
    };
    let granted: Vec<&str> = granted.split_ascii_whitespace().collect();
    config
        .scopes
        .iter()
        .filter(|scope| *scope != "offline_access" && !granted.contains(&scope.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn config(port: u16) -> SignIn {
        SignIn {
            name: "Test".into(),
            issuer: "https://id.example.test".into(),
            client_id: "qrow-desktop".into(),
            scopes: vec!["kyuubi".into()],
            resource: Some("https://kyuubi.example.test".into()),
            allowed_hosts: vec!["kyuubi.example.test".into()],
            callback_ports: if port == 0 { vec![] } else { vec![port] },
            ..SignIn::default()
        }
    }

    #[test]
    fn the_callback_uses_the_first_free_port() {
        let busy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let busy_port = busy.local_addr().unwrap().port();
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let free_port = free.local_addr().unwrap().port();
        drop(free);
        let listener = bind_callback(&[busy_port, free_port]).unwrap();
        assert_eq!(listener.local_addr().unwrap().port(), free_port);
        let error = bind_callback(&[busy_port]).unwrap_err().to_string();
        assert!(error.contains(&format!("port {busy_port}")), "{error}");
    }

    fn discovery() -> Discovery {
        Discovery {
            issuer: "https://id.example.test".into(),
            authorization_endpoint: Url::parse("https://id.example.test/auth?prompt=login")
                .unwrap(),
            token_endpoint: Url::parse("https://id.example.test/token").unwrap(),
            jwks_uri: Url::parse("https://id.example.test/keys").unwrap(),
        }
    }

    fn get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn query(attempt: &Attempt, name: &str) -> String {
        attempt
            .url()
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .unwrap()
    }

    #[test]
    fn the_authorization_url_uses_pkce_state_nonce_and_the_loopback_callback() {
        let attempt = Attempt::prepare(&config(0), &discovery()).unwrap();
        let port = attempt.listener.local_addr().unwrap().port();
        assert!(attempt.listener.local_addr().unwrap().ip().is_loopback());
        assert_eq!(query(&attempt, "prompt"), "login");
        assert_eq!(query(&attempt, "response_type"), "code");
        assert_eq!(query(&attempt, "client_id"), "qrow-desktop");
        assert_eq!(
            query(&attempt, "redirect_uri"),
            format!("http://127.0.0.1:{port}/callback")
        );
        assert_eq!(query(&attempt, "scope"), "openid kyuubi");
        assert_eq!(query(&attempt, "code_challenge_method"), "S256");
        assert_eq!(
            query(&attempt, "code_challenge"),
            code_challenge(&attempt.verifier)
        );
        assert_eq!(query(&attempt, "resource"), "https://kyuubi.example.test");
        assert_eq!(query(&attempt, "state"), *attempt.state);
        assert_eq!(query(&attempt, "nonce"), *attempt.nonce);
        let other = Attempt::prepare(&config(0), &discovery()).unwrap();
        assert_ne!(*other.state, *attempt.state);
        assert_ne!(*other.nonce, *attempt.nonce);
        assert_ne!(*other.verifier, *attempt.verifier);
    }

    #[test]
    fn pkce_challenge_matches_rfc_7636() {
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn the_callback_ignores_other_requests_and_accepts_the_matching_state() {
        let attempt = Arc::new(Attempt::prepare(&config(0), &discovery()).unwrap());
        let port = attempt.listener.local_addr().unwrap().port();
        let state = attempt.state.to_string();
        let waiter = thread::spawn(move || {
            attempt
                .wait(&AtomicBool::new(false), Duration::from_secs(10))
                .map(|code| code.to_string())
        });
        assert!(get(port, "/favicon.ico").starts_with("HTTP/1.1 404"));
        assert!(get(port, "/callback?code=evil&state=wrong").starts_with("HTTP/1.1 400"));
        let accepted = get(port, &format!("/callback?code=the-code&state={state}"));
        assert!(accepted.starts_with("HTTP/1.1 200"));
        assert_eq!(waiter.join().unwrap().unwrap(), "the-code");
    }

    #[test]
    fn the_callback_reports_provider_errors_and_issuer_mix_ups() {
        for (query, reason) in [
            ("error=access_denied", "denied"),
            ("error=server_error", "server_error"),
            ("iss=https%3A%2F%2Fevil.test&code=c", "another provider"),
            ("", "no authorization code"),
        ] {
            let attempt = Arc::new(Attempt::prepare(&config(0), &discovery()).unwrap());
            let port = attempt.listener.local_addr().unwrap().port();
            let target = format!("/callback?state={}&{query}", *attempt.state);
            let waiter = {
                let attempt = attempt.clone();
                thread::spawn(move || {
                    attempt
                        .wait(&AtomicBool::new(false), Duration::from_secs(10))
                        .map(|_| ())
                })
            };
            get(port, &target);
            let error = waiter.join().unwrap().unwrap_err().to_string();
            assert!(error.contains(reason), "{error} does not contain {reason}");
        }
    }

    #[test]
    fn the_wait_ends_on_cancellation_and_timeout_and_closes_the_listener() {
        let attempt = Attempt::prepare(&config(0), &discovery()).unwrap();
        let port = attempt.listener.local_addr().unwrap().port();
        let error = attempt
            .wait(&AtomicBool::new(true), Duration::from_secs(10))
            .unwrap_err();
        assert_eq!(super::super::failure(&error), Failure::Cancelled);
        let error = attempt
            .wait(&AtomicBool::new(false), Duration::ZERO)
            .unwrap_err();
        assert!(error.to_string().contains("in time"));
        drop(attempt);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    }

    #[test]
    fn optional_offline_access_and_missing_scope_lists() {
        let mut config = config(0);
        config.scopes = vec!["kyuubi".into(), "offline_access".into()];
        assert!(missing_scopes(&config, Some("openid kyuubi")).is_empty());
        assert!(missing_scopes(&config, None).is_empty());
        assert_eq!(missing_scopes(&config, Some("openid")), vec!["kyuubi"]);
    }
}
