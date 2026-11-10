//! Socket interruption and a plaintext message ceiling before driver decoding.
use super::cancellation::Endpoint;
use crate::connector::Cancellation;
use anyhow::Result;
use futures_util::task::AtomicWaker;
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
    wakers: Arc<Wakers>,
    pub(super) closed: Arc<AtomicBool>,
    pub(super) limit: Arc<AtomicUsize>,
    pub(super) boundary: Arc<AtomicBool>,
}

impl Abort {
    #[cfg(test)]
    pub(super) fn synthetic() -> Arc<Self> {
        Arc::new(Self {
            socket: Socket::Synthetic,
            wakers: Arc::default(),
            closed: Arc::new(AtomicBool::new(false)),
            limit: Arc::new(AtomicUsize::new(PREVIEW_FRAME_BYTES)),
            boundary: Arc::new(AtomicBool::new(true)),
        })
    }
    pub(super) async fn tcp(
        address: std::net::SocketAddr,
        limit: usize,
    ) -> Result<(Interruptible<tokio::net::TcpStream>, Arc<Self>)> {
        let socket = tokio::net::TcpStream::connect(address).await?.into_std()?;
        socket.set_nodelay(true)?;
        let abort = Arc::new(Self {
            socket: Socket::Tcp(socket.try_clone()?),
            wakers: Arc::default(),
            closed: Arc::new(AtomicBool::new(false)),
            limit: Arc::new(AtomicUsize::new(limit)),
            boundary: Arc::new(AtomicBool::new(true)),
        });
        let socket = abort.interruptible(tokio::net::TcpStream::from_std(socket)?);
        Ok((socket, abort))
    }

    #[cfg(unix)]
    pub(super) async fn unix(
        path: &std::path::Path,
        limit: usize,
    ) -> Result<(Interruptible<tokio::net::UnixStream>, Arc<Self>)> {
        let socket = tokio::net::UnixStream::connect(path).await?.into_std()?;
        let abort = Arc::new(Self {
            socket: Socket::Unix(socket.try_clone()?),
            wakers: Arc::default(),
            closed: Arc::new(AtomicBool::new(false)),
            limit: Arc::new(AtomicUsize::new(limit)),
            boundary: Arc::new(AtomicBool::new(true)),
        });
        let socket = abort.interruptible(tokio::net::UnixStream::from_std(socket)?);
        Ok((socket, abort))
    }

    fn interruptible<S>(&self, stream: S) -> Interruptible<S> {
        Interruptible {
            stream,
            closed: self.closed.clone(),
            wakers: self.wakers.clone(),
        }
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
        // Linux wakes a pending read after a shutdown, but Windows does not.
        // Wake the readers and writers so that they see the abort.
        self.wakers.read.wake();
        self.wakers.write.wake();
    }
}

#[derive(Default)]
struct Wakers {
    read: AtomicWaker,
    write: AtomicWaker,
}

/// A socket that fails its pending and later reads and writes after
/// `Abort::abort_transport`, also on systems where a shutdown does not wake
/// a pending read.
pub(super) struct Interruptible<S> {
    stream: S,
    closed: Arc<AtomicBool>,
    wakers: Arc<Wakers>,
}

fn aborted() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "Postgres transport was closed",
    )
}

impl<S: AsyncRead + Unpin> AsyncRead for Interruptible<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // Register before the check, so that an abort between them wakes us.
        this.wakers.read.register(cx.waker());
        if this.closed.load(Ordering::SeqCst) {
            return Poll::Ready(Err(aborted()));
        }
        Pin::new(&mut this.stream).poll_read(cx, output)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Interruptible<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.wakers.write.register(cx.waker());
        if this.closed.load(Ordering::SeqCst) {
            return Poll::Ready(Err(aborted()));
        }
        Pin::new(&mut this.stream).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.wakers.write.register(cx.waker());
        if this.closed.load(Ordering::SeqCst) {
            return Poll::Ready(Err(aborted()));
        }
        Pin::new(&mut this.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
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
    boundary: Arc<AtomicBool>,
}

impl<S> Wire<S> {
    #[cfg(test)]
    pub(super) fn new(stream: S, limit: Arc<AtomicUsize>) -> Self {
        Self::controlled(stream, limit, Arc::new(AtomicBool::new(true)))
    }

    pub(super) fn controlled(
        stream: S,
        limit: Arc<AtomicUsize>,
        boundary: Arc<AtomicBool>,
    ) -> Self {
        Self {
            stream,
            limit,
            header: [0; 5],
            received: 0,
            sent: 0,
            body: 0,
            failed: false,
            boundary,
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
            this.boundary.store(false, Ordering::SeqCst);
            if this.received == 5 {
                let length = u32::from_be_bytes(this.header[1..].try_into().unwrap()) as usize;
                let mut limit = this.limit.load(Ordering::SeqCst);
                if limit == EXPORT_FRAME_BYTES
                    && matches!(this.header[0], b'E' | b'N' | b'R' | b'S' | b'A')
                {
                    limit = 1024 * 1024;
                }
                if length < 4 || length.saturating_add(1) > limit {
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
            if this.sent == 5 && this.body == 0 {
                this.boundary.store(true, Ordering::SeqCst);
            }
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
        if this.body == 0 {
            this.boundary.store(true, Ordering::SeqCst);
        }
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
    pub(super) boundary: Arc<AtomicBool>,
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
        Box::pin(async move {
            Ok(Wire::controlled(
                self.tls.connect(stream).await?,
                self.limit,
                self.boundary,
            ))
        })
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
    fn an_abort_wakes_a_pending_read_and_write_without_a_socket_shutdown() {
        // A synthetic abort has no socket to shut down, like Windows, where a
        // shutdown does not wake a pending read.
        runtime().block_on(async {
            let abort = Abort::synthetic();
            let (client, _server) = tokio::io::duplex(1);
            let (reader, writer) = tokio::io::split(abort.interruptible(client));
            let read = tokio::spawn(async move {
                let mut reader = reader;
                reader.read(&mut [0; 1]).await
            });
            let write = tokio::spawn(async move {
                let mut writer = writer;
                // The duplex holds one byte, so the second write waits.
                writer.write_all(b"ab").await
            });
            tokio::task::yield_now().await;
            assert!(!read.is_finished() && !write.is_finished());
            abort.abort_transport();
            let timeout = std::time::Duration::from_secs(5);
            let read = tokio::time::timeout(timeout, read).await.unwrap().unwrap();
            let write = tokio::time::timeout(timeout, write).await.unwrap().unwrap();
            assert_eq!(read.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
            assert_eq!(write.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        });
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
    fn export_diagnostics_and_authentication_have_a_smaller_predecode_ceiling() {
        for tag in *b"ENRSA" {
            runtime().block_on(async {
                let (client, mut server) = tokio::io::duplex(16);
                server.write_u8(tag).await.unwrap();
                server.write_u32(2 * 1024 * 1024).await.unwrap();
                let mut wire = Wire::new(client, Arc::new(AtomicUsize::new(EXPORT_FRAME_BYTES)));
                let mut out = [0; 16];
                assert_eq!(
                    wire.read(&mut out).await.unwrap_err().kind(),
                    io::ErrorKind::InvalidData
                );
                assert_eq!(out, [0; 16]);
                assert!(wire.read(&mut out).await.is_err());
            });
        }
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
