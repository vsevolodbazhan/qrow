//! Synthetic peers hold setup at protocol boundaries without real credentials.
use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
};

fn profile(listener: &TcpListener, tls: bool) -> Profile {
    Profile {
        database_type: crate::model::DatabaseType::Postgres,
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        username: "synthetic-user".into(),
        database: "synthetic-db".into(),
        postgres_ssl_mode: Some(if tls {
            PostgresSslMode::Require
        } else {
            PostgresSslMode::Disable
        }),
        ..Profile::default()
    }
}

#[test]
fn controlled_setup_is_interruptible_during_ssl_tls_or_authentication() {
    for stage in 0..3 {
        let tls = stage != 0;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let profile = profile(&listener, tls);
        let (received, setup) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut length = [0; 4];
            socket.read_exact(&mut length).unwrap();
            let count = u32::from_be_bytes(length) as usize;
            assert!((8..4096).contains(&count));
            let mut startup = vec![0; count - 4];
            socket.read_exact(&mut startup).unwrap();
            if tls {
                assert_eq!(startup, [4, 210, 22, 47]);
                if stage == 2 {
                    socket.write_all(b"S").unwrap();
                    let mut header = [0; 5];
                    socket.read_exact(&mut header).unwrap();
                    assert_eq!(header[0], 22); // TLS handshake record.
                    let bytes = u16::from_be_bytes(header[3..].try_into().unwrap()) as usize;
                    assert!((1..=16384).contains(&bytes));
                    socket.read_exact(&mut vec![0; bytes]).unwrap();
                }
            }
            received.send(()).unwrap();
            assert_eq!(socket.read(&mut [0; 16]).unwrap(), 0);
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let (registered, target) = mpsc::channel();
        let control = ConnectionControl::new(
            cancelled,
            Arc::new(move |transport| {
                registered.send(transport).unwrap();
                Ok(())
            }),
        );
        let (completed, result) = mpsc::channel();
        let worker = thread::spawn(move || {
            completed
                .send(
                    PostgresConnector::default()
                        .connect_controlled(
                            &profile,
                            Secret::password("synthetic-password"),
                            &control,
                        )
                        .is_err(),
                )
                .unwrap();
        });
        let target = target.recv_timeout(Duration::from_secs(3)).unwrap();
        setup.recv_timeout(Duration::from_secs(3)).unwrap();
        flag.store(true, Ordering::SeqCst);
        target.abort_transport();
        assert!(result.recv_timeout(Duration::from_secs(2)).unwrap());
        worker.join().unwrap();
        server.join().unwrap();
    }
}

#[test]
fn cancellation_at_registration_closes_the_socket_before_startup() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let profile = profile(&listener, false);
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        assert_eq!(socket.read(&mut [0; 32]).unwrap(), 0);
    });
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let registrations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let control = ConnectionControl::new(
        cancelled,
        Arc::new(move |_| {
            // The first registration guards DNS/TCP connect; the second is the
            // connected socket right before startup bytes are written.
            if registrations.fetch_add(1, Ordering::SeqCst) == 1 {
                flag.store(true, Ordering::SeqCst);
            }
            Ok(())
        }),
    );
    assert!(
        PostgresConnector::default()
            .connect_controlled(&profile, Secret::password("synthetic-password"), &control)
            .is_err()
    );
    server.join().unwrap();
}

#[test]
fn cancellation_before_tcp_connect_aborts_immediately_and_preview_keeps_preview_frame_limit() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = profile(&listener, false);
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let control = ConnectionControl::new(
        cancelled,
        Arc::new(move |_| {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        }),
    );
    let started = Instant::now();
    let error = PostgresConnector::default()
        .connect_controlled(&cfg, Secret::password("synthetic-password"), &control)
        .err()
        .unwrap();
    assert!(error.is::<crate::export::Cancelled>(), "{error:#}");
    assert!(started.elapsed() < Duration::from_secs(2));
    listener.set_nonblocking(true).unwrap();
    assert!(matches!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    ));

    // A non-export ConnectionControl keeps PREVIEW_FRAME_BYTES; for_export uses EXPORT_FRAME_BYTES.
    for export in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = profile(&listener, false);
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut length = [0; 4];
            socket.read_exact(&mut length).unwrap();
            socket
                .read_exact(&mut vec![0; u32::from_be_bytes(length) as usize - 4])
                .unwrap();
            // Advertise 16 MiB + 1 bytes (> EXPORT_FRAME_BYTES, < PREVIEW_FRAME_BYTES), then EOF.
            let oversized = (transport::EXPORT_FRAME_BYTES as u32 + 1).to_be_bytes();
            let mut header = [b'R', 0, 0, 0, 0];
            header[1..].copy_from_slice(&oversized);
            let _ = socket.write_all(&header);
        });
        let control =
            ConnectionControl::new(Arc::new(AtomicBool::new(false)), Arc::new(|_| Ok(())));
        let control = if export {
            control.for_export()
        } else {
            control
        };
        let error = format!(
            "{:#}",
            PostgresConnector::default()
                .connect_controlled(&cfg, Secret::password("synthetic-password"), &control)
                .err()
                .unwrap()
        );
        if export {
            assert!(error.contains("transport limit"), "{error}");
        } else {
            assert!(!error.contains("transport limit"), "{error}");
        }
        server.join().unwrap();
    }
}

#[test]
fn startup_rejects_a_large_advertised_backend_frame_without_reading_its_body() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let profile = profile(&listener, false);
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut length = [0; 4];
        socket.read_exact(&mut length).unwrap();
        let count = u32::from_be_bytes(length) as usize;
        assert!((8..4096).contains(&count));
        socket.read_exact(&mut vec![0; count - 4]).unwrap();
        socket.write_all(b"R\x7f\xff\xff\xff").unwrap();
        assert_eq!(socket.read(&mut [0; 32]).unwrap(), 0);
    });
    let error = PostgresConnector::default()
        .connect(&profile, Secret::password("synthetic-password"))
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("transport limit"));
    server.join().unwrap();
}

#[test]
fn connect_abort_interrupts_in_flight_future_and_second_address_retry() {
    // 1. Aborting an actively running ConnectAbort::run() future wakes immediately.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let connect_abort = transport::ConnectAbort::new();
    let owner = connect_abort.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        runtime.block_on(owner.run(async {
            started_tx.send(()).unwrap();
            std::future::pending::<Result<()>>().await
        }))
    });
    started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let started = Instant::now();
    connect_abort.abort_transport();
    let error = handle.join().unwrap().unwrap_err();
    assert!(error.is::<crate::export::Cancelled>(), "{error:#}");
    assert!(started.elapsed() < Duration::from_secs(2));

    // 2. When the first resolved address fails startup, ConnectAbort stays registered
    // and cancels the second address attempt immediately.
    let first = TcpListener::bind("127.0.0.1:0").unwrap();
    let second = TcpListener::bind("127.0.0.1:0").unwrap();
    let first_addr = first.local_addr().unwrap();
    let second_addr = second.local_addr().unwrap();
    let (second_ready_tx, second_ready_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        // First address accepts and closes immediately so startup fails and retries second.
        let (mut socket1, _) = first.accept().unwrap();
        socket1
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut length = [0; 4];
        socket1.read_exact(&mut length).unwrap();
        drop(socket1);

        // Second address accepts startup and stalls until cancelled.
        let (mut socket2, _) = second.accept().unwrap();
        socket2
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut length = [0; 4];
        socket2.read_exact(&mut length).unwrap();
        socket2
            .read_exact(&mut vec![0; u32::from_be_bytes(length) as usize - 4])
            .unwrap();
        second_ready_tx.send(()).unwrap();
        assert_eq!(socket2.read(&mut [0; 16]).unwrap(), 0);
    });

    let latest_target: Arc<std::sync::Mutex<Option<Arc<dyn Cancellation>>>> = Arc::default();
    let slot = latest_target.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let control = ConnectionControl::new(
        cancelled,
        Arc::new(move |transport| {
            *slot.lock().unwrap() = Some(transport);
            Ok(())
        }),
    );
    let worker = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let connect_abort = transport::ConnectAbort::new();
        control.register(connect_abort.clone()).unwrap();
        let mut config = Config::new();
        config
            .user("synthetic-user")
            .dbname("synthetic-db")
            .password("synthetic-password")
            .ssl_mode(SslMode::Disable);
        let result = runtime.block_on(connect_abort.run(connect_addresses(
            [first_addr, second_addr],
            &config,
            None,
            "127.0.0.1",
            transport::PREVIEW_FRAME_BYTES,
            &connect_abort,
            Some(&control),
        )));
        assert!(result.is_err());
    });
    second_ready_rx
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    let started = Instant::now();
    flag.store(true, Ordering::SeqCst);
    latest_target
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .abort_transport();
    worker.join().unwrap();
    server.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn startup_metadata_replacements_are_bounded_and_empty_notices_cannot_bypass_the_count() {
    for case in 0..3 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let profile = profile(&listener, false);
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut length = [0; 4];
            socket.read_exact(&mut length).unwrap();
            socket
                .read_exact(&mut vec![0; u32::from_be_bytes(length) as usize - 4])
                .unwrap();
            socket.write_all(b"R\0\0\0\x08\0\0\0\0").unwrap();
            let count = if case == 2 { 257 } else { 4097 };
            for index in 0..count {
                let (tag, body) = if case == 2 {
                    (b'N', vec![0])
                } else {
                    let key = if case == 0 {
                        "replacement".into()
                    } else {
                        format!("key{index}")
                    };
                    let value = if case == 0 {
                        "x".repeat(1024)
                    } else {
                        "v".into()
                    };
                    (b'S', format!("{key}\0{value}\0").into_bytes())
                };
                socket.write_all(&[tag]).unwrap();
                socket
                    .write_all(&((body.len() + 4) as u32).to_be_bytes())
                    .unwrap();
                socket.write_all(&body).unwrap();
            }
            if case == 0 {
                socket.write_all(b"Z\0\0\0\x05I").unwrap();
            }
            assert_eq!(socket.read(&mut [0; 1]).unwrap(), 0);
        });
        let result =
            PostgresConnector::default().connect(&profile, Secret::password("synthetic-password"));
        if case == 0 {
            result.unwrap().close().unwrap();
        } else {
            let error = format!("{:#}", result.err().unwrap());
            assert!(
                error.contains(if case == 1 {
                    "session metadata exceeds its limit"
                } else {
                    "startup notices exceed their metadata limit"
                }),
                "{error}"
            );
        }
        server.join().unwrap();
    }
}
