//! A small HTTPS client for provider requests: discovery, keys, and tokens.
//! It sends one request for each connection and follows no redirects.
use crate::{connector::sasl::connect_tcp, tls::Trust};
use anyhow::{Context, Result};
use std::{
    io::{self, Read, Write},
    time::Duration,
};
use url::Url;
use zeroize::Zeroizing;

/// The limit for one socket read or write.
const IO_TIMEOUT: Duration = Duration::from_secs(15);
/// The limit for the response head and body.
const MAX_RESPONSE: usize = 1024 * 1024;

pub struct Response {
    pub status: u16,
    /// The `Location` header of a redirect. Qrow does not follow redirects.
    pub location: Option<String>,
    /// The body can contain tokens.
    pub body: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("body", &format_args!("{} bytes", self.body.len()))
            .finish()
    }
}

/// Requires HTTPS, so that provider responses and tokens are protected.
pub fn require_https(url: &Url) -> Result<()> {
    anyhow::ensure!(
        url.scheme() == "https" && url.host_str().is_some(),
        "The provider gave an endpoint that does not use HTTPS: {}",
        url.origin().ascii_serialization()
    );
    Ok(())
}

pub fn get(trust: &Trust, url: &Url) -> Result<Response> {
    request(trust, "GET", url, None)
}

/// Sends an `application/x-www-form-urlencoded` body.
pub fn post_form(trust: &Trust, url: &Url, form: &[(&str, &str)]) -> Result<Response> {
    let mut body = Zeroizing::new(String::new());
    {
        let mut serializer = form_urlencoded::Serializer::new(&mut *body);
        for (name, value) in form {
            serializer.append_pair(name, value);
        }
        serializer.finish();
    }
    request(trust, "POST", url, Some(&body))
}

fn request(trust: &Trust, method: &str, url: &Url, body: Option<&str>) -> Result<Response> {
    require_https(url)?;
    let host = url.host_str().context("The URL has no host")?;
    // Url keeps the brackets of an IPv6 address, but name resolution and TLS
    // need the address alone.
    let name = host.trim_start_matches('[').trim_end_matches(']');
    let port = url.port_or_known_default().unwrap_or(443);
    let origin = url.origin().ascii_serialization();
    let tcp = connect_tcp(name, port).with_context(|| format!("Could not reach {origin}"))?;
    tcp.set_read_timeout(Some(IO_TIMEOUT))?;
    tcp.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut stream = trust.connect(name, tcp)?;
    let target = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let mut head = format!(
        "{method} {target} HTTP/1.1\r\nHost: {authority}\r\nAccept: application/json\r\nUser-Agent: Qrow/{}\r\nConnection: close\r\n",
        env!("CARGO_PKG_VERSION")
    );
    if let Some(body) = body {
        head.push_str(&format!(
            "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    head.push_str("\r\n");
    let mut message = Zeroizing::new(head.into_bytes());
    if let Some(body) = body {
        message.extend_from_slice(body.as_bytes());
    }
    stream
        .write_all(&message)
        .and_then(|()| stream.flush())
        .with_context(|| format!("Could not send the request to {origin}"))?;
    read_response(&mut stream).with_context(|| format!("Could not read the response of {origin}"))
}

/// Reads until the response is complete. A server can close a TLS
/// connection without a closing message, so the end of the stream counts
/// only after a complete response, or for a response without a length.
pub(super) fn read_response(stream: &mut impl Read) -> Result<Response> {
    let mut data = Zeroizing::new(Vec::new());
    let mut buffer = Zeroizing::new([0; 16 * 1024]);
    loop {
        let read = match stream.read(&mut buffer[..]) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => 0,
            Err(error) => return Err(error.into()),
        };
        if read > 0 {
            data.extend_from_slice(&buffer[..read]);
            anyhow::ensure!(data.len() <= MAX_RESPONSE, "The response exceeds 1 MiB");
        }
        if let Some(response) = parse(&data, read == 0)? {
            return Ok(response);
        }
        anyhow::ensure!(read > 0, "The connection closed before the response ended");
    }
}

/// Returns the response when `data` holds all of it.
fn parse(data: &[u8], closed: bool) -> Result<Option<Response>> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut response = httparse::Response::new(&mut headers);
    let head = match response
        .parse(data)
        .context("The response is not valid HTTP")?
    {
        httparse::Status::Complete(head) => head,
        httparse::Status::Partial => return Ok(None),
    };
    let status = response.code.context("The response has no status")?;
    let header = |name: &str| {
        response
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| String::from_utf8_lossy(header.value).trim().to_owned())
    };
    let location = header("location");
    let rest = &data[head..];
    if header("transfer-encoding").is_some_and(|value| value.eq_ignore_ascii_case("chunked")) {
        return Ok(dechunk(rest)?.map(|body| Response {
            status,
            location,
            body: Zeroizing::new(body),
        }));
    }
    if let Some(length) = header("content-length") {
        let length: usize = length.parse().context("Invalid Content-Length")?;
        anyhow::ensure!(length <= MAX_RESPONSE, "The response exceeds 1 MiB");
        return Ok((rest.len() >= length).then(|| Response {
            status,
            location,
            body: Zeroizing::new(rest[..length].to_vec()),
        }));
    }
    Ok(closed.then(|| Response {
        status,
        location,
        body: Zeroizing::new(rest.to_vec()),
    }))
}

/// Decodes a chunked body, or returns `None` while it is incomplete.
fn dechunk(mut data: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut body = Vec::new();
    loop {
        let Some(line) = data.windows(2).position(|pair| pair == b"\r\n") else {
            return Ok(None);
        };
        let size = std::str::from_utf8(&data[..line])
            .ok()
            .and_then(|text| text.split(';').next())
            .and_then(|size| usize::from_str_radix(size.trim(), 16).ok())
            .context("Invalid chunk size")?;
        anyhow::ensure!(
            size <= MAX_RESPONSE && body.len() + size <= MAX_RESPONSE,
            "The response exceeds 1 MiB"
        );
        data = &data[line + 2..];
        if size == 0 {
            return Ok(Some(body));
        }
        if data.len() < size + 2 {
            return Ok(None);
        }
        body.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_content_length_chunked_and_close_delimited_bodies() {
        let response =
            read_response(&mut &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}extra"[..])
                .unwrap();
        assert_eq!((response.status, &response.body[..]), (200, &b"{}"[..]));
        let response = read_response(
            &mut &b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/callback\r\nContent-Length: 0\r\n\r\n"[..],
        )
        .unwrap();
        assert_eq!(
            response.location.as_deref(),
            Some("http://127.0.0.1:9/callback")
        );
        let response = read_response(
            &mut &b"HTTP/1.1 400 Bad Request\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x=y\r\nde\r\n0\r\n\r\n"[..],
        )
        .unwrap();
        assert_eq!((response.status, &response.body[..]), (400, &b"abcde"[..]));
        let response = read_response(&mut &b"HTTP/1.1 200 OK\r\n\r\nall"[..]).unwrap();
        assert_eq!(&response.body[..], b"all");
    }

    #[test]
    fn rejects_truncated_and_oversized_responses() {
        assert!(
            read_response(&mut &b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab"[..]).is_err()
        );
        assert!(
            read_response(
                &mut &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab"[..]
            )
            .is_err()
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            MAX_RESPONSE + 1
        );
        assert!(read_response(&mut head.as_bytes()).is_err());
        // A huge chunk size is an error, not an overflow.
        let huge = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffff\r\nab";
        assert!(read_response(&mut &huge[..]).is_err());
    }

    #[test]
    fn requires_https() {
        assert!(require_https(&Url::parse("http://example.test/token").unwrap()).is_err());
        assert!(require_https(&Url::parse("https://example.test/token").unwrap()).is_ok());
    }

    #[test]
    fn debug_output_does_not_show_the_body() {
        let response = Response {
            status: 200,
            location: None,
            body: Zeroizing::new(b"{\"access_token\":\"secret\"}".to_vec()),
        };
        assert!(!format!("{response:?}").contains("secret"));
    }
}
