//! HiveServer2 SASL PLAIN negotiation and length-prefixed, unencrypted frames.
//! LDAP verification happens on Kyuubi, as with PyHive's LDAP transport.
use super::{protocol::ResponseProtocol, t_c_l_i_service::TCLIServiceSyncClient};
use anyhow::{Context, Result, ensure};
use std::{
    io::{self, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};
use thrift::protocol::TBinaryOutputProtocol;
use zeroize::Zeroizing;

pub const MAX_FRAME: usize = 64 * 1024 * 1024;
/// The limit for one TCP connection attempt to one resolved address.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The limit for one socket read. The connector polls the status of a running
/// query, so this does not limit the query duration.
const READ_TIMEOUT: Duration = Duration::from_secs(120);
/// The limit for one socket write.
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);
pub type Client = TCLIServiceSyncClient<
    ResponseProtocol<TcpStream>,
    TBinaryOutputProtocol<FrameWriter<TcpStream>>,
>;

pub fn connect(host: &str, port: u16, username: &str, password: &str) -> Result<Client> {
    let addresses = (host, port)
        .to_socket_addrs()
        .context("Could not resolve the Kyuubi host")?;
    let mut last_error = None;
    let mut connection = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => {
                connection = Some(stream);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let mut stream = connection.with_context(|| {
        format!(
            "Could not connect to {host}:{port}: {}",
            last_error.map(|e| e.to_string()).unwrap_or_default()
        )
    })?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    negotiate(&mut stream, username, password)?;
    let reader = FrameReader::new(stream.try_clone()?);
    let writer = FrameWriter::new(stream);
    Ok(TCLIServiceSyncClient::new(
        ResponseProtocol::new(reader, MAX_FRAME)?,
        TBinaryOutputProtocol::new(writer, true),
    ))
}

pub fn negotiate<S: Read + Write>(stream: &mut S, username: &str, password: &str) -> Result<()> {
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
        .map_err(read_error)
        .context("Kyuubi did not complete SASL authentication")?;
    let length = u32::from_be_bytes(header[1..].try_into()?) as usize;
    ensure!(length <= 65536, "SASL response exceeds 64 KiB");
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).map_err(read_error)?;
    // Do not include the remote response in errors: authentication messages may contain secrets.
    ensure!(
        header[0] == 5,
        "Kyuubi rejected SASL PLAIN authentication (status {}). Check the username, password, and server authentication mode.",
        header[0]
    );
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

fn read_error(error: io::Error) -> io::Error {
    timed_out(error, || {
        format!(
            "Kyuubi did not answer within {} seconds",
            READ_TIMEOUT.as_secs()
        )
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
}
impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner: BufReader::new(inner),
            frame_remaining: 0,
            response_remaining: usize::MAX,
        }
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
            self.inner.read_exact(&mut header).map_err(read_error)?;
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
        let read = self.inner.read(&mut buffer[..count]).map_err(read_error)?;
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
        let error = FrameReader::new(Stalled).read(&mut [0; 4]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            error.to_string(),
            "Kyuubi did not answer within 120 seconds"
        );
        // Other errors keep their text.
        let refused = read_error(io::Error::from(io::ErrorKind::ConnectionReset));
        assert_eq!(refused.kind(), io::ErrorKind::ConnectionReset);
    }
}
