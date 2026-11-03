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
    pub transport: Arc<Transport>,
    requests: Mutex<Requests>,
    peer: Mutex<Option<SocketAddr>>,
    pub statement: Url,
    pub timeout: Duration,
    username: String,
    secret: Secret,
    trust: Trust,
    allow_authentication: AtomicBool,
    cleanup_token: Mutex<Option<zeroize::Zeroizing<String>>>,
}

#[derive(Clone)]
struct Requests {
    scope: Arc<Transport>,
    client: Option<Client>,
    runtime: Option<Arc<Runtime>>,
}

fn coordinator_client(
    scope: Arc<Transport>,
    timeout: Duration,
    tls: bool,
    trust: &Trust,
    allowance: Option<Arc<crate::export::budget::Allowance>>,
) -> Result<Client> {
    let mut builder = Client::builder()
        .tcp_connection_control(move |socket| {
            let lease = scope.register(socket)?;
            Ok(Arc::new((lease, allowance.clone())) as Arc<dyn Send + Sync>)
        })
        .http1_max_buf_size(1024 * 1024)
        .timeout(timeout)
        .connect_timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if tls {
        builder = builder.use_preconfigured_tls((*trust.client_config()?).clone());
    }
    Ok(builder.build()?)
}

impl Http {
    pub fn new(profile: &Profile, secret: Secret, trust: &Trust) -> Result<Self> {
        anyhow::ensure!(profile.username.len() <= 1024, "Trino user exceeds 1 KiB");
        if let Secret::Password(password) = &secret {
            anyhow::ensure!(
                password.len() <= 64 * 1024,
                "Trino credential exceeds 64 KiB"
            );
        }
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
        let scope = transport.child();
        let client = coordinator_client(scope.clone(), timeout, profile.tls, trust, None)?;
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
            requests: Mutex::new(Requests {
                scope,
                client: Some(client),
                runtime: Some(Arc::new(Runtime::new()?)),
            }),
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

    pub fn segment_control(
        &self,
        cancel: &super::Cancel,
        allowance: Arc<crate::export::budget::Allowance>,
        external: Option<Arc<AtomicBool>>,
    ) -> super::segment_download::Control {
        super::segment_download::Control {
            scope: cancel.requests.clone(),
            requested: cancel.requested.clone(),
            external,
            allowance,
            trust: self.trust.clone(),
            timeout: self.timeout,
            threads: cancel.threads.clone(),
        }
    }

    pub fn disable_authentication(&self) {
        self.allow_authentication.store(false, Ordering::SeqCst);
    }

    pub fn begin_requests(
        &self,
        allowance: Option<Arc<crate::export::budget::Allowance>>,
    ) -> Result<Arc<Transport>> {
        let scope = self.transport.child();
        let client = coordinator_client(
            scope.clone(),
            self.timeout,
            self.statement.scheme() == "https",
            &self.trust,
            allowance,
        )?;
        let runtime = Arc::new(Runtime::new()?);
        *self.requests.lock().unwrap() = Requests {
            scope: scope.clone(),
            client: Some(client),
            runtime: Some(runtime),
        };
        Ok(scope)
    }

    pub fn end_requests(&self, scope: &Arc<Transport>) {
        let mut requests = self.requests.lock().unwrap();
        if Arc::ptr_eq(&requests.scope, scope) {
            requests.client = None;
            requests.runtime = None;
        }
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
        requests: &Requests,
        spooling: bool,
    ) -> Result<reqwest::Response> {
        self.cursor(url.as_str())?;
        anyhow::ensure!(!self.transport.is_closed(), crate::export::Cancelled);
        let authenticate = self.allow_authentication.load(Ordering::SeqCst);
        let mut request = requests
            .client
            .as_ref()
            .context("Trino operation is closed")?
            .request(method, url.clone())
            .headers(headers.request()?)
            .header("X-Trino-Source", "Qrow")
            .header("X-Trino-Client-Capabilities", "PARAMETRIC_DATETIME")
            .header("X-Trino-User", &self.username)
            .header("Accept", "application/json");
        if spooling {
            request = request.header("X-Trino-Query-Data-Encoding", "json+lz4,json");
        }
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
                anyhow::ensure!(token.len() <= 64 * 1024, "Trino token exceeds 64 KiB");
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
                anyhow::ensure!(token.len() <= 64 * 1024, "Trino token exceeds 64 KiB");
                *self.cleanup_token.lock().unwrap() =
                    Some(zeroize::Zeroizing::new(token.as_str().to_owned()));
            }
            let mut current = request.try_clone().context("Cannot retry Trino request")?;
            if let Some(token) = &token {
                current = current.bearer_auth(token.as_str());
            }
            let response = requests
                .runtime
                .as_ref()
                .context("Trino operation is closed")?
                .block_on(requests.scope.run(async {
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
        self.page_encoded(method, url, sql, headers, false)
    }

    pub fn page_encoded(
        &self,
        method: Method,
        url: &Url,
        sql: Option<&str>,
        headers: &SessionHeaders,
        spooling: bool,
    ) -> Result<(Page, HeaderMap)> {
        let requests = self.requests.lock().unwrap().clone();
        let _activity = requests.scope.activity()?;
        let mut response = self.request(method, url, sql, headers, &requests, spooling)?;
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
        let bytes = requests
            .runtime
            .as_ref()
            .context("Trino operation is closed")?
            .block_on(requests.scope.run(async {
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

#[derive(Clone, Default)]
pub(super) struct SessionHeaders {
    catalog: String,
    schema: String,
    properties: BTreeMap<String, String>,
    roles: BTreeMap<String, String>,
    prepared: BTreeMap<String, String>,
    transaction: Option<String>,
}

const MAX_HEADER_STATE: usize = 256 * 1024;
const MAX_REQUEST_HEADERS: usize = 512 * 1024;
const MAX_HEADER_ENTRIES: usize = 1024;

fn encode(text: &str) -> String {
    form_urlencoded::byte_serialize(text.as_bytes()).collect()
}
fn decode(text: &str, limit: usize) -> Result<String> {
    anyhow::ensure!(
        text.len() <= limit * 3,
        "Trino session header exceeds its size limit"
    );
    let text = form_urlencoded::parse(format!("value={text}").as_bytes())
        .next()
        .unwrap()
        .1
        .into_owned();
    anyhow::ensure!(
        text.len() <= limit,
        "Trino session header exceeds its size limit"
    );
    Ok(text)
}

impl SessionHeaders {
    pub fn new(profile: &Profile) -> Result<Self> {
        anyhow::ensure!(
            profile.database.len() <= 1024 && profile.trino_schema.len() <= 1024,
            "Trino catalog or schema exceeds 1 KiB"
        );
        anyhow::ensure!(
            profile.parameters.len() <= MAX_HEADER_ENTRIES,
            "Trino session exceeds 1024 settings"
        );
        let mut bytes = profile.database.len() + profile.trino_schema.len();
        for (name, value) in &profile.parameters {
            anyhow::ensure!(
                name.len() <= 1024 && value.len() <= 64 * 1024,
                "Trino session setting exceeds its size limit"
            );
            bytes += name.len() + value.len();
            anyhow::ensure!(
                bytes <= MAX_HEADER_STATE,
                "Trino session state exceeds 256 KiB"
            );
        }
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

    fn validate(&self) -> Result<()> {
        let mut bytes = 0;
        for value in [&self.catalog, &self.schema]
            .into_iter()
            .chain(self.transaction.iter())
        {
            anyhow::ensure!(
                value.len() <= 1024,
                "Trino catalog, schema or transaction exceeds 1 KiB"
            );
            bytes += value.capacity();
        }
        let mut count = 0;
        for map in [&self.properties, &self.roles, &self.prepared] {
            count += map.len();
            for (name, value) in map {
                anyhow::ensure!(
                    name.len() <= 1024 && value.len() <= 64 * 1024,
                    "Trino session setting exceeds its size limit"
                );
                bytes += name.capacity() + value.capacity();
            }
        }
        anyhow::ensure!(
            count <= MAX_HEADER_ENTRIES && bytes <= MAX_HEADER_STATE,
            "Trino session state exceeds 1024 settings or 256 KiB"
        );
        Ok(())
    }

    pub fn request(&self) -> Result<HeaderMap> {
        self.validate()?;
        let mut headers = HeaderMap::new();
        let mut total = 0;
        for (name, value) in [
            ("x-trino-catalog", self.catalog.as_str()),
            ("x-trino-schema", self.schema.as_str()),
            (
                "x-trino-transaction-id",
                self.transaction.as_deref().unwrap_or("NONE"),
            ),
        ] {
            total += value.len();
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
            let size = map
                .iter()
                .map(|(key, value)| {
                    form_urlencoded::byte_serialize(key.as_bytes())
                        .map(str::len)
                        .sum::<usize>()
                        + 1
                        + form_urlencoded::byte_serialize(value.as_bytes())
                            .map(str::len)
                            .sum::<usize>()
                })
                .sum::<usize>()
                + map.len().saturating_sub(1);
            total += size;
            anyhow::ensure!(
                total <= MAX_REQUEST_HEADERS,
                "Trino request session headers exceed 512 KiB"
            );
            let mut value = String::with_capacity(size);
            for (key, item) in map {
                if !value.is_empty() {
                    value.push(',');
                }
                value.push_str(&encode(key));
                value.push('=');
                value.push_str(&encode(item));
            }
            if !value.is_empty() {
                headers.insert(name, HeaderValue::from_str(&value)?);
            }
        }
        Ok(headers)
    }

    pub fn apply(&mut self, headers: &HeaderMap) -> Result<()> {
        anyhow::ensure!(
            headers.values().map(HeaderValue::len).sum::<usize>() <= MAX_REQUEST_HEADERS,
            "Trino response headers exceed 512 KiB"
        );
        let mut candidate = self.clone();
        for (name, target) in [
            ("x-trino-set-catalog", &mut candidate.catalog),
            ("x-trino-set-schema", &mut candidate.schema),
        ] {
            if let Some(value) = headers.get(name) {
                let value = value.to_str()?;
                anyhow::ensure!(value.len() <= 1024, "Trino catalog or schema exceeds 1 KiB");
                *target = value.to_owned();
            }
        }
        for name in [
            "x-trino-set-session",
            "x-trino-set-role",
            "x-trino-added-prepare",
        ] {
            for value in headers.get_all(name) {
                for pair in value.to_str()?.split(',') {
                    let (key, value) = pair
                        .trim()
                        .split_once('=')
                        .context("Invalid Trino session response header")?;
                    let key = decode(key, 1024)?;
                    let value = decode(value, 64 * 1024)?;
                    // At most one bounded candidate entry can overlap the bound.
                    let map = match name {
                        "x-trino-set-session" => &mut candidate.properties,
                        "x-trino-set-role" => &mut candidate.roles,
                        _ => &mut candidate.prepared,
                    };
                    map.insert(key, value);
                    candidate.validate()?;
                }
            }
        }
        for name in ["x-trino-clear-session", "x-trino-deallocated-prepare"] {
            for value in headers.get_all(name) {
                for key in value.to_str()?.split(',') {
                    let key = decode(key.trim(), 1024)?;
                    let map = if name == "x-trino-clear-session" {
                        &mut candidate.properties
                    } else {
                        &mut candidate.prepared
                    };
                    map.remove(&key);
                }
            }
        }
        if let Some(value) = headers.get("x-trino-started-transaction-id") {
            let value = value.to_str()?;
            anyhow::ensure!(value.len() <= 1024, "Trino transaction exceeds 1 KiB");
            candidate.transaction = Some(value.to_owned());
        }
        if headers.contains_key("x-trino-clear-transaction-id") {
            candidate.transaction = None;
        }
        candidate.request()?;
        *self = candidate;
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

    #[test]
    fn session_headers_bound_cumulative_state_and_commit_updates_together() -> Result<()> {
        let mut headers = SessionHeaders::default();
        let mut response = HeaderMap::new();
        for index in 0..MAX_HEADER_ENTRIES {
            response.append(
                "x-trino-set-session",
                HeaderValue::from_str(&format!("k{index}=v"))?,
            );
        }
        headers.apply(&response)?;
        let before = headers.request()?;
        let mut response = HeaderMap::new();
        response.insert(
            "x-trino-set-schema",
            HeaderValue::from_static("must_not_commit"),
        );
        response.insert(
            "x-trino-added-prepare",
            HeaderValue::from_static("one_more=SELECT+1"),
        );
        assert!(headers.apply(&response).is_err());
        assert_eq!(headers.request()?, before);
        let mut headers = SessionHeaders::default();
        for index in 0..3 {
            let mut response = HeaderMap::new();
            response.insert(
                "x-trino-added-prepare",
                HeaderValue::from_str(&format!("s{index}={}", "a".repeat(64 * 1024)))?,
            );
            headers.apply(&response)?;
        }
        let before = headers.request()?;
        let mut response = HeaderMap::new();
        response.insert(
            "x-trino-added-prepare",
            HeaderValue::from_str(&format!("overflow={}", "a".repeat(64 * 1024)))?,
        );
        assert!(headers.apply(&response).is_err());
        assert_eq!(headers.request()?, before);
        Ok(())
    }

    #[test]
    fn percent_expansion_is_bounded_before_request_headers_are_built() -> Result<()> {
        let mut headers = SessionHeaders::default();
        for index in 0..3 {
            headers
                .prepared
                .insert(format!("s{index}"), "🦀".repeat(16 * 1024));
        }
        assert!(headers.validate().is_ok());
        assert!(headers.request().is_err());
        Ok(())
    }
}
