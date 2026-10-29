//! A scripted Trino HTTPS coordinator with synthetic credentials.
#![allow(dead_code)]
use anyhow::Result;
use qrow::{
    connector::{Connector, DatabaseConnector, Secret, Session},
    model::{DatabaseType, Profile},
    tls::Trust,
};
use serde_json::json;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
fn trust() -> Trust {
    Trust::from_pem(include_bytes!("../integration/testdata/tls/ca.pem")).unwrap()
}
fn server_config() -> Arc<rustls::ServerConfig> {
    use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, pem::PemObject};
    let chain =
        CertificateDer::pem_slice_iter(include_bytes!("../integration/testdata/tls/server.pem"))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        include_bytes!("../integration/testdata/tls/server-key.der").to_vec(),
    ));
    Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap(),
    )
}
trait Stream: Read + Write {}
impl<T: Read + Write> Stream for T {}
pub struct Server {
    pub profile: Profile,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
pub struct Reply {
    pub body: String,
    pub headers: String,
    pub status: u16,
}
impl Reply {
    pub fn page(value: serde_json::Value) -> Self {
        Self {
            body: value.to_string(),
            headers: String::new(),
            status: 200,
        }
    }
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push_str(&format!("{name}: {value}\r\n"));
        self
    }
}
impl Server {
    pub fn new(tls: bool, replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let origin = format!("{}://localhost:{port}", if tls { "https" } else { "http" });
        let config = server_config();
        let thread = thread::spawn(move || {
            let mut replies = replies.into_iter();
            while !stopping.load(Ordering::SeqCst) {
                let tcp: TcpStream = match listener.accept() {
                    Ok((tcp, _)) => tcp,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                tcp.set_nonblocking(false).unwrap();
                tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut stream: Box<dyn Stream> = if tls {
                    Box::new(rustls::StreamOwned::new(
                        rustls::ServerConnection::new(config.clone()).unwrap(),
                        tcp,
                    ))
                } else {
                    Box::new(tcp)
                };
                let mut request = Vec::new();
                let mut byte = [0; 1];
                loop {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    assert!(request.len() < 16384);
                }
                if request.is_empty() {
                    continue;
                }
                let head = String::from_utf8(request).unwrap();
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                recorded
                    .lock()
                    .unwrap()
                    .push(format!("{head}{}", String::from_utf8(body).unwrap()));
                let reply = replies.next().unwrap_or_else(|| Reply {
                    status: 204,
                    body: String::new(),
                    headers: String::new(),
                });
                if reply.status == 0 {
                    continue;
                }
                let body = reply.body.replace("{origin}", &origin);
                let response = format!(
                    "HTTP/1.1 {} OK\r\nConnection: close\r\nContent-Length: {}\r\n{}\r\n{}",
                    reply.status,
                    body.len(),
                    reply.headers.replace("{origin}", &origin),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            profile: Profile {
                database_type: DatabaseType::Trino,
                name: "Protocol test".into(),
                host: "localhost".into(),
                port,
                username: "qrow".into(),
                database: "tpch".into(),
                trino_schema: "tiny".into(),
                tls,
                ..Profile::default()
            },
            requests,
            stop,
            thread: Some(thread),
        }
    }
    pub fn connect_secret(&self, secret: Secret) -> Result<Box<dyn Session>> {
        DatabaseConnector::new(trust()).connect(&self.profile, secret)
    }
    pub fn connect(&self) -> Box<dyn Session> {
        self.connect_secret(Secret::password(if self.profile.tls {
            "synthetic-password"
        } else {
            ""
        }))
        .unwrap()
    }
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}
pub fn done() -> Reply {
    Reply::page(json!({"columns":[{"name":"value","type":"integer"}],"data":[[1]]}))
}
