//! SASL PLAIN over TLS against a local server with a synthetic certificate.
use crate::oidc_provider::{other_trust, server_config, trust};
use qrow::{
    connector::{Connector, Secret, TokenSource, hive::HiveConnector, sasl},
    model::{Authentication, Profile},
};
use rustls::{ServerConnection, StreamOwned};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Accepts one TLS connection, reads the SASL PLAIN exchange, and answers
/// with `status`. Returns the PLAIN response that the client sent.
fn sasl_server(status: u8) -> (u16, thread::JoinHandle<Option<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut stream = StreamOwned::new(ServerConnection::new(server_config()).unwrap(), tcp);
        let mut response = None;
        for expected in [1, 2] {
            let mut header = [0; 5];
            stream.read_exact(&mut header).ok()?;
            assert_eq!(header[0], expected);
            let mut payload = vec![0; u32::from_be_bytes(header[1..].try_into().unwrap()) as usize];
            stream.read_exact(&mut payload).unwrap();
            if expected == 2 {
                response = Some(payload);
            } else {
                assert_eq!(payload, b"PLAIN");
            }
        }
        stream.write_all(&[status, 0, 0, 0, 0]).unwrap();
        stream.flush().unwrap();
        response
    });
    (port, server)
}

#[test]
fn sasl_plain_runs_inside_tls_with_the_token_in_the_password_field() {
    let (port, server) = sasl_server(5);
    let trust = trust();
    let endpoint = sasl::Endpoint {
        host: "127.0.0.1",
        port,
        tls: Some(&trust),
        read_timeout: Duration::from_secs(5),
    };
    sasl::connect(&endpoint, "kyuubi-analytics", "access-token").unwrap();
    assert_eq!(
        server.join().unwrap().unwrap(),
        b"\0kyuubi-analytics\0access-token"
    );
}

#[test]
fn an_untrusted_server_certificate_stops_before_credentials_are_sent() {
    let (port, server) = sasl_server(5);
    let trust = other_trust();
    let endpoint = sasl::Endpoint {
        host: "127.0.0.1",
        port,
        tls: Some(&trust),
        read_timeout: Duration::from_secs(5),
    };
    let error = format!(
        "{:#}",
        sasl::connect(&endpoint, "user", "secret-token")
            .err()
            .unwrap()
    );
    assert!(error.contains("TLS handshake"), "{error}");
    assert!(!error.contains("secret-token"));
    assert_eq!(server.join().unwrap(), None);
}

#[test]
fn a_certificate_for_another_host_is_rejected() {
    let (port, server) = sasl_server(5);
    let tcp = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    // The certificate names 127.0.0.1 and localhost only.
    let error = format!(
        "{:#}",
        trust().connect("kyuubi.example.test", tcp).err().unwrap()
    );
    assert!(error.contains("TLS handshake"), "{error}");
    assert_eq!(server.join().unwrap(), None);
}

struct Tokens(AtomicUsize);
impl TokenSource for Tokens {
    fn access_token(&self) -> anyhow::Result<Zeroizing<String>> {
        let number = self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Zeroizing::new(format!("token-{number}")))
    }
}

fn oidc_profile(port: u16, tls: bool) -> Profile {
    Profile {
        host: "127.0.0.1".into(),
        port,
        username: "kyuubi-analytics".into(),
        tls,
        authentication: Authentication::Oidc {
            sign_in: Uuid::new_v4(),
        },
        ..Profile::default()
    }
}

#[test]
fn a_rejected_token_names_the_database_account() {
    let (port, server) = sasl_server(4);
    let tokens = Arc::new(Tokens(AtomicUsize::new(0)));
    let error = HiveConnector::new(trust())
        .connect(&oidc_profile(port, true), Secret::Token(tokens))
        .err()
        .unwrap();
    let message = format!("{error:#}");
    assert!(
        message.contains(
            "did not accept the access token for the database account \"kyuubi-analytics\""
        ),
        "{message}"
    );
    assert!(!message.contains("token-0"));
    assert_eq!(
        server.join().unwrap().unwrap(),
        b"\0kyuubi-analytics\0token-0"
    );
}

#[test]
fn a_token_is_never_sent_without_tls_or_with_a_password_secret() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let tokens = Arc::new(Tokens(AtomicUsize::new(0)));
    let connector = HiveConnector::new(trust());
    let error = connector
        .connect(&oidc_profile(port, false), Secret::Token(tokens.clone()))
        .err()
        .unwrap();
    assert!(error.to_string().contains("requires TLS"), "{error}");
    let error = connector
        .connect(&oidc_profile(port, true), Secret::password("password"))
        .err()
        .unwrap();
    assert!(error.to_string().contains("do not match"), "{error}");
    let password_profile = Profile {
        authentication: Authentication::Password,
        ..oidc_profile(port, true)
    };
    assert!(
        connector
            .connect(&password_profile, Secret::Token(tokens.clone()))
            .is_err()
    );
    assert!(listener.accept().is_err(), "no connection was opened");
    assert_eq!(tokens.0.load(Ordering::SeqCst), 0);
}
