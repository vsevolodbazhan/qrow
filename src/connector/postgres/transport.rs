//! Socket interruption and a plaintext message ceiling before driver decoding.
use super::cancellation::Endpoint;
use crate::connector::Cancellation;
use anyhow::Result;
use std::{
    future::Future,
    io,
    net::Shutdown,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll, ready},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_postgres::tls::{ChannelBinding, TlsConnect, TlsStream};

pub(super) const PREVIEW_FRAME_BYTES: usize = 128 * 1024 * 1024;
pub(super) const EXPORT_FRAME_BYTES: usize = 16 * 1024 * 1024;

enum Socket {
    Tcp(std::net::TcpStream),
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
    #[cfg(test)]
    Synthetic,
}

pub(super) struct Abort {
    socket: Socket,
    pub(super) closed: Arc<AtomicBool>,
    pub(super) limit: Arc<AtomicUsize>,
}

impl Abort {
    #[cfg(test)]
    pub(super) fn synthetic() -> Arc<Self> {
        Arc::new(Self {
            socket: Socket::Synthetic,
            closed: Arc::new(AtomicBool::new(false)),
            limit: Arc::new(AtomicUsize::new(PREVIEW_FRAME_BYTES)),
        })
    }
    pub(super) async fn tcp(
        address: std::net::SocketAddr,
        limit: usize,
    ) -> Result<(tokio::net::TcpStream, Arc<Self>)> {
        let socket = tokio::net::TcpStream::connect(address).await?.into_std()?;
        socket.set_nodelay(true)?;
        let abort = Arc::new(Self {
            socket: Socket::Tcp(socket.try_clone()?),
            closed: Arc::new(AtomicBool::new(false)),
            limit: Arc::new(AtomicUsize::new(limit)),
        });
        Ok((tokio::net::TcpStream::from_std(socket)?, abort))
    }

    #[cfg(unix)]
    pub(super) async fn unix(
        path: &std::path::Path,
        limit: usize,
    ) -> Result<(tokio::net::UnixStream, Arc<Self>)> {
        let socket = tokio::net::UnixStream::connect(path).await?.into_std()?;
        let abort = Arc::new(Self {
            socket: Socket::Unix(socket.try_clone()?),
            closed: Arc::new(AtomicBool::new(false)),
            limit: Arc::new(AtomicUsize::new(limit)),
        });
        Ok((tokio::net::UnixStream::from_std(socket)?, abort))
    }

    pub(super) fn endpoint(&self) -> Result<Endpoint> {
        Ok(match &self.socket {
            Socket::Tcp(socket) => Endpoint::Tcp(socket.peer_addr()?),
            #[cfg(unix)]
            Socket::Unix(socket) => Endpoint::Unix(
                socket
                    .peer_addr()?
                    .as_pathname()
                    .ok_or_else(|| io::Error::other("Postgres Unix socket has no path"))?
                    .to_owned(),
            ),
            #[cfg(test)]
            Socket::Synthetic => Endpoint::Tcp("127.0.0.1:0".parse().unwrap()),
        })
    }
}

impl Cancellation for Abort {
    fn cancel(&self) -> Result<()> {
        self.abort_transport();
        Ok(())
    }
    fn abort_transport(&self) {
        self.closed.store(true, Ordering::SeqCst);
        match &self.socket {
            Socket::Tcp(socket) => {
                let _ = socket.shutdown(Shutdown::Both);
            }
            #[cfg(unix)]
            Socket::Unix(socket) => {
                let _ = socket.shutdown(Shutdown::Both);
            }
            #[cfg(test)]
            Socket::Synthetic => {}
        }
    }
}

pub(super) struct Wire<S> {
    stream: S,
    limit: Arc<AtomicUsize>,
    header: [u8; 5],
    received: usize,
    sent: usize,
    body: usize,
    failed: bool,
}

impl<S> Wire<S> {
    pub(super) fn new(stream: S, limit: Arc<AtomicUsize>) -> Self {
        Self {
            stream,
            limit,
            header: [0; 5],
            received: 0,
            sent: 0,
            body: 0,
            failed: false,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Wire<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.failed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Postgres transport previously rejected a message",
            )));
        }
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if this.sent == 5 && this.body == 0 {
            this.sent = 0;
            this.received = 0;
        }
        while this.received < 5 {
            let mut input = ReadBuf::new(&mut this.header[this.received..]);
            ready!(Pin::new(&mut this.stream).poll_read(cx, &mut input))?;
            let count = input.filled().len();
            if count == 0 {
                return Poll::Ready(if this.received == 0 {
                    Ok(())
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Truncated Postgres message header",
                    ))
                });
            }
            this.received += count;
            if this.received == 5 {
                let length = u32::from_be_bytes(this.header[1..].try_into().unwrap()) as usize;
                if length < 4 || length.saturating_add(1) > this.limit.load(Ordering::SeqCst) {
                    this.failed = true;
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Postgres message exceeds the transport limit",
                    )));
                }
                this.body = length - 4;
            }
        }
        if this.sent < 5 {
            let count = (5 - this.sent).min(output.remaining());
            output.put_slice(&this.header[this.sent..this.sent + count]);
            this.sent += count;
            return Poll::Ready(Ok(()));
        }
        let count = this.body.min(output.remaining()).min(8192);
        let mut input = ReadBuf::new(output.initialize_unfilled_to(count));
        ready!(Pin::new(&mut this.stream).poll_read(cx, &mut input))?;
        let count = input.filled().len();
        if count == 0 && this.body > 0 {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Truncated Postgres message body",
            )));
        }
        output.advance(count);
        this.body -= count;
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Wire<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl<S: TlsStream + Unpin> TlsStream for Wire<S> {
    fn channel_binding(&self) -> ChannelBinding {
        self.stream.channel_binding()
    }
}

pub(super) struct GuardedTls<T> {
    pub(super) tls: T,
    pub(super) limit: Arc<AtomicUsize>,
}

impl<S: Send + 'static, T: TlsConnect<S> + Send + 'static> TlsConnect<S> for GuardedTls<T>
where
    T::Future: Send + 'static,
    T::Stream: Send + 'static,
    T::Error: 'static,
{
    type Stream = Wire<T::Stream>;
    type Error = T::Error;
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<Self::Stream, Self::Error>> + Send>>;
    fn connect(self, stream: S) -> Self::Future {
        Box::pin(async move { Ok(Wire::new(self.tls.connect(stream).await?, self.limit)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn fragmented_frames_are_exact_and_reads_stop_at_a_frame_boundary() {
        runtime().block_on(async {
            let (client, mut server) = tokio::io::duplex(128);
            let packet = b"D\0\0\0\x07abcZ\0\0\0\x05I";
            let writer = tokio::spawn(async move {
                for byte in packet {
                    server.write_all(&[*byte]).await.unwrap();
                    tokio::task::yield_now().await;
                }
            });
            let mut wire = Wire::new(client, Arc::new(AtomicUsize::new(16)));
            let mut observed = Vec::new();
            wire.read_to_end(&mut observed).await.unwrap();
            writer.await.unwrap();
            assert_eq!(observed, packet);
        });
    }

    #[test]
    fn advertised_oversize_is_rejected_before_the_header_reaches_the_decoder() {
        runtime().block_on(async {
            let (client, mut server) = tokio::io::duplex(16);
            server.write_all(b"D\x7f\xff\xff\xff").await.unwrap();
            let mut wire = Wire::new(client, Arc::new(AtomicUsize::new(16)));
            let mut bytes = [0; 64];
            assert_eq!(
                wire.read(&mut bytes).await.unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(bytes, [0; 64]);
            assert!(wire.read(&mut bytes).await.is_err());
        });
    }

    #[test]
    fn body_reads_are_bounded_and_the_limit_can_change_between_operations() {
        runtime().block_on(async {
            let (client, mut server) = tokio::io::duplex(65536);
            let mut bytes = vec![b'D'];
            bytes.extend_from_slice(&20004u32.to_be_bytes());
            bytes.extend(vec![b'x'; 20000]);
            server.write_all(&bytes).await.unwrap();
            let limit = Arc::new(AtomicUsize::new(30000));
            let mut wire = Wire::new(client, limit.clone());
            let mut out = vec![0; 32768];
            assert_eq!(wire.read(&mut out).await.unwrap(), 5);
            let mut remaining = 20000;
            while remaining > 0 {
                let count = wire.read(&mut out).await.unwrap();
                assert!(count <= 8192);
                remaining -= count;
            }
            limit.store(16, Ordering::SeqCst);
            server.write_all(b"D\0\0\0\x20").await.unwrap();
            assert!(wire.read(&mut out).await.is_err());
        });
    }
}
