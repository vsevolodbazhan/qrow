//! Disposable concurrent HTTPS storage peers with synthetic certificates.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub enum Reply {
    Bytes(u16, Vec<u8>, Vec<(String, String)>),
    Stall,
}
impl Reply {
    pub fn bytes(bytes: Vec<u8>) -> Self {
        Self::Bytes(200, bytes, Vec::new())
    }
    pub fn status(status: u16) -> Self {
        Self::Bytes(status, Vec::new(), Vec::new())
    }
    pub fn redirect(url: String) -> Self {
        Self::Bytes(307, Vec::new(), vec![("Location".into(), url)])
    }
}
pub struct Server {
    pub origin: String,
    recorded: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<Vec<TcpStream>>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn new(
        config: Arc<rustls::ServerConfig>,
        handler: impl Fn(&str) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        );
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let requests = recorded.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let owned = sockets.clone();
        let handler = Arc::new(handler);
        let thread = thread::spawn(move || {
            let mut handles = Vec::new();
            while !stopping.load(Ordering::SeqCst) {
                let tcp = match listener.accept() {
                    Ok((tcp, _)) => tcp,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                tcp.set_nonblocking(false).unwrap();
                tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
                owned.lock().unwrap().push(tcp.try_clone().unwrap());
                let config = config.clone();
                let requests = requests.clone();
                let handler = handler.clone();
                handles.push(thread::spawn(move || {
                    let mut stream=rustls::StreamOwned::new(rustls::ServerConnection::new(config).unwrap(),tcp);
                    let mut head=Vec::new();let mut byte=[0];
                    while !head.ends_with(b"\r\n\r\n") {
                        if stream.read(&mut byte).unwrap_or(0)==0 {return}
                        head.push(byte[0]);assert!(head.len()<=65536);
                    }
                    let mut head=String::from_utf8(head).unwrap();
                    let length=head.lines().find_map(|line|line.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|v|v.parse::<usize>().ok())).unwrap_or(0);
                    assert!(length<=1024*1024);
                    let mut body=vec![0;length];
                    if stream.read_exact(&mut body).is_err() {return}
                    head.push_str(&String::from_utf8(body).unwrap());
                    requests.lock().unwrap().push(head.clone());
                    match handler(&head) {
                        Reply::Bytes(status,bytes,headers)=>{
                            let mut head=format!("HTTP/1.1 {status} Result\r\nConnection: close\r\nContent-Length: {}\r\n",bytes.len());
                            for (name,value) in headers {head.push_str(&format!("{name}: {value}\r\n"));}
                            head.push_str("\r\n");
                            let _=stream.write_all(head.as_bytes());let _=stream.write_all(&bytes);let _=stream.flush();
                        }
                        Reply::Stall=>{
                            let _=stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n[");let _=stream.flush();
                            while stream.read(&mut byte).unwrap_or(0)>0 {}
                        }
                    }
                }));
            }
            for handle in handles {
                handle.join().unwrap();
            }
        });
        Self {
            origin,
            recorded,
            stop,
            sockets,
            thread: Some(thread),
        }
    }
    pub fn requests(&self) -> Vec<String> {
        self.recorded.lock().unwrap().clone()
    }
    pub fn wait_for(&self, path: &str, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(4);
        while self
            .requests()
            .iter()
            .filter(|h| h.starts_with(&format!("GET {path} ")))
            .count()
            < count
        {
            assert!(Instant::now() < deadline, "Missing storage request {path}");
            thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for socket in self.sockets.lock().unwrap().iter() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        self.thread.take().unwrap().join().unwrap();
    }
}
