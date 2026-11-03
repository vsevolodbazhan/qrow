//! A local TCP relay that can pause responses from a disposable server.
use std::{
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Default)]
struct State {
    paused: AtomicBool,
    stopped: AtomicBool,
    blocked: AtomicUsize,
    sockets: Mutex<Vec<TcpStream>>,
}

pub struct ResponseGate {
    pub port: u16,
    state: Arc<State>,
    relay: Option<JoinHandle<()>>,
}

impl ResponseGate {
    pub fn new(server_port: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(State::default());
        let running = state.clone();
        let relay = thread::spawn(move || {
            let mut threads = Vec::new();
            while !running.stopped.load(Ordering::SeqCst) {
                let client = match listener.accept() {
                    Ok((client, _)) => client,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("Response gate accept failed: {error}"),
                };
                let server = TcpStream::connect_timeout(
                    &std::net::SocketAddr::from(([127, 0, 0, 1], server_port)),
                    Duration::from_secs(2),
                )
                .unwrap();
                for socket in [&client, &server] {
                    socket
                        .set_read_timeout(Some(Duration::from_millis(100)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    running
                        .sockets
                        .lock()
                        .unwrap()
                        .push(socket.try_clone().unwrap());
                }
                let (input, output, control) = (
                    client.try_clone().unwrap(),
                    server.try_clone().unwrap(),
                    running.clone(),
                );
                threads.push(thread::spawn(move || {
                    forward(input, output, control, false)
                }));
                let control = running.clone();
                threads.push(thread::spawn(move || {
                    forward(server, client, control, true)
                }));
            }
            for socket in running.sockets.lock().unwrap().iter() {
                let _ = socket.shutdown(Shutdown::Both);
            }
            for thread in threads {
                thread.join().unwrap();
            }
        });
        Self {
            port,
            state,
            relay: Some(relay),
        }
    }

    pub fn pause(&self) {
        self.state.paused.store(true, Ordering::SeqCst);
    }

    pub fn blocked(&self) -> bool {
        self.state.blocked.load(Ordering::SeqCst) > 0
    }

    pub fn resume(&self) {
        self.state.paused.store(false, Ordering::SeqCst);
    }
}

fn forward(mut input: TcpStream, mut output: TcpStream, state: Arc<State>, gate: bool) {
    let mut buffer = [0; 8192];
    while !state.stopped.load(Ordering::SeqCst) {
        let count = match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(_) => break,
        };
        if gate && state.paused.load(Ordering::SeqCst) {
            state.blocked.fetch_add(1, Ordering::SeqCst);
            while state.paused.load(Ordering::SeqCst) && !state.stopped.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(2));
            }
            state.blocked.fetch_sub(1, Ordering::SeqCst);
        }
        if state.stopped.load(Ordering::SeqCst) || output.write_all(&buffer[..count]).is_err() {
            break;
        }
    }
    let _ = output.shutdown(Shutdown::Write);
}

impl Drop for ResponseGate {
    fn drop(&mut self) {
        self.state.stopped.store(true, Ordering::SeqCst);
        self.resume();
        if self.relay.take().unwrap().join().is_err() && !thread::panicking() {
            panic!("Response gate relay failed");
        }
    }
}
