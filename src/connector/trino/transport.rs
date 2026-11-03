//! Owned raw sockets and request futures for a Trino session.
use anyhow::Result;
use futures_util::future::{AbortHandle, Abortable};
use std::{
    future::Future,
    io,
    net::{Shutdown, TcpStream},
    os::fd::AsFd,
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

const MAX_SOCKETS: usize = 8;
const MAX_REQUESTS: usize = 8;

struct SocketLease(TcpStream);
struct RequestLease(AbortHandle);
#[derive(Default)]
struct State {
    closed: bool,
    sockets: Vec<(u64, Weak<SocketLease>)>,
    requests: Vec<(u64, Weak<RequestLease>)>,
}
#[derive(Default)]
struct Group {
    next: AtomicU64,
    state: Mutex<State>,
}

#[derive(Default)]
struct Activity {
    state: Mutex<(bool, usize)>,
    changed: Condvar,
}

pub(super) struct ActivityGuard(Arc<Activity>);
impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.1 -= 1;
        self.0.changed.notify_all();
    }
}

pub(super) struct Transport {
    group: Arc<Group>,
    id: u64,
    closed: AtomicBool,
    activity: Arc<Activity>,
}

impl Transport {
    pub fn new() -> Arc<Self> {
        Self::scope(Arc::default())
    }

    fn scope(group: Arc<Group>) -> Arc<Self> {
        let id = group.next.fetch_add(1, Ordering::Relaxed);
        Arc::new(Self {
            group,
            id,
            closed: AtomicBool::new(false),
            activity: Arc::default(),
        })
    }

    pub fn child(&self) -> Arc<Self> {
        Self::scope(self.group.clone())
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst) || self.group.state.lock().unwrap().closed
    }

    /// Covers authentication, response headers, body and decoding as one read.
    pub fn activity(&self) -> Result<ActivityGuard> {
        let mut state = self.activity.state.lock().unwrap();
        anyhow::ensure!(!state.0 && !self.is_closed(), crate::export::Cancelled);
        anyhow::ensure!(state.1 < MAX_REQUESTS, "Trino activity limit reached");
        state.1 += 1;
        Ok(ActivityGuard(self.activity.clone()))
    }

    pub fn seal(&self) {
        self.activity.state.lock().unwrap().0 = true;
    }

    /// A successful DELETE does not prove an earlier response read has stopped.
    pub fn wait_idle(&self, deadline: Instant) -> Result<()> {
        let mut state = self.activity.state.lock().unwrap();
        while state.1 != 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            anyhow::ensure!(
                !remaining.is_zero(),
                "Trino primary cleanup deadline expired"
            );
            state = self
                .activity
                .changed
                .wait_timeout(state, remaining)
                .unwrap()
                .0;
        }
        Ok(())
    }

    /// The lease remains in reqwest's connecting future and pooled connection.
    pub fn register(&self, stream: &tokio::net::TcpStream) -> io::Result<Arc<dyn Send + Sync>> {
        let socket = TcpStream::from(stream.as_fd().try_clone_to_owned()?);
        let mut state = self.group.state.lock().unwrap();
        state
            .sockets
            .retain(|(_, socket)| socket.strong_count() != 0);
        if state.closed || self.closed.load(Ordering::SeqCst) || state.sockets.len() >= MAX_SOCKETS
        {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Trino transport is closed or at its socket limit",
            ));
        }
        let lease = Arc::new(SocketLease(socket));
        state.sockets.push((self.id, Arc::downgrade(&lease)));
        Ok(lease)
    }

    pub async fn run<T>(&self, future: impl Future<Output = Result<T>>) -> Result<T> {
        let (abort, registration) = AbortHandle::new_pair();
        let lease = Arc::new(RequestLease(abort));
        {
            let mut state = self.group.state.lock().unwrap();
            state
                .requests
                .retain(|(_, request)| request.strong_count() != 0);
            anyhow::ensure!(
                !state.closed && !self.closed.load(Ordering::SeqCst),
                crate::export::Cancelled
            );
            anyhow::ensure!(
                state.requests.len() < MAX_REQUESTS,
                "Trino transport is at its request limit"
            );
            state.requests.push((self.id, Arc::downgrade(&lease)));
        }
        let result = Abortable::new(future, registration).await;
        drop(lease);
        result.map_err(|_| anyhow::Error::new(crate::export::Cancelled))?
    }

    pub fn close(&self) {
        self.shutdown(false);
    }

    pub fn close_all(&self) {
        self.shutdown(true);
    }

    fn shutdown(&self, all: bool) {
        let mut state = self.group.state.lock().unwrap();
        self.closed.store(true, Ordering::SeqCst);
        state.closed |= all;
        for (id, socket) in &state.sockets {
            if (all || *id == self.id)
                && let Some(socket) = socket.upgrade()
            {
                let _ = socket.0.shutdown(Shutdown::Both);
            }
        }
        for (id, request) in &state.requests {
            if (all || *id == self.id)
                && let Some(request) = request.upgrade()
            {
                request.0.abort();
            }
        }
        state
            .sockets
            .retain(|(_, socket)| socket.strong_count() != 0);
        state
            .requests
            .retain(|(_, request)| request.strong_count() != 0);
    }
}

impl super::super::Cancellation for Transport {
    fn cancel(&self) -> Result<()> {
        self.close_all();
        Ok(())
    }

    fn abort_transport(&self) {
        self.close_all();
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.close();
    }
}

/// Stop owned runtime work without waiting for an already-running system DNS helper.
pub(super) struct Runtime(Option<tokio::runtime::Runtime>);
impl Runtime {
    pub fn new() -> Result<Self> {
        Ok(Self(Some(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        )))
    }
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.0
            .as_ref()
            .expect("live Trino runtime")
            .block_on(future)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}
