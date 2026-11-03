//! Wait for Postgres to process a cancellation before reusing the session.
use super::transport;
use anyhow::Result;
use std::sync::Arc;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_postgres::{CancelToken, NoTls, tls::MakeTlsConnect};
use tokio_postgres_rustls::MakeRustlsConnect;

#[derive(Clone)]
pub(super) enum Endpoint {
    Tcp(std::net::SocketAddr),
    #[cfg(unix)]
    Unix(std::path::PathBuf),
}

pub(super) async fn send(
    token: &CancelToken,
    host: &str,
    endpoint: &Endpoint,
    tls: Option<MakeRustlsConnect>,
    register: impl Fn(Arc<transport::Abort>),
) -> Result<()> {
    match endpoint {
        Endpoint::Tcp(address) => {
            let (socket, abort) =
                transport::Abort::tcp(*address, transport::PREVIEW_FRAME_BYTES).await?;
            register(abort);
            send_over(token, socket, host, tls).await
        }
        #[cfg(unix)]
        Endpoint::Unix(path) => {
            let (socket, abort) =
                transport::Abort::unix(path, transport::PREVIEW_FRAME_BYTES).await?;
            register(abort);
            send_over(token, socket, host, tls).await
        }
    }
}

async fn send_over<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    token: &CancelToken,
    stream: S,
    host: &str,
    tls: Option<MakeRustlsConnect>,
) -> Result<()> {
    let socket = WaitForClose::new(stream);
    match tls {
        Some(mut tls) => {
            let tls = <MakeRustlsConnect as MakeTlsConnect<WaitForClose<S>>>::make_tls_connect(
                &mut tls, host,
            )?;
            token.cancel_query_raw(socket, tls).await?;
        }
        None => token.cancel_query_raw(socket, NoTls).await?,
    }
    Ok(())
}

// tokio-postgres completes CancelRequest by shutting down the write side.
// Postgres closes this socket after processing the request. Wait for that
// closure, including below TLS, so a pending packet cannot hit the next query.
struct WaitForClose<S> {
    stream: S,
    shutdown: bool,
}

impl<S> WaitForClose<S> {
    fn new(stream: S) -> Self {
        Self {
            stream,
            shutdown: false,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for WaitForClose<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for WaitForClose<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.shutdown {
            ready!(Pin::new(&mut self.stream).poll_shutdown(cx))?;
            self.shutdown = true;
        }
        // A TLS peer can leave encrypted shutdown bytes before the socket EOF.
        // Bound work per poll and retain no response data.
        for _ in 0..16 {
            let mut bytes = [0; 1024];
            let mut buffer = ReadBuf::new(&mut bytes);
            ready!(Pin::new(&mut self.stream).poll_read(cx, &mut buffer))?;
            if buffer.filled().is_empty() {
                return Poll::Ready(Ok(()));
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::pin_mut;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn cancellation_shutdown_waits_for_server_closure() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (client, mut server) = tokio::io::duplex(1024);
            let mut client = WaitForClose::new(client);
            client.write_all(b"cancel").await.unwrap();
            let shutdown = client.shutdown();
            pin_mut!(shutdown);
            assert!(futures_util::poll!(&mut shutdown).is_pending());
            let mut packet = Vec::new();
            server.read_to_end(&mut packet).await.unwrap();
            assert_eq!(packet, b"cancel");
            server.write_all(b"shutdown bytes").await.unwrap();
            assert!(futures_util::poll!(&mut shutdown).is_pending());
            server.shutdown().await.unwrap();
            shutdown.await.unwrap();
        });
    }

    #[test]
    fn cancellation_shutdown_is_bounded_when_server_keeps_socket_open() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (client, _server) = tokio::io::duplex(1024);
            let mut client = WaitForClose::new(client);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(10), client.shutdown())
                    .await
                    .is_err()
            );
        });
    }
}
