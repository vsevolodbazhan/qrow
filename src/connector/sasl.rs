//! HiveServer2 SASL PLAIN negotiation and length-prefixed, unencrypted frames.
//! LDAP verification happens on Kyuubi, as with PyHive's LDAP transport.
use super::t_c_l_i_service::TCLIServiceSyncClient;
use anyhow::{Context, Result, ensure};
use std::{
    io::{self, Cursor, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};
use thrift::protocol::{TBinaryInputProtocol, TBinaryOutputProtocol};
use zeroize::Zeroizing;

pub const MAX_FRAME: usize = 64 * 1024 * 1024;
pub type Client = TCLIServiceSyncClient<
    TBinaryInputProtocol<FrameReader<TcpStream>>,
    TBinaryOutputProtocol<FrameWriter<TcpStream>>,
>;

pub fn connect(host: &str, port: u16, username: &str, password: &str) -> Result<Client> {
    let addresses = (host, port)
        .to_socket_addrs()
        .context("Could not resolve the Kyuubi host")?;
    let mut last_error = None;
    let mut connection = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, Duration::from_secs(10)) {
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
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    negotiate(&mut stream, username, password)?;
    let reader = FrameReader::new(stream.try_clone()?);
    let writer = FrameWriter::new(stream);
    let config = thrift::TConfiguration::builder()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME))
        .max_string_size(Some(MAX_FRAME))
        .max_container_size(Some(100_000))
        .build()?;
    Ok(TCLIServiceSyncClient::new(
        TBinaryInputProtocol::with_config(reader, true, config),
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
        .context("Kyuubi did not complete SASL authentication")?;
    let length = u32::from_be_bytes(header[1..].try_into()?) as usize;
    ensure!(length <= 65536, "SASL response exceeds 64 KiB");
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload)?;
    // Do not include the remote response in errors: authentication messages may contain secrets.
    ensure!(
        header[0] == 5,
        "Kyuubi rejected SASL PLAIN authentication (status {}). Check the username, password, and server authentication mode.",
        header[0]
    );
    Ok(())
}

fn send_handshake(stream: &mut impl Write, status: u8, payload: &[u8]) -> io::Result<()> {
    stream.write_all(&[status])?;
    stream.write_all(&(payload.len() as u32).to_be_bytes())?;
    stream.write_all(payload)?;
    stream.flush()
}

pub struct FrameReader<R> {
    inner: R,
    frame: Cursor<Vec<u8>>,
}
impl<R> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            frame: Cursor::new(vec![]),
        }
    }
}
impl<R: Read> Read for FrameReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.frame.position() as usize >= self.frame.get_ref().len() {
            let mut header = [0; 4];
            self.inner.read_exact(&mut header)?;
            let length = u32::from_be_bytes(header) as usize;
            if length == 0 || length > MAX_FRAME {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid SASL frame length",
                ));
            }
            let mut bytes = vec![0; length];
            self.inner.read_exact(&mut bytes)?;
            self.frame = Cursor::new(bytes);
        }
        self.frame.read(buffer)
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
                .write_all(&(self.frame.len() as u32).to_be_bytes())?;
            self.inner.write_all(&self.frame)?;
            self.frame.clear();
        }
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
