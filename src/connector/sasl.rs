//! HiveServer2 SASL PLAIN negotiation and length-prefixed frames, over plain
//! TCP or TLS. Kyuubi verifies the password or access token, as with PyHive's
//! LDAP transport.
use super::{protocol::ResponseProtocol, t_c_l_i_service::TCLIServiceSyncClient};
use crate::tls::{TlsStream, Trust};
use anyhow::{Context, Result, ensure};
use std::{
    io::{self, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};
use thrift::protocol::TBinaryOutputProtocol;
use zeroize::Zeroizing;

pub const MAX_FRAME: usize = 64 * 1024 * 1024;
/// The limit for one TCP connection attempt to one resolved address.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The read timeout of a reader without one, like a test fixture.
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(300);
/// The limit for one socket write.
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);
pub type Client =
    TCLIServiceSyncClient<ResponseProtocol<Stream>, TBinaryOutputProtocol<FrameWriter<Stream>>>;

/// The server of a transport. Without `tls`, the transport is not encrypted.
pub struct Endpoint<'a> {
    pub host: &'a str,
    pub port: u16,
    pub tls: Option<&'a Trust>,
    /// The limit for each socket read. The connector polls the status of a
    /// running query, so this does not limit the query duration.
    pub read_timeout: Duration,
}

/// A connected socket. The reader and the writer of one Thrift client share
/// a TLS stream, and use it one after the other.
pub enum Stream {
    Plain(TcpStream),
    Tls(Arc<Mutex<TlsStream>>),
}

impl Stream {
    fn try_clone(&self) -> io::Result<Self> {
        match self {
            Self::Plain(stream) => stream.try_clone().map(Self::Plain),
            Self::Tls(stream) => Ok(Self::Tls(stream.clone())),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .read(buffer),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .write(buffer),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .flush(),
        }
    }
}

/// Connects to the first address of `host` that accepts a TCP connection.
pub fn connect_tcp(host: &str, port: u16) -> Result<TcpStream> {
    let addresses = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("Could not resolve {host}"))?;
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    anyhow::bail!(
        "Could not connect to {host}:{port}: {}",
        last_error.map(|e| e.to_string()).unwrap_or_default()
    )
}

pub fn connect(endpoint: &Endpoint<'_>, username: &str, password: &str) -> Result<Client> {
    connect_controlled(endpoint, username, password).map(|(client, _)| client)
}

/// This socket handle can interrupt a read even while TLS holds its stream lock.
pub struct Abort(TcpStream);
impl Abort {
    pub(crate) fn peer(&self) -> io::Result<SocketAddr> {
        self.0.peer_addr()
    }
    pub fn shutdown(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

impl super::Cancellation for Abort {
    fn cancel(&self) -> Result<()> {
        self.shutdown();
        Ok(())
    }
    fn abort_transport(&self) {
        self.shutdown();
    }
}

pub fn connect_controlled(
    endpoint: &Endpoint<'_>,
    username: &str,
    password: &str,
) -> Result<(Client, Arc<Abort>)> {
    connect_registered(endpoint, username, password, |_| Ok(()))
}

pub(crate) fn connect_registered(
    endpoint: &Endpoint<'_>,
    username: &str,
    password: &str,
    register: impl FnOnce(Arc<Abort>) -> Result<()>,
) -> Result<(Client, Arc<Abort>)> {
    let stream = connect_tcp(endpoint.host, endpoint.port)?;
    connect_socket(
        endpoint,
        username,
        password,
        stream,
        WRITE_TIMEOUT,
        MAX_FRAME,
        register,
    )
}

pub(crate) fn connect_deadline(
    endpoint: &Endpoint<'_>,
    username: &str,
    password: &str,
    peer: SocketAddr,
    deadline: Instant,
    register: impl FnOnce(Arc<Abort>) -> Result<()>,
) -> Result<Client> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    ensure!(!remaining.is_zero(), "Cancellation cleanup deadline passed");
    // Reuse the established peer address; cancellation does not wait for DNS.
    let stream = TcpStream::connect_timeout(&peer, remaining.min(CONNECT_TIMEOUT))?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    ensure!(!remaining.is_zero(), "Cancellation cleanup deadline passed");
    let endpoint = Endpoint {
        host: endpoint.host,
        port: endpoint.port,
        tls: endpoint.tls,
        read_timeout: remaining,
    };
    connect_socket(
        &endpoint,
        username,
        password,
        stream,
        remaining.min(WRITE_TIMEOUT),
        64 * 1024,
        register,
    )
    .map(|(client, _)| client)
}

fn connect_socket(
    endpoint: &Endpoint<'_>,
    username: &str,
    password: &str,
    stream: TcpStream,
    write_timeout: Duration,
    response_limit: usize,
    register: impl FnOnce(Arc<Abort>) -> Result<()>,
) -> Result<(Client, Arc<Abort>)> {
    let Endpoint {
        host,
        port: _,
        tls,
        read_timeout,
    } = *endpoint;
    let abort = Arc::new(Abort(stream.try_clone()?));
    register(abort.clone())?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(read_timeout))?;
    stream.set_write_timeout(Some(write_timeout))?;
    let mut stream = match tls {
        Some(trust) => Stream::Tls(Arc::new(Mutex::new(trust.connect(host, stream)?))),
        None => Stream::Plain(stream),
    };
    negotiate(&mut stream, username, password, read_timeout)?;
    let reader = FrameReader::new(stream.try_clone()?).with_timeout(read_timeout);
    let writer = FrameWriter::new(stream);
    Ok((
        TCLIServiceSyncClient::new(
            ResponseProtocol::new(reader, response_limit)?,
            TBinaryOutputProtocol::new(writer, true),
        ),
        abort,
    ))
}

/// Kyuubi did not accept the SASL PLAIN credentials.
#[derive(Debug)]
pub struct Rejected(pub u8);

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Kyuubi rejected SASL PLAIN authentication (status {})",
            self.0
        )
    }
}

impl std::error::Error for Rejected {}

/// Authenticate with SASL PLAIN. `read_timeout` is the read timeout of
/// `stream`, for the error when it elapses.
pub fn negotiate<S: Read + Write>(
    stream: &mut S,
    username: &str,
    password: &str,
    read_timeout: Duration,
) -> Result<()> {
    ensure!(
        !username.contains('\0') && !password.contains('\0'),
        "Credentials cannot contain null characters"
    );
    send_handshake(stream, 1, b"PLAIN")?;
    let mut response = Zeroizing::new(Vec::with_capacity(username.len() + password.len() + 2));
    response.push(0);
    response.extend_from_slice(username.as_bytes());
    response.push(0);
    response.extend_from_slice(password.as_bytes());
    send_handshake(stream, 2, &response)?;
    let mut header = [0; 5];
    stream
        .read_exact(&mut header)
        .map_err(|error| read_error(error, read_timeout))
        .context("Kyuubi did not complete SASL authentication")?;
    let length = u32::from_be_bytes(header[1..].try_into()?) as usize;
    ensure!(length <= 65536, "SASL response exceeds 64 KiB");
    let mut payload = vec![0; length];
    stream
        .read_exact(&mut payload)
        .map_err(|error| read_error(error, read_timeout))?;
    // Do not include the remote response in errors: authentication messages may contain secrets.
    if header[0] != 5 {
        return Err(Rejected(header[0]).into());
    }
    Ok(())
}

fn send_handshake(stream: &mut impl Write, status: u8, payload: &[u8]) -> io::Result<()> {
    (|| {
        stream.write_all(&[status])?;
        stream.write_all(&(payload.len() as u32).to_be_bytes())?;
        stream.write_all(payload)?;
        stream.flush()
    })()
    .map_err(write_error)
}

/// A socket timeout reports `WouldBlock` on macOS, which reads "Resource
/// temporarily unavailable (os error 35)". Name the limit instead.
fn timed_out(error: io::Error, message: impl FnOnce() -> String) -> io::Error {
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            io::Error::new(io::ErrorKind::TimedOut, message())
        }
        _ => error,
    }
}

fn read_error(error: io::Error, timeout: Duration) -> io::Error {
    timed_out(error, || {
        format!("Kyuubi did not answer within {} seconds", timeout.as_secs())
    })
}

fn write_error(error: io::Error) -> io::Error {
    timed_out(error, || {
        format!(
            "Kyuubi did not accept the request within {} seconds",
            WRITE_TIMEOUT.as_secs()
        )
    })
}

pub struct FrameReader<R> {
    inner: BufReader<R>,
    frame_remaining: usize,
    response_remaining: usize,
    /// The read timeout of the socket, for the error when it elapses.
    timeout: Duration,
}
impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner: BufReader::new(inner),
            frame_remaining: 0,
            response_remaining: usize::MAX,
            timeout: DEFAULT_READ_TIMEOUT,
        }
    }

    /// Set the read timeout of the socket, which the error names when it
    /// elapses.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub(super) fn begin_response(&mut self, limit: usize) {
        self.response_remaining = limit;
    }

    pub(super) fn response_remaining(&self) -> usize {
        self.response_remaining
    }
}
impl<R: Read> Read for FrameReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.response_remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Server response exceeds byte limit",
            ));
        }
        if self.frame_remaining == 0 {
            let mut header = [0; 4];
            let timeout = self.timeout;
            self.inner
                .read_exact(&mut header)
                .map_err(|error| read_error(error, timeout))?;
            let length = u32::from_be_bytes(header) as usize;
            if length == 0 || length > MAX_FRAME {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid SASL frame length",
                ));
            }
            self.frame_remaining = length;
        }
        // Stream frame payloads into the caller's buffer, without a second frame-sized allocation.
        let count = buffer
            .len()
            .min(self.frame_remaining)
            .min(self.response_remaining);
        let timeout = self.timeout;
        let read = self
            .inner
            .read(&mut buffer[..count])
            .map_err(|error| read_error(error, timeout))?;
        self.frame_remaining -= read;
        self.response_remaining -= read;
        Ok(read)
    }
}

pub struct FrameWriter<W> {
    inner: W,
    frame: Vec<u8>,
}
impl<W> FrameWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            frame: vec![],
        }
    }
}
impl<W: Write> Write for FrameWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.frame.len().saturating_add(buffer.len()) > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Request exceeds 64 MiB",
            ));
        }
        self.frame.extend_from_slice(buffer);
        Ok(buffer.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if !self.frame.is_empty() {
            self.inner
                .write_all(&(self.frame.len() as u32).to_be_bytes())
                .map_err(write_error)?;
            self.inner.write_all(&self.frame).map_err(write_error)?;
            self.frame.clear();
        }
        self.inner.flush().map_err(write_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn frame_boundaries_and_oversized_frames() {
        let mut writer = FrameWriter::new(vec![]);
        writer.write_all(b"hello").unwrap();
        writer.flush().unwrap();
        writer.write_all(b"world").unwrap();
        writer.flush().unwrap();
        let mut reader = FrameReader::new(Cursor::new(writer.inner));
        let mut value = [0; 10];
        reader.read_exact(&mut value).unwrap();
        assert_eq!(&value, b"helloworld");
        let mut bad = FrameReader::new(Cursor::new(u32::MAX.to_be_bytes()));
        assert!(bad.read(&mut value).is_err());
    }

    /// A reader whose socket timeout elapsed. macOS reports it as
    /// `WouldBlock` (os error 35).
    struct Stalled;
    impl Read for Stalled {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::WouldBlock))
        }
    }

    #[test]
    fn a_socket_timeout_names_the_limit() {
        let error = FrameReader::new(Stalled)
            .with_timeout(Duration::from_secs(45))
            .read(&mut [0; 4])
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "Kyuubi did not answer within 45 seconds");
        // Other errors keep their text.
        let refused = read_error(
            io::Error::from(io::ErrorKind::ConnectionReset),
            DEFAULT_READ_TIMEOUT,
        );
        assert_eq!(refused.kind(), io::ErrorKind::ConnectionReset);
    }
}
