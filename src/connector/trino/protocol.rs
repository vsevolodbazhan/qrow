use super::Page;
use crate::{connector::Secret, model::Profile, tls::Trust};
use anyhow::{Context, Result};
use reqwest::{
    Method,
    blocking::Client,
    header::{HeaderMap, HeaderValue},
};
use std::{collections::BTreeMap, io::Read, time::Duration};
use url::Url;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct Http {
    client: Client,
    pub statement: Url,
    pub timeout: Duration,
    username: String,
    secret: Secret,
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
        let mut builder = Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy();
        if profile.tls {
            builder = builder.use_preconfigured_tls((*trust.client_config()?).clone());
        }
        let http = Self {
            client: builder.build()?,
            statement,
            timeout,
            username: profile.username.clone(),
            secret,
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
    ) -> Result<reqwest::blocking::Response> {
        self.cursor(url.as_str())?;
        let mut request = self
            .client
            .request(method, url.clone())
            .headers(headers.request()?)
            .header("X-Trino-Source", "Qrow")
            .header("X-Trino-User", &self.username)
            .header("Accept", "application/json");
        match &self.secret {
            Secret::Password(password) if !password.is_empty() => {
                request = request.basic_auth(&self.username, Some(password.as_str()));
            }
            Secret::Token(_) => {
                request = request.bearer_auth(self.secret.value()?.as_str());
            }
            _ => {}
        }
        if let Some(sql) = sql {
            request = request
                .header("Content-Type", "text/plain; charset=utf-8")
                .body(sql.to_owned());
        }
        request.send().map_err(|error| {
            anyhow::Error::new(error.without_url()).context("Trino request failed")
        })
    }

    pub fn page(
        &self,
        method: Method,
        url: &Url,
        sql: Option<&str>,
        headers: &SessionHeaders,
    ) -> Result<(Page, HeaderMap)> {
        let response = self.request(method, url, sql, headers)?;
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
        let mut bytes = Vec::new();
        response
            .take((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() <= MAX_RESPONSE_BYTES,
            "Trino response exceeds 16 MiB"
        );
        Ok((
            serde_json::from_slice(&bytes).context("Invalid Trino result response")?,
            headers,
        ))
    }

    pub fn delete(&self, url: &Url) -> Result<()> {
        let response = self.request(Method::DELETE, url, None, &SessionHeaders::default())?;
        anyhow::ensure!(
            response.status().is_success()
                || response.status().as_u16() == 404
                || response.status().as_u16() == 410,
            "Trino cancellation returned HTTP {}",
            response.status().as_u16()
        );
        Ok(())
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
