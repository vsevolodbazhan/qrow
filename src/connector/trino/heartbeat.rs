//! Independently driven HEAD requests while the result consumer is paused.
use super::{
    protocol::{Http, SessionHeaders},
    transport::Transport,
};
use anyhow::Result;
use std::{
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use url::Url;

const INTERVAL: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Default)]
struct State {
    stopped: bool,
    disabled: bool,
    route: Option<(Url, SessionHeaders)>,
}
pub(super) struct Heartbeat {
    state: Arc<(Mutex<State>, Condvar)>,
    transport: Arc<Transport>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Heartbeat {
    pub fn new(http: Arc<Http>) -> Result<Self> {
        let state: Arc<(Mutex<State>, Condvar)> = Arc::default();
        let shared = state.clone();
        let transport = http.transport.child();
        let sockets = transport.clone();
        let thread = thread::Builder::new()
            .name("trino-heartbeat".into())
            .spawn(move || {
                let (mutex, changed) = &*shared;
                loop {
                    let mut state = mutex.lock().unwrap();
                    if state.stopped {
                        break;
                    }
                    state = changed.wait_timeout(state, INTERVAL).unwrap().0;
                    if state.stopped {
                        break;
                    }
                    let route = if state.disabled {
                        None
                    } else {
                        state.route.clone()
                    };
                    drop(state);
                    let Some((uri, headers)) = route else {
                        continue;
                    };
                    let response = http.auxiliary(
                        reqwest::Method::HEAD,
                        &uri,
                        &headers,
                        Instant::now() + REQUEST_TIMEOUT,
                        &sockets,
                    );
                    if response
                        .is_ok_and(|response| matches!(response.status().as_u16(), 404 | 405))
                    {
                        mutex.lock().unwrap().disabled = true;
                    }
                }
            })?;
        Ok(Self {
            state,
            transport,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Publish only after an executing GET established the server-side Query.
    pub fn update(&self, uri: Url, headers: &SessionHeaders) {
        let mut state = self.state.0.lock().unwrap();
        if !state.stopped && !state.disabled {
            state.route = Some((uri, headers.clone()));
        }
    }

    pub fn stop(&self) {
        self.state.0.lock().unwrap().stopped = true;
        self.transport.close();
        self.state.1.notify_all();
        if let Some(thread) = self.thread.lock().unwrap().take() {
            thread.join().expect("Trino heartbeat thread stopped");
        }
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.stop();
    }
}
