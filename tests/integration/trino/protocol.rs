use super::*;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

trait Stream: Read + Write {}
impl<T: Read + Write> Stream for T {}
struct Server {
    profile: Profile,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
struct Reply {
    body: String,
    headers: String,
    status: u16,
}
impl Reply {
    fn page(value: serde_json::Value) -> Self {
        Self {
            body: value.to_string(),
            headers: String::new(),
            status: 200,
        }
    }
    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push_str(&format!("{name}: {value}\r\n"));
        self
    }
}
impl Server {
    fn new(tls: bool, replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let origin = format!("{}://localhost:{port}", if tls { "https" } else { "http" });
        let config = crate::oidc_provider::server_config();
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
                let body = reply.body.replace("{origin}", &origin);
                let response = format!(
                    "HTTP/1.1 {} OK\r\nConnection: close\r\nContent-Length: {}\r\n{}\r\n{}",
                    reply.status,
                    body.len(),
                    reply.headers,
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
    fn connect(&self) -> Box<dyn Session> {
        DatabaseConnector::new(crate::oidc_provider::trust())
            .connect(
                &self.profile,
                Secret::password(if self.profile.tls {
                    "synthetic-password"
                } else {
                    ""
                }),
            )
            .unwrap()
    }
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn done() -> Reply {
    Reply::page(json!({"columns":[{"name":"value","type":"integer"}],"data":[[1]]}))
}

#[test]
fn pages_use_the_latest_cursor_and_preserve_values_and_session_headers() -> Result<()> {
    let server = Server::new(
        false,
        vec![
            done(),
            Reply::page(json!({"nextUri":"{origin}/next/1"}))
                .header("X-Trino-Set-Schema", "new_schema")
                .header("X-Trino-Set-Session", "name=a%2Cb%3Dc"),
            Reply::page(
                json!({"nextUri":"{origin}/next/2","columns":[{"name":"value","type":"varchar"}],"data":[[null],[""],["α🦀"]]}),
            ),
            Reply::page(json!({"data":[[12345678901234567890u64],[true],[[1,2]]]})),
            done(),
        ],
    );
    let mut session = server.connect();
    complete(&mut *session, "SELECT value")?;
    assert_eq!(
        session.fetch(2)?.rows,
        vec![vec![None], vec![Some("".into())]]
    );
    assert_eq!(
        session.fetch(10)?.rows,
        vec![
            vec![Some("α🦀".into())],
            vec![Some("12345678901234567890".into())],
            vec![Some("true".into())],
            vec![Some("[1,2]".into())]
        ]
    );
    complete(&mut *session, "SELECT 2")?;
    let requests = server.requests();
    assert!(requests[2].starts_with("GET /next/1 "));
    assert!(requests[3].starts_with("GET /next/2 "));
    let last = requests.last().unwrap().to_lowercase();
    assert!(last.contains("x-trino-schema: new_schema\r\n"));
    assert!(last.contains("x-trino-session: name=a%2cb%3dc\r\n"));
    assert!(
        !requests
            .iter()
            .any(|request| request.to_lowercase().contains("authorization:"))
    );
    session.close()
}

#[test]
fn verified_https_sends_password_or_refreshable_bearer_tokens() -> Result<()> {
    use qrow::connector::TokenSource;
    use zeroize::Zeroizing;
    struct Tokens;
    impl TokenSource for Tokens {
        fn access_token(&self) -> Result<Zeroizing<String>> {
            Ok(Zeroizing::new("synthetic-token".into()))
        }
    }
    let server = Server::new(true, vec![done(), done(), done()]);
    let mut session = server.connect();
    complete(&mut *session, "SELECT 2")?;
    session.close()?;
    let mut token_session = DatabaseConnector::new(crate::oidc_provider::trust())
        .connect(&server.profile, Secret::Token(Arc::new(Tokens)))?;
    token_session.close()?;
    let requests = server.requests();
    assert!(requests[0].to_lowercase().contains("authorization: basic "));
    assert!(requests[2].contains("synthetic-token"));
    let mut plain = server.profile.clone();
    plain.tls = false;
    assert!(
        DatabaseConnector::default()
            .connect(&plain, Secret::password("must-not-leave-client"))
            .is_err()
    );
    assert!(
        DatabaseConnector::default()
            .connect(&plain, Secret::Token(Arc::new(Tokens)))
            .is_err()
    );
    assert_eq!(server.requests().len(), 3);
    Ok(())
}

#[test]
fn cancellation_deletes_the_current_query_and_old_handles_do_not_target_new_work() -> Result<()> {
    let server = Server::new(
        false,
        vec![
            done(),
            Reply::page(json!({"nextUri":"{origin}/query/one"})),
            Reply::page(json!({"nextUri":"{origin}/query/two"})),
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
            done(),
        ],
    );
    let mut session = server.connect();
    let cancel = session.execute("SELECT slow")?;
    assert_eq!(session.poll()?, QueryState::Running);
    cancel.cancel()?;
    assert_eq!(session.poll()?, QueryState::Cancelled);
    complete(&mut *session, "SELECT 42")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("1"));
    let requests = server.requests();
    assert!(requests[3].starts_with("DELETE /query/two "));
    assert!(requests[4].starts_with("DELETE /query/two "));
    assert!(requests[5].starts_with("POST /v1/statement "));
    session.close()
}

#[test]
fn rejects_off_origin_cursors_redirects_bad_json_and_oversized_responses() {
    for reply in [
        Reply::page(json!({"nextUri":"http://example.test/steal"})),
        Reply {
            status: 302,
            body: String::new(),
            headers: "Location: http://example.test/steal\r\n".into(),
        },
        Reply {
            status: 200,
            body: "invalid-json".into(),
            headers: String::new(),
        },
        Reply {
            status: 200,
            body: "x".repeat(16 * 1024 * 1024 + 1),
            headers: String::new(),
        },
    ] {
        let server = Server::new(false, vec![reply]);
        assert!(
            DatabaseConnector::default()
                .connect(&server.profile, Secret::password(""))
                .is_err()
        );
        assert_eq!(server.requests().len(), 1);
    }
}

#[test]
fn sql_errors_keep_the_session_and_multiple_statements_never_reach_the_server() -> Result<()> {
    let server = Server::new(
        false,
        vec![
            done(),
            Reply::page(
                json!({"error":{"errorName":"SYNTAX_ERROR","message":"Synthetic SQL error"}}),
            ),
            done(),
        ],
    );
    let mut session = server.connect();
    assert!(session.execute("SELECT 1; SELECT 2").is_err());
    assert_eq!(server.requests().len(), 1);
    let error = session.execute("SELECT broken").err().unwrap();
    assert!(error.downcast_ref::<QueryError>().is_some());
    complete(&mut *session, "SELECT 3; -- trailing comment")?;
    assert!(server.requests().last().unwrap().ends_with("SELECT 3"));
    session.close()
}

#[test]
fn response_headers_preserve_and_clear_transactions_roles_and_prepared_statements() -> Result<()> {
    let server = Server::new(
        false,
        vec![
            done(),
            Reply::page(json!({}))
                .header("X-Trino-Started-Transaction-Id", "test-transaction")
                .header("X-Trino-Set-Role", "tpch=ROLE%7Breader%7D")
                .header("X-Trino-Added-Prepare", "answer=SELECT+7"),
            done(),
            Reply::page(json!({}))
                .header("X-Trino-Clear-Transaction-Id", "true")
                .header("X-Trino-Deallocated-Prepare", "answer")
                .header("X-Trino-Clear-Session", "setting"),
            done(),
        ],
    );
    let mut profile = server.profile.clone();
    profile.parameters.insert("setting".into(), "a,b=c".into());
    let mut session = DatabaseConnector::default().connect(&profile, Secret::password(""))?;
    complete(&mut *session, "START TRANSACTION")?;
    complete(&mut *session, "EXECUTE answer")?;
    complete(&mut *session, "ROLLBACK")?;
    complete(&mut *session, "SELECT 3")?;
    let requests = server.requests();
    let inside = requests[2].to_lowercase();
    let after = requests[4].to_lowercase();
    assert!(inside.contains("x-trino-transaction-id: test-transaction"));
    assert!(inside.contains("x-trino-role: tpch=role%7breader%7d"));
    assert!(inside.contains("x-trino-prepared-statement: answer=select+7"));
    assert!(inside.contains("x-trino-session: setting=a%2cb%3dc"));
    assert!(after.contains("x-trino-transaction-id: none"));
    assert!(!after.contains("x-trino-prepared-statement:"));
    assert!(!after.contains("x-trino-session:"));
    session.close()
}

#[test]
fn disconnect_rolls_back_an_open_transaction() -> Result<()> {
    let server = Server::new(
        false,
        vec![
            done(),
            Reply::page(json!({})).header("X-Trino-Started-Transaction-Id", "test-transaction"),
            Reply::page(json!({})).header("X-Trino-Clear-Transaction-Id", "true"),
        ],
    );
    let mut session = server.connect();
    complete(&mut *session, "START TRANSACTION")?;
    session.close()?;
    assert!(server.requests()[2].ends_with("ROLLBACK"));
    session.close()?;
    assert_eq!(server.requests().len(), 3);
    Ok(())
}
