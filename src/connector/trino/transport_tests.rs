use super::{
    protocol::Http,
    transport::{Runtime, Transport},
};
use crate::{
    connector::{Cancellation, Secret},
    model::Profile,
    tls::Trust,
};
use anyhow::Result;
use reqwest::Method;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

fn accept(listener: &TcpListener) -> Result<TcpStream> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(4)))?;
                stream.set_write_timeout(Some(Duration::from_secs(4)))?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                anyhow::ensure!(Instant::now() < deadline, "test peer accept timed out");
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn headers(stream: &mut TcpStream) -> Result<String> {
    let mut bytes = Vec::new();
    let mut byte = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        anyhow::ensure!(bytes.len() < 64 * 1024, "test request headers too large");
        stream.read_exact(&mut byte)?;
        bytes.push(byte[0]);
    }
    Ok(String::from_utf8(bytes)?)
}

fn eof(stream: &mut TcpStream) -> Result<()> {
    let mut bytes = [0; 8192];
    loop {
        match stream.read(&mut bytes) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn profile(peer: SocketAddr, tls: bool) -> Profile {
    Profile {
        host: peer.ip().to_string(),
        port: peer.port(),
        tls,
        username: "synthetic".into(),
        ..Profile::default()
    }
}

fn operation(http: Arc<Http>) -> Result<super::Operation> {
    Ok(super::Operation {
        cancel: Arc::new(super::Cancel {
            requests: http.begin_requests(),
            http,
            next: std::sync::Mutex::new(super::Cursor::default()),
            requested: std::sync::atomic::AtomicBool::new(false),
            active: std::sync::Mutex::new(true),
            cleanup: std::sync::Mutex::default(),
            changed: std::sync::Condvar::new(),
        }),
        columns: Vec::new(),
        writer: Some(std::io::BufWriter::new(tempfile::tempfile()?)),
        reader: None,
        rows: 0,
        bytes: 0,
        remaining: 0,
        exhausted: false,
        limited: false,
        finished: false,
        cancelled: false,
    })
}

#[test]
fn sealing_between_poll_check_and_get_admission_is_clean_cancellation() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let http = Arc::new(Http::new(
        &profile(listener.local_addr()?, false),
        Secret::password(""),
        &Trust::default(),
    )?);
    let operation = operation(http.clone())?;
    let cancel = operation.cancel.clone();
    let (checked_tx, checked_rx) = mpsc::channel();
    let (continue_tx, continue_rx) = mpsc::channel();
    let reader = thread::spawn(move || -> Result<()> {
        // This is poll's fast path immediately before its read_page call.
        assert!(!operation.cancel.requested.load(Ordering::SeqCst));
        checked_tx.send(())?;
        continue_rx.recv_timeout(Duration::from_secs(4))?;
        assert!(
            operation
                .read_page(
                    &operation.cancel.http.statement,
                    &super::protocol::SessionHeaders::default()
                )?
                .is_none()
        );
        Ok(())
    });
    checked_rx.recv_timeout(Duration::from_secs(4))?;
    cancel.requested.store(true, Ordering::SeqCst);
    cancel.requests.seal();
    continue_tx.send(())?;
    reader.join().unwrap()?;
    assert!(!http.transport.is_closed());
    listener.set_nonblocking(true)?;
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));
    Ok(())
}

#[test]
fn terminal_error_and_detached_cancellation_preserve_successor_transport() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let http = Arc::new(Http::new(
        &profile(listener.local_addr()?, false),
        Secret::password(""),
        &Trust::default(),
    )?);
    let mut operation = operation(http.clone())?;
    let error = operation
        .page(serde_json::from_str(
            r#"{"error":{"errorName":"SYNTHETIC","message":"terminal failure"}}"#,
        )?)
        .unwrap_err();
    assert!(error.is::<crate::connector::QueryError>());
    let old = operation.cancel.clone();
    let mut current = Some(operation);
    super::TrinoSession::stop(&mut current)?;
    assert!(current.is_none());
    let successor = http.begin_requests();
    let _read = successor.activity()?;
    old.abort_transport();
    old.cancel()?;
    assert!(!http.transport.is_closed());
    assert!(!successor.is_closed());
    Ok(())
}

#[test]
fn raw_socket_abort_interrupts_tls_before_authentication() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let http = Arc::new(Http::new(
        &profile(listener.local_addr()?, true),
        Secret::password("synthetic"),
        &Trust::default(),
    )?);
    let (ready_tx, ready_rx) = mpsc::channel();
    let peer = thread::spawn(move || -> Result<()> {
        let mut socket = accept(&listener)?;
        let mut hello = [0; 4096];
        anyhow::ensure!(socket.read(&mut hello)? > 0, "TLS hello did not arrive");
        ready_tx.send(())?;
        eof(&mut socket)
    });
    let owner = http.clone();
    let request = thread::spawn(move || {
        owner.page(
            Method::POST,
            &owner.statement,
            Some("SELECT 1"),
            &super::protocol::SessionHeaders::default(),
        )
    });
    ready_rx.recv_timeout(Duration::from_secs(4))?;
    let start = Instant::now();
    http.transport.close_all();
    assert!(request.join().unwrap().is_err());
    peer.join().unwrap()?;
    assert!(start.elapsed() < Duration::from_secs(2));
    Ok(())
}

#[test]
fn raw_abort_closes_post_headers_and_body_get_head_delete_stalls() -> Result<()> {
    for (method, body_started) in [
        (Method::POST, false),
        (Method::POST, true),
        (Method::GET, true),
        (Method::HEAD, false),
        (Method::DELETE, false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let transport = Transport::new();
        let sockets = transport.clone();
        let client = reqwest::Client::builder()
            .no_proxy()
            .tcp_connection_control(move |socket| sockets.register(socket))
            .build()?;
        let url = format!("http://{}/v1/statement", listener.local_addr()?);
        let expected = method.to_string();
        let (ready_tx, ready_rx) = mpsc::channel();
        let peer = thread::spawn(move || -> Result<()> {
            let mut socket = accept(&listener)?;
            assert!(headers(&mut socket)?.starts_with(&format!("{expected} ")));
            if body_started {
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 300\r\n\r\n{\"data\":[")?;
                socket.flush()?;
            }
            ready_tx.send(())?;
            eof(&mut socket)
        });
        let owner = transport.clone();
        let request = thread::spawn(move || -> Result<()> {
            let runtime = Runtime::new()?;
            runtime.block_on(owner.run(async {
                let mut response = client.request(method, url).send().await?;
                while response.chunk().await?.is_some() {}
                Ok(())
            }))
        });
        ready_rx.recv_timeout(Duration::from_secs(4))?;
        let start = Instant::now();
        transport.close_all();
        assert!(request.join().unwrap().is_err());
        peer.join().unwrap()?;
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    Ok(())
}

#[test]
fn abort_covers_a_connection_already_in_the_pool() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let transport = Transport::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let sockets = transport.clone();
    let client = reqwest::Client::builder()
        .no_proxy()
        .tcp_connection_control(move |socket| {
            observed.fetch_add(1, Ordering::SeqCst);
            sockets.register(socket)
        })
        .build()?;
    let url = format!("http://{}/v1/statement", listener.local_addr()?);
    let peer = thread::spawn(move || -> Result<()> {
        let mut socket = accept(&listener)?;
        for _ in 0..2 {
            headers(&mut socket)?;
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")?;
        }
        eof(&mut socket)
    });
    let runtime = Runtime::new()?;
    for _ in 0..2 {
        runtime.block_on(transport.run(async {
            assert_eq!(client.get(&url).send().await?.text().await?, "{}");
            Ok(())
        }))?;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    transport.close_all();
    peer.join().unwrap()?;
    Ok(())
}

#[test]
fn closed_registry_rejects_a_late_connection_before_http_authentication() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let transport = Transport::new();
    transport.close_all();
    let sockets = transport;
    let client = reqwest::Client::builder()
        .no_proxy()
        .tcp_connection_control(move |socket| sockets.register(socket))
        .build()?;
    let url = format!("http://{}/v1/statement", listener.local_addr()?);
    let peer = thread::spawn(move || -> Result<()> {
        let mut socket = accept(&listener)?;
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte)?, 0);
        Ok(())
    });
    let runtime = Runtime::new()?;
    assert!(
        runtime
            .block_on(
                client
                    .post(url)
                    .basic_auth("synthetic", Some("must-not-arrive"))
                    .send()
            )
            .is_err()
    );
    peer.join().unwrap()?;
    Ok(())
}

#[test]
fn request_abort_is_bounded_before_a_socket_exists() -> Result<()> {
    let transport = Transport::new();
    let owner = transport.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let request = thread::spawn(move || -> Result<()> {
        Runtime::new()?.block_on(owner.run(async {
            started_tx.send(())?;
            std::future::pending::<()>().await;
            Ok(())
        }))
    });
    started_rx.recv_timeout(Duration::from_secs(4))?;
    let start = Instant::now();
    transport.abort_transport();
    assert!(request.join().unwrap().is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    Ok(())
}

#[test]
fn closing_a_heartbeat_scope_preserves_the_primary_socket() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let peer = listener.local_addr()?;
    let primary = Transport::new();
    let heartbeat = primary.child();
    let runtime = Runtime::new()?;
    let (first, second) = runtime.block_on(async {
        Ok::<_, anyhow::Error>((
            tokio::net::TcpStream::connect(peer).await?,
            tokio::net::TcpStream::connect(peer).await?,
        ))
    })?;
    let _first_lease = primary.register(&first)?;
    let _second_lease = heartbeat.register(&second)?;
    drop((first, second));
    let mut first_peer = accept(&listener)?;
    let mut second_peer = accept(&listener)?;
    heartbeat.close();
    eof(&mut second_peer)?;
    assert!(!primary.is_closed());
    first_peer.set_read_timeout(Some(Duration::from_millis(30)))?;
    let error = first_peer.read(&mut [0]).unwrap_err();
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    first_peer.set_read_timeout(Some(Duration::from_secs(4)))?;
    primary.close_all();
    eof(&mut first_peer)?;
    Ok(())
}

#[test]
fn delete_deadline_closes_its_socket_after_a_stalled_response() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let peer = listener.local_addr()?;
    let http = Arc::new(Http::new(
        &profile(peer, false),
        Secret::password(""),
        &Trust::default(),
    )?);
    let server = thread::spawn(move || -> Result<()> {
        let mut probe = accept(&listener)?;
        assert!(headers(&mut probe)?.starts_with("GET "));
        probe.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\n{}")?;
        drop(probe);
        let mut cleanup = accept(&listener)?;
        assert!(headers(&mut cleanup)?.starts_with("DELETE "));
        eof(&mut cleanup)
    });
    http.page(
        Method::GET,
        &http.statement,
        None,
        &super::protocol::SessionHeaders::default(),
    )?;
    let start = Instant::now();
    assert!(
        http.delete(&http.statement, start + Duration::from_millis(200))
            .is_err()
    );
    server.join().unwrap()?;
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!http.transport.is_closed());
    Ok(())
}

#[test]
fn successful_delete_still_bounds_a_stalled_primary_body() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let http = Arc::new(Http::new(
        &profile(listener.local_addr()?, false),
        Secret::password(""),
        &Trust::default(),
    )?);
    let requests = http.begin_requests();
    let cancel = super::Cancel {
        http: http.clone(),
        next: std::sync::Mutex::new(super::Cursor {
            next: Some(http.statement.clone()),
            terminal: false,
        }),
        requested: std::sync::atomic::AtomicBool::new(false),
        active: std::sync::Mutex::new(true),
        requests,
        cleanup: std::sync::Mutex::default(),
        changed: std::sync::Condvar::new(),
    };
    let (ready_tx, ready_rx) = mpsc::channel();
    let server = thread::spawn(move || -> Result<()> {
        let mut primary = accept(&listener)?;
        assert!(headers(&mut primary)?.starts_with("GET "));
        primary.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 300\r\n\r\n{\"data\":[")?;
        primary.flush()?;
        ready_tx.send(())?;
        let mut cleanup = accept(&listener)?;
        assert!(headers(&mut cleanup)?.starts_with("DELETE "));
        cleanup.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")?;
        drop(cleanup);
        eof(&mut primary)
    });
    let owner = http.clone();
    let primary = thread::spawn(move || {
        owner.page(
            Method::GET,
            &owner.statement,
            None,
            &super::protocol::SessionHeaders::default(),
        )
    });
    ready_rx.recv_timeout(Duration::from_secs(4))?;
    // page() stores the established peer after headers, before reading the body.
    let peer_deadline = Instant::now() + Duration::from_secs(2);
    while !http.has_peer() {
        anyhow::ensure!(
            Instant::now() < peer_deadline,
            "primary headers were not read"
        );
        thread::sleep(Duration::from_millis(2));
    }
    let start = Instant::now();
    assert!(
        cancel
            .cancel_with_deadline(start + Duration::from_millis(200))
            .is_err()
    );
    assert!(primary.join().unwrap().is_err());
    server.join().unwrap()?;
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(http.transport.is_closed());
    assert!(cancel.cancel().is_err()); // Cached failure; no second DELETE.
    Ok(())
}

#[test]
fn sealed_activity_waits_for_the_current_read_and_rejects_a_late_read() -> Result<()> {
    let transport = Transport::new();
    let first = transport.child();
    let activity = first.activity()?;
    first.seal();
    assert!(first.activity().is_err());
    assert!(first.wait_idle(Instant::now()).is_err());
    drop(activity);
    first.wait_idle(Instant::now())?;
    let successor = transport.child();
    let _next = successor.activity()?;
    first.close();
    assert!(!successor.is_closed());
    assert!(!transport.is_closed());
    Ok(())
}
