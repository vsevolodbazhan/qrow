use super::{
    Page, external,
    transport::{Runtime, Transport},
};
use crate::{connector::Secret, model::Profile, tls::Trust};
use anyhow::{Context, Result};
use reqwest::{
    Client, Method,
    header::{HeaderMap, HeaderValue},
};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use url::Url;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct Http {
    client: Client,
    runtime: Runtime,
    pub transport: Arc<Transport>,
    requests: Mutex<Arc<Transport>>,
    peer: Mutex<Option<SocketAddr>>,
    pub statement: Url,
    pub timeout: Duration,
    username: String,
    secret: Secret,
    trust: Trust,
    allow_authentication: AtomicBool,
    cleanup_token: Mutex<Option<zeroize::Zeroizing<String>>>,
}

impl Http {
    pub fn new(profile: &Profile, secret: Secret, trust: &Trust) -> Result<Self> {
        let mut statement = Url::parse(if profile.tls {
            "https://localhost"
        } else {
            "http://localhost"
        })?;
        if let Ok(address) = profile.host.parse::<std::net::IpAddr>() {
            statement
                .set_ip_host(address)
                .map_err(|()| anyhow::anyhow!("Invalid Trino IP address"))?;
        } else {
            statement
                .set_host(Some(&profile.host))
                .context("Invalid Trino host")?;
        }
        statement
            .set_port(Some(profile.port))
            .map_err(|()| anyhow::anyhow!("Invalid Trino port"))?;
        statement.set_path("/v1/statement");
        let timeout = Duration::from_secs(profile.lifecycle.response_timeout_seconds);
        let transport = Transport::new();
        let sockets = transport.clone();
        let mut builder = Client::builder()
            .tcp_connection_control(move |socket| sockets.register(socket))
            .http1_max_buf_size(1024 * 1024)
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy();
        if profile.tls {
            builder = builder.use_preconfigured_tls((*trust.client_config()?).clone());
        }
        let secret = match secret {
            Secret::External(source) => {
                let sockets = transport.clone();
                let control = source
                    .control()
                    .clone()
                    .with_cancel(Arc::new(move || sockets.is_closed()));
                Secret::External(source.with_control(control))
            }
            secret => secret,
        };
        let http = Self {
            client: builder.build()?,
            runtime: Runtime::new()?,
            requests: Mutex::new(transport.child()),
            transport,
            peer: Mutex::default(),
            statement,
            timeout,
            username: profile.username.clone(),
            secret,
            trust: trust.clone(),
            allow_authentication: AtomicBool::new(true),
            cleanup_token: Mutex::default(),
        };
        // Authenticated Trino coordinators require HTTPS. Never send a credential
        // to a plain endpoint, including through a server-provided nextUri.
        anyhow::ensure!(
            profile.tls
                || matches!(&http.secret, Secret::Password(password) if password.is_empty()),
            "Trino passwords and sign-in tokens require TLS."
        );
        Ok(http)
    }

    pub fn disable_authentication(&self) {
        self.allow_authentication.store(false, Ordering::SeqCst);
    }

    pub fn begin_requests(&self) -> Arc<Transport> {
        let scope = self.transport.child();
        *self.requests.lock().unwrap() = scope.clone();
        scope
    }

    #[cfg(test)]
    pub fn has_peer(&self) -> bool {
        self.peer.lock().unwrap().is_some()
    }

    pub fn cursor(&self, text: &str) -> Result<Url> {
        let cursor = Url::parse(text).context("Trino returned an invalid result URL")?;
        anyhow::ensure!(
            cursor.origin() == self.statement.origin()
                && cursor.username().is_empty()
                && cursor.password().is_none()
                && cursor.fragment().is_none(),
            "Trino returned a result URL outside the coordinator origin"
        );
        Ok(cursor)
    }

    fn request(
        &self,
        method: Method,
        url: &Url,
        sql: Option<&str>,
        headers: &SessionHeaders,
        authenticate: bool,
        scope: &Transport,
    ) -> Result<reqwest::Response> {
        self.cursor(url.as_str())?;
        anyhow::ensure!(!self.transport.is_closed(), crate::export::Cancelled);
        let authenticate = authenticate && self.allow_authentication.load(Ordering::SeqCst);
        let mut request = self
            .client
            .request(method, url.clone())
            .headers(headers.request()?)
            .header("X-Trino-Source", "Qrow")
            .header("X-Trino-Client-Capabilities", "PARAMETRIC_DATETIME")
            .header("X-Trino-User", &self.username)
            .header("Accept", "application/json");
        match &self.secret {
            Secret::Password(password) if !password.is_empty() => {
                request = request.basic_auth(&self.username, Some(password.as_str()));
            }
            Secret::Token(_) => {
                let token = if authenticate {
                    self.secret.value()?
                } else {
                    self.cleanup_token
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|token| zeroize::Zeroizing::new(token.as_str().to_owned()))
                        .context("No cached Trino authentication token for cleanup")?
                };
                *self.cleanup_token.lock().unwrap() =
                    Some(zeroize::Zeroizing::new(token.as_str().to_owned()));
                request = request.bearer_auth(token.as_str());
            }
            _ => {}
        }
        if let Some(sql) = sql {
            request = request
                .header("Content-Type", "text/plain; charset=utf-8")
                .body(sql.to_owned());
        }
        let mut token = match &self.secret {
            Secret::External(source) if authenticate => source.cached()?,
            Secret::External(source) => source.cached().ok().flatten().or_else(|| {
                self.cleanup_token
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|token| zeroize::Zeroizing::new(token.as_str().to_owned()))
            }),
            _ => None,
        };
        // Rebuild only after an explicit rejection. A network error returns at
        // once because repeating a submission could execute SQL twice.
        for attempt in 0..=2 {
            if authenticate
                && let Secret::External(source) = &self.secret
                && source.control().is_cancelled()
            {
                return Err(crate::external_auth::Cancelled.into());
            }
            if let Some(token) = &token {
                *self.cleanup_token.lock().unwrap() =
                    Some(zeroize::Zeroizing::new(token.as_str().to_owned()));
            }
            let mut current = request.try_clone().context("Cannot retry Trino request")?;
            if let Some(token) = &token {
                current = current.bearer_auth(token.as_str());
            }
            let response = self.runtime.block_on(scope.run(async {
                current.send().await.map_err(|error| {
                    anyhow::Error::new(error.without_url()).context("Trino request failed")
                })
            }))?;
            *self.peer.lock().unwrap() = response.remote_addr();
            if response.status() != reqwest::StatusCode::UNAUTHORIZED || !authenticate {
                return Ok(response);
            }
            let Secret::External(source) = &self.secret else {
                return Ok(response);
            };
            if attempt == 2 {
                source.reject(token.as_deref().map(String::as_str));
                anyhow::bail!("Trino rejected the replacement authentication token");
            }
            source.reject(token.as_deref().map(String::as_str));
            let challenge = external::challenge(response.headers(), &self.statement)?;
            // A separate pool keeps poll dispatchers on the authentication runtime.
            let authentication = Transport::new();
            let sockets = authentication.clone();
            let client = Client::builder()
                .tcp_connection_control(move |socket| sockets.register(socket))
                .http1_max_buf_size(1024 * 1024)
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .use_preconfigured_tls((*self.trust.client_config()?).clone())
                .build()?;
            let coordinator = self.statement.clone();
            let browser = source.browser();
            let timeout = source.timeout();
            token = Some(source.authenticate(
                token.as_deref().map(String::as_str),
                timeout,
                move |cancel| {
                    external::authenticate(
                        client,
                        coordinator,
                        challenge,
                        browser,
                        cancel,
                        timeout,
                        authentication,
                    )
                },
            )?);
        }
        unreachable!("bounded authentication attempts")
    }

    pub fn page(
        &self,
        method: Method,
        url: &Url,
        sql: Option<&str>,
        headers: &SessionHeaders,
    ) -> Result<(Page, HeaderMap)> {
        let scope = self.requests.lock().unwrap().clone();
        let _activity = scope.activity()?;
        let mut response = self.request(method, url, sql, headers, true, &scope)?;
        anyhow::ensure!(
            response.status().as_u16() == 200,
            "Trino returned HTTP {}",
            response.status().as_u16()
        );
        let headers = response.headers().clone();
        if let Some(length) = response.content_length() {
            anyhow::ensure!(
                length <= MAX_RESPONSE_BYTES as u64,
                "Trino response exceeds 16 MiB"
            );
        }
        let bytes = self.runtime.block_on(scope.run(async {
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| error.without_url())?
            {
                anyhow::ensure!(
                    bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES,
                    "Trino response exceeds 16 MiB"
                );
                bytes.extend_from_slice(&chunk);
            }
            Ok::<_, anyhow::Error>(bytes)
        }))?;
        Ok((
            serde_json::from_slice(&bytes).context("Invalid Trino result response")?,
            headers,
        ))
    }

    /// Cleanup uses the established coordinator peer and cached credentials.
    pub fn delete(&self, url: &Url, deadline: Instant) -> Result<()> {
        let response = self.auxiliary(
            Method::DELETE,
            url,
            &SessionHeaders::default(),
            deadline,
            &self.transport.child(),
        )?;
        anyhow::ensure!(
            response.status().is_success()
                || response.status().as_u16() == 404
                || response.status().as_u16() == 410,
            "Trino cancellation returned HTTP {}",
            response.status().as_u16()
        );
        Ok(())
    }

    pub fn auxiliary(
        &self,
        method: Method,
        url: &Url,
        headers: &SessionHeaders,
        deadline: Instant,
        transport: &Arc<Transport>,
    ) -> Result<reqwest::Response> {
        self.cursor(url.as_str())?;
        let peer = self
            .peer
            .lock()
            .unwrap()
            .context("No established Trino coordinator peer for cleanup")?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(!remaining.is_zero(), "Trino cleanup deadline expired");
        let sockets = transport.clone();
        let mut builder = Client::builder()
            .tcp_connection_control(move |socket| sockets.register(socket))
            .http1_max_buf_size(1024 * 1024)
            .timeout(remaining)
            .connect_timeout(remaining)
            .resolve_to_addrs(self.statement.host_str().context("No Trino host")?, &[peer])
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy();
        if self.statement.scheme() == "https" {
            builder = builder.use_preconfigured_tls((*self.trust.client_config()?).clone());
        }
        let client = builder.build()?;
        let runtime = Runtime::new()?;
        let mut request = client
            .request(method, url.clone())
            .headers(headers.request()?)
            .header("X-Trino-Source", "Qrow")
            .header("X-Trino-User", &self.username)
            .header("X-Trino-Client-Capabilities", "PARAMETRIC_DATETIME");
        if let Secret::Password(password) = &self.secret {
            if !password.is_empty() {
                request = request.basic_auth(&self.username, Some(password.as_str()));
            }
        } else if let Some(token) = self.cleanup_token.lock().unwrap().as_ref() {
            request = request.bearer_auth(token.as_str());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(!remaining.is_zero(), "Trino cleanup deadline expired");
        let request = request.timeout(remaining);
        runtime.block_on(transport.run(async {
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
                request.send().await.map_err(|error| {
                    anyhow::Error::new(error.without_url()).context("Trino cleanup request failed")
                })
            })
            .await
            .context("Trino cleanup deadline expired")?
        }))
    }
}

#[derive(Default)]
pub(super) struct SessionHeaders {
    catalog: String,
    schema: String,
    properties: BTreeMap<String, String>,
    roles: BTreeMap<String, String>,
    prepared: BTreeMap<String, String>,
    transaction: Option<String>,
}

fn encode(text: &str) -> String {
    form_urlencoded::byte_serialize(text.as_bytes()).collect()
}
fn decode(text: &str) -> String {
    form_urlencoded::parse(format!("value={text}").as_bytes())
        .next()
        .unwrap()
        .1
        .into_owned()
}

impl SessionHeaders {
    pub fn new(profile: &Profile) -> Result<Self> {
        let headers = Self {
            catalog: profile.database.clone(),
            schema: profile.trino_schema.clone(),
            properties: profile.parameters.clone(),
            ..Self::default()
        };
        headers.request()?;
        Ok(headers)
    }

    pub fn in_transaction(&self) -> bool {
        self.transaction.is_some()
    }

    pub fn request(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("x-trino-catalog", self.catalog.as_str()),
            ("x-trino-schema", self.schema.as_str()),
            (
                "x-trino-transaction-id",
                self.transaction.as_deref().unwrap_or("NONE"),
            ),
        ] {
            if !value.is_empty() {
                headers.insert(
                    name,
                    HeaderValue::from_str(value)
                        .with_context(|| format!("Invalid Trino {name} setting"))?,
                );
            }
        }
        for (name, map) in [
            ("x-trino-session", &self.properties),
            ("x-trino-role", &self.roles),
            ("x-trino-prepared-statement", &self.prepared),
        ] {
            let value = map
                .iter()
                .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
                .collect::<Vec<_>>()
                .join(",");
            if !value.is_empty() {
                headers.insert(name, HeaderValue::from_str(&value)?);
            }
        }
        Ok(headers)
    }

    pub fn apply(&mut self, headers: &HeaderMap) -> Result<()> {
        for (name, target) in [
            ("x-trino-set-catalog", &mut self.catalog),
            ("x-trino-set-schema", &mut self.schema),
        ] {
            if let Some(value) = headers.get(name) {
                *target = value.to_str()?.to_owned();
            }
        }
        for (name, map) in [
            ("x-trino-set-session", &mut self.properties),
            ("x-trino-set-role", &mut self.roles),
            ("x-trino-added-prepare", &mut self.prepared),
        ] {
            for value in headers.get_all(name) {
                for pair in value.to_str()?.split(',') {
                    let (name, value) = pair
                        .trim()
                        .split_once('=')
                        .context("Invalid Trino session response header")?;
                    map.insert(decode(name), decode(value));
                }
            }
        }
        for (name, map) in [
            ("x-trino-clear-session", &mut self.properties),
            ("x-trino-deallocated-prepare", &mut self.prepared),
        ] {
            for value in headers.get_all(name) {
                for name in value.to_str()?.split(',') {
                    map.remove(&decode(name.trim()));
                }
            }
        }
        if let Some(value) = headers.get("x-trino-started-transaction-id") {
            self.transaction = Some(value.to_str()?.to_owned());
        }
        if headers.contains_key("x-trino-clear-transaction-id") {
            self.transaction = None;
        }
        self.request()?;
        Ok(())
    }
}

impl Drop for Http {
    fn drop(&mut self) {
        self.transport.close_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinator_urls_preserve_ipv6_addresses() -> Result<()> {
        for host in ["::1", "2001:db8::7", "[::1]"] {
            let profile = Profile {
                host: host.into(),
                port: 8080,
                ..Profile::default()
            };
            let http = Http::new(&profile, Secret::password(""), &Trust::default())?;
            let expected = host
                .trim_matches(['[', ']'])
                .parse::<std::net::Ipv6Addr>()?;
            assert_eq!(http.statement.host(), Some(url::Host::Ipv6(expected)));
            assert_eq!(http.statement.port(), Some(8080));
            assert_eq!(http.statement.path(), "/v1/statement");
        }
        Ok(())
    }
    #[test]
    fn session_headers_reject_injection_and_decode_properties() -> Result<()> {
        let profile = Profile {
            database: "tpch\r\nAuthorization: secret".into(),
            ..Profile::default()
        };
        assert!(SessionHeaders::new(&profile).is_err());
        let mut headers = SessionHeaders::default();
        let mut response = HeaderMap::new();
        response.append(
            "x-trino-set-session",
            HeaderValue::from_static("a=one%2Ctwo"),
        );
        response.append(
            "x-trino-set-session",
            HeaderValue::from_static("b=%CE%B1%F0%9F%A6%80"),
        );
        headers.apply(&response)?;
        assert_eq!(headers.properties["a"], "one,two");
        assert_eq!(headers.properties["b"], "α🦀");
        response.insert(
            "x-trino-set-session",
            HeaderValue::from_static("missing_equals"),
        );
        assert!(headers.apply(&response).is_err());
        Ok(())
    }
}
