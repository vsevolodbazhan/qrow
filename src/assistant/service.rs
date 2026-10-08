//! Background ownership of the Codex process and its synchronous protocol.
//!
//! Qrow's window thread sends commands and receives events. It never waits for
//! a Codex response, including while a query-tool call waits for the worker.
//! The worker thread sleeps until a command or Codex output arrives.

use super::{
    AssistantEvent, CodexHarness, Conversation, ConversationHistory, ConversationPage,
    HarnessSnapshot, LoginStart, TitleRequest, ToolCall, ToolDefinition, ToolResult, Turn,
    TurnRequest,
    codex::Input,
    inbox::{CommandSender, Inbox, SendError},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const COMMAND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 1_024;
/// The time that a background stop waits for the worker. After three quarters
/// of it, the stop kills the Codex process group.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
/// The only `AssistantEvent::Other` methods that the assistant panel reads.
/// The worker drops other notifications so that they do not wake the window.
const UI_NOTIFICATIONS: [&str; 2] = ["account/login/completed", "account/updated"];

/// The tool calls of each thread's current turn. Codex sends a call again
/// that waits for its answer when a client resumes the thread. Qrow runs each
/// call once.
#[derive(Default)]
struct ToolCallLedger {
    threads: BTreeMap<String, TurnCalls>,
}

#[derive(Default)]
struct TurnCalls {
    turn_id: String,
    received: BTreeSet<String>,
}

impl ToolCallLedger {
    /// Returns false for a call that Qrow already received in this turn.
    fn receive(&mut self, call: &ToolCall) -> bool {
        let calls = self.threads.entry(call.thread_id.clone()).or_default();
        if calls.turn_id != call.turn_id {
            *calls = TurnCalls {
                turn_id: call.turn_id.clone(),
                ..TurnCalls::default()
            };
        }
        calls.received.insert(call.call_id.clone())
    }

    /// Codex does not send the calls of a turn again after it ends.
    fn end_turn(&mut self, thread_id: &str) {
        self.threads.remove(thread_id);
    }
}

#[derive(Debug)]
pub enum Command {
    Login,
    CancelLogin(String),
    Refresh,
    Create(Vec<ToolDefinition>),
    Resume(String),
    Read(String),
    ReadOlder {
        thread_id: String,
        cursor: String,
    },
    Rename {
        thread_id: String,
        title: String,
    },
    Delete(String),
    GenerateTitle(TitleRequest),
    Start(TurnRequest),
    Steer {
        thread_id: String,
        turn_id: String,
        text: String,
        /// The workspace context that Qrow adds to `text`.
        context: serde_json::Value,
    },
    Interrupt {
        thread_id: String,
        turn_id: String,
    },
    Answer {
        call: ToolCall,
        result: ToolResult,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Login,
    CancelLogin,
    Refresh,
    Create,
    Resume,
    Read,
    ReadOlder,
    Rename,
    Delete,
    GenerateTitle,
    Start,
    Steer,
    Interrupt,
    Answer,
}

impl Command {
    fn operation(&self) -> Operation {
        match self {
            Self::Login => Operation::Login,
            Self::CancelLogin(_) => Operation::CancelLogin,
            Self::Refresh => Operation::Refresh,
            Self::Create(_) => Operation::Create,
            Self::Resume(_) => Operation::Resume,
            Self::Read(_) => Operation::Read,
            Self::ReadOlder { .. } => Operation::ReadOlder,
            Self::Rename { .. } => Operation::Rename,
            Self::Delete(_) => Operation::Delete,
            Self::GenerateTitle(_) => Operation::GenerateTitle,
            Self::Start(_) => Operation::Start,
            Self::Steer { .. } => Operation::Steer,
            Self::Interrupt { .. } => Operation::Interrupt,
            Self::Answer { .. } => Operation::Answer,
        }
    }

    fn identifier(&self) -> Option<String> {
        match self {
            Self::Resume(id) | Self::Read(id) | Self::Delete(id) | Self::CancelLogin(id) => {
                Some(id.clone())
            }
            Self::ReadOlder { thread_id, .. }
            | Self::Rename { thread_id, .. }
            | Self::Steer { thread_id, .. }
            | Self::Interrupt { thread_id, .. } => Some(thread_id.clone()),
            Self::Start(request) => Some(request.thread_id.clone()),
            Self::GenerateTitle(request) => Some(request.thread_id.clone()),
            Self::Answer { call, .. } => Some(call.call_id.clone()),
            Self::Login | Self::Refresh | Self::Create(_) => None,
        }
    }
}

#[derive(Debug)]
pub enum Event {
    Ready(HarnessSnapshot),
    Snapshot(HarnessSnapshot),
    LoginStarted(LoginStart),
    LoginCancelled(String),
    Created(Conversation),
    Resumed(Conversation),
    History(ConversationHistory),
    HistoryPage(ConversationPage),
    Renamed(String),
    Deleted(String),
    TitleRequested(String),
    TurnStarted {
        thread_id: String,
        turn: Turn,
    },
    Steered(String),
    Interrupted(String),
    ToolAnswered(String),
    Harness(AssistantEvent),
    Failed {
        operation: Operation,
        id: Option<String>,
        error: String,
    },
    Disconnected(String),
}

pub struct Service {
    commands: CommandSender,
    pub events: mpsc::Receiver<Event>,
    stopping: Arc<AtomicBool>,
    /// `None` after a stop request.
    exit: Option<WorkerExit>,
    cleanup_ids: Arc<Mutex<Vec<String>>>,
    cleanup_result: Arc<Mutex<Option<Result<(), String>>>>,
    /// The passes of the worker loop.
    #[cfg(test)]
    passes: Arc<std::sync::atomic::AtomicUsize>,
}

struct DoneSignal(mpsc::Sender<()>);
impl Drop for DoneSignal {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

/// The end of a worker thread after a stop request.
struct WorkerExit {
    pid: Arc<AtomicU32>,
    done: mpsc::Receiver<()>,
}

impl WorkerExit {
    /// Waits for the worker. After three quarters of `timeout`, kills the
    /// Codex process group, because a Codex descendant can keep its output
    /// open after Codex exits.
    fn wait(self, timeout: Duration) -> std::io::Result<()> {
        if self.done.recv_timeout(timeout - timeout / 4).is_ok() {
            return Ok(());
        }
        kill_codex(&self.pid);
        self.done
            .recv_timeout(timeout / 4)
            .map_err(|_| std::io::Error::other("Codex assistant did not stop"))
    }
}

fn kill_codex(pid: &AtomicU32) {
    let pid = pid.load(Ordering::Acquire);
    #[cfg(unix)]
    if pid > 1 {
        let _ = super::codex::terminate_process_group(pid);
    }
    #[cfg(windows)]
    if pid > 0 {
        let _ = super::codex::terminate_process_tree(pid);
    }
}

/// Services that stop in the background. A receiver disconnects when its stop
/// ends.
static BACKGROUND_STOPS: Mutex<Vec<mpsc::Receiver<()>>> = Mutex::new(Vec::new());

impl Service {
    pub fn launch(executable: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<Self> {
        let (commands, inbox) = Inbox::channel(COMMAND_CAPACITY);
        let (event_tx, events) = mpsc::sync_channel(EVENT_CAPACITY);
        let (done_tx, done) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let pid = Arc::new(AtomicU32::new(0));
        let cleanup_ids = Arc::new(Mutex::new(Vec::<String>::new()));
        let cleanup_result = Arc::new(Mutex::new(None));
        let thread_stopping = Arc::clone(&stopping);
        let thread_pid = Arc::clone(&pid);
        let thread_cleanup_ids = Arc::clone(&cleanup_ids);
        let thread_cleanup_result = Arc::clone(&cleanup_result);
        #[cfg(test)]
        let passes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        #[cfg(test)]
        let thread_passes = Arc::clone(&passes);
        thread::Builder::new()
            .name("qrow-assistant".into())
            .spawn(move || {
                let _done = DoneSignal(done_tx);
                let emit = |event: Event| {
                    let mut event = event;
                    loop {
                        if thread_stopping.load(Ordering::Acquire) {
                            return false;
                        }
                        match event_tx.try_send(event) {
                            Ok(()) => {
                                wake();
                                return true;
                            }
                            Err(mpsc::TrySendError::Full(pending)) => {
                                // A completed turn reloads its durable history. Discarding
                                // streamed fragments under UI backpressure keeps the protocol
                                // reader alive without losing the final message.
                                if matches!(
                                    &pending,
                                    Event::Harness(AssistantEvent::MessageDelta { .. })
                                ) {
                                    return true;
                                }
                                event = pending;
                                thread::sleep(Duration::from_millis(10));
                            }
                            Err(mpsc::TrySendError::Disconnected(_)) => return false,
                        }
                    }
                };
                let directory = match tempfile::Builder::new().prefix("qrow-assistant-").tempdir() {
                    Ok(directory) => directory,
                    Err(error) => {
                        emit(Event::Disconnected(format!(
                            "Could not create assistant workspace: {error}"
                        )));
                        return;
                    }
                };
                let mut harness = match CodexHarness::launch_with_inbox(
                    &executable,
                    directory.path(),
                    inbox,
                    move |id| thread_pid.store(id, Ordering::Release),
                ) {
                    Ok(harness) => harness,
                    Err(error) => {
                        emit(Event::Disconnected(error.to_string()));
                        return;
                    }
                };
                match harness.snapshot() {
                    Ok(snapshot) => {
                        if !emit(Event::Ready(snapshot)) {
                            return;
                        }
                    }
                    Err(error) => {
                        emit(Event::Disconnected(error.to_string()));
                        return;
                    }
                }
                let mut tool_calls = ToolCallLedger::default();
                loop {
                    #[cfg(test)]
                    thread_passes.fetch_add(1, Ordering::Relaxed);
                    if thread_stopping.load(Ordering::Acquire) {
                        break;
                    }
                    let event = match harness.next_input(None) {
                        Ok(Some(Input::Command(command))) => execute(&mut harness, command),
                        Ok(Some(Input::Event(event))) => {
                            match &event {
                                AssistantEvent::ToolCall(call) if !tool_calls.receive(call) => {
                                    continue;
                                }
                                AssistantEvent::Other { method, .. }
                                    if !UI_NOTIFICATIONS.contains(&method.as_str()) =>
                                {
                                    continue;
                                }
                                AssistantEvent::TurnCompleted { thread_id, .. } => {
                                    tool_calls.end_turn(thread_id);
                                }
                                _ => {}
                            }
                            Event::Harness(event)
                        }
                        Ok(Some(Input::Stop) | None) => break,
                        Err(error) => {
                            emit(Event::Disconnected(error.to_string()));
                            break;
                        }
                    };
                    if !emit(event) {
                        break;
                    }
                }
                if let Ok(mut ids) = thread_cleanup_ids.lock()
                    && !ids.is_empty()
                {
                    let result = harness.delete_conversations_on_shutdown(ids.drain(..));
                    if let Ok(mut outcome) = thread_cleanup_result.lock() {
                        *outcome = Some(result.map_err(|error| error.to_string()));
                    }
                }

                if let Err(error) = harness.shutdown() {
                    let _ = emit(Event::Disconnected(format!(
                        "Could not stop Codex: {error}"
                    )));
                }
            })?;
        Ok(Self {
            commands,
            events,
            stopping,
            exit: Some(WorkerExit { pid, done }),
            cleanup_ids,
            cleanup_result,
            #[cfg(test)]
            passes,
        })
    }

    pub fn send(&self, command: Command) -> Result<(), &'static str> {
        self.commands.send(command).map_err(|error| match error {
            SendError::Full => "Assistant is busy; try again.",
            SendError::Disconnected => "Assistant is disconnected.",
        })
    }

    /// Asks the worker to stop. A request in progress ends at once.
    fn request_stop(&mut self) -> Option<WorkerExit> {
        let exit = self.exit.take()?;
        self.stopping.store(true, Ordering::Release);
        self.commands.stop();
        Some(exit)
    }

    /// Stops the worker and Codex without a wait. Another thread waits and
    /// kills the Codex process group if necessary. `wait_for_background_stops`
    /// waits for that thread.
    pub fn stop(&mut self) {
        let Some(exit) = self.request_stop() else {
            return;
        };
        let pid = Arc::clone(&exit.pid);
        let (stopped, stop) = mpsc::channel::<()>();
        let spawned = thread::Builder::new()
            .name("qrow-assistant-stop".into())
            .spawn(move || {
                let _stopped = stopped;
                let _ = exit.wait(STOP_TIMEOUT);
            });
        if spawned.is_err() {
            kill_codex(&pid);
            return;
        }
        let mut stops = BACKGROUND_STOPS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        stops.retain(|stop| matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)));
        stops.push(stop);
    }

    /// Waits until the background stops end, at most until `deadline`. Quit
    /// calls this, so that no Codex process outlives Qrow.
    pub fn wait_for_background_stops(deadline: Instant) {
        let stops = std::mem::take(
            &mut *BACKGROUND_STOPS
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for stop in stops {
            let _ = stop.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        }
    }

    /// Stops the worker and Codex, and waits at most `timeout`.
    pub fn shutdown_and_wait(&mut self, timeout: Duration) -> std::io::Result<()> {
        match self.request_stop() {
            Some(exit) => exit.wait(timeout),
            None => Ok(()),
        }
    }

    pub fn shutdown_and_delete(
        &mut self,
        ids: Vec<String>,
        timeout: Duration,
    ) -> std::io::Result<()> {
        if ids.is_empty() {
            return self.shutdown_and_wait(timeout);
        }
        if let Ok(mut cleanup) = self.cleanup_ids.lock() {
            *cleanup = ids;
        }
        self.shutdown_and_wait(timeout)?;
        match self
            .cleanup_result
            .lock()
            .ok()
            .and_then(|mut result| result.take())
        {
            Some(Ok(())) => Ok(()),
            Some(Err(error)) => Err(std::io::Error::other(format!(
                "Codex could not delete demo conversations: {error}"
            ))),
            None => Err(std::io::Error::other(
                "Codex did not confirm demo conversation deletion",
            )),
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop();
    }
}

fn execute(harness: &mut CodexHarness, command: Command) -> Event {
    let operation = command.operation();
    let id = command.identifier();
    let result = match command {
        Command::Login => harness.begin_login().map(Event::LoginStarted),
        Command::CancelLogin(id) => harness
            .cancel_login(&id)
            .map(|()| Event::LoginCancelled(id)),
        Command::Refresh => harness.snapshot().map(Event::Snapshot),
        Command::Create(tools) => harness.create_conversation(&tools).map(Event::Created),
        Command::Resume(id) => harness.resume_conversation(&id).map(Event::Resumed),
        Command::Read(id) => harness.read_conversation(&id).map(Event::History),
        Command::ReadOlder { thread_id, cursor } => harness
            .read_older_conversation(&thread_id, &cursor)
            .map(Event::HistoryPage),
        Command::Rename { thread_id, title } => harness
            .rename_conversation(&thread_id, &title)
            .map(|()| Event::Renamed(thread_id)),
        Command::Delete(id) => harness
            .delete_conversation(&id)
            .map(|()| Event::Deleted(id)),
        Command::GenerateTitle(request) => {
            let thread_id = request.thread_id.clone();
            harness
                .generate_title(request)
                .map(|()| Event::TitleRequested(thread_id))
        }
        Command::Start(request) => {
            let thread_id = request.thread_id.clone();
            harness
                .start_turn(request)
                .map(|turn| Event::TurnStarted { thread_id, turn })
        }
        Command::Steer {
            thread_id,
            turn_id,
            text,
            context,
        } => harness
            .steer_turn(&thread_id, &turn_id, &text, &context)
            .map(|()| Event::Steered(thread_id)),
        Command::Interrupt { thread_id, turn_id } => harness
            .interrupt_turn(&thread_id, &turn_id)
            .map(|()| Event::Interrupted(thread_id)),
        Command::Answer { call, result } => {
            let call_id = call.call_id.clone();
            harness
                .answer_tool_call(&call, result)
                .map(|()| Event::ToolAnswered(call_id))
        }
    };
    result.unwrap_or_else(|error| Event::Failed {
        operation,
        id,
        error: error.to_string(),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;

    /// Covers the start of a fake app-server on a loaded machine.
    const EVENT_TIMEOUT: Duration = Duration::from_secs(15);

    fn call(thread: &str, turn: &str, id: &str) -> ToolCall {
        ToolCall {
            request_id: serde_json::json!(0),
            call_id: id.into(),
            thread_id: thread.into(),
            turn_id: turn.into(),
            name: "tab-read-sql".into(),
            arguments: serde_json::Value::Null,
        }
    }

    #[test]
    fn replayed_tool_calls_run_once_per_thread_and_turn() {
        let mut ledger = ToolCallLedger::default();
        let first = call("thread-a", "turn-a", "call-1");
        let other = call("thread-b", "turn-b", "call-1");
        assert!(ledger.receive(&first));
        assert!(ledger.receive(&other));
        // Codex sends a waiting call again after thread/resume.
        assert!(!ledger.receive(&first));
        // The end of one turn does not forget the calls of another thread.
        ledger.end_turn("thread-a");
        assert!(!ledger.receive(&other));
        assert!(ledger.receive(&first));
        // A new turn can use the same call ID again.
        assert!(ledger.receive(&call("thread-b", "turn-c", "call-1")));
    }

    /// Writes a fake app-server that answers the start requests. `setup` runs
    /// first, and `cases` adds `case` branches for other requests.
    fn fake_codex(path: &std::path::Path, setup: &str, cases: &str) {
        crate::assistant::write_test_executable(
            path,
            &format!(
                r#"#!/bin/sh
{setup}
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$0.log"
  id=$(printf '%s' "$line" | sed -nE 's/.*"id":([0-9]+).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{{"id":%s,"result":{{}}}}\n' "$id" ;;
    *'"method":"account/read"'*) printf '{{"id":%s,"result":{{"account":null,"requiresOpenaiAuth":true}}}}\n' "$id" ;;
    *'"method":"model/list"'*) printf '{{"id":%s,"result":{{"data":[],"nextCursor":null}}}}\n' "$id" ;;
{cases}
  esac
done
"#
            ),
        );
    }

    fn launch_ready(executable: &std::path::Path) -> Service {
        let service = Service::launch(executable.to_path_buf(), Arc::new(|| {})).unwrap();
        assert!(matches!(
            service.events.recv_timeout(EVENT_TIMEOUT).unwrap(),
            Event::Ready(_)
        ));
        service
    }

    fn assert_stopped(pid_file: &std::path::Path) {
        let pid = fs::read_to_string(pid_file).unwrap();
        let output = std::process::Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", pid.trim()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);
        assert!(
            !output.status.success() || state.trim_start().starts_with('Z'),
            "Codex descendant remained active with state {state:?}"
        );
    }

    #[test]
    fn queued_commands_run_without_a_wait_between_them() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        fake_codex(&executable, "", "");
        let service = launch_ready(&executable);
        let started = Instant::now();
        // An invalid ID fails before a request, so only the worker loop takes time.
        for _ in 0..20 {
            service.send(Command::Read(String::new())).unwrap();
        }
        for _ in 0..20 {
            assert!(matches!(
                service.events.recv_timeout(EVENT_TIMEOUT).unwrap(),
                Event::Failed {
                    operation: Operation::Read,
                    ..
                }
            ));
        }
        // A poll of 50 ms after each command took at least 950 ms.
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn idle_worker_waits_without_passes() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        fake_codex(&executable, "", "");
        let service = launch_ready(&executable);
        let deadline = Instant::now() + EVENT_TIMEOUT;
        while service.passes.load(Ordering::Relaxed) == 0 {
            assert!(Instant::now() < deadline, "the worker loop did not start");
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(100));
        let idle = service.passes.load(Ordering::Relaxed);
        thread::sleep(Duration::from_millis(300));
        assert_eq!(service.passes.load(Ordering::Relaxed), idle);
        service.send(Command::Read(String::new())).unwrap();
        assert!(matches!(
            service.events.recv_timeout(EVENT_TIMEOUT).unwrap(),
            Event::Failed { .. }
        ));
        thread::sleep(Duration::from_millis(100));
        assert_eq!(service.passes.load(Ordering::Relaxed), idle + 1);
    }

    #[test]
    fn request_in_flight_keeps_later_commands_in_order() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        fake_codex(
            &executable,
            "",
            r#"    *'"method":"thread/read"'*)
      i=0
      while [ "$i" -lt 200 ]; do printf '{"method":"notice","params":{"index":%s}}\n' "$i"; i=$((i + 1)); done
      sleep 0.5
      printf '{"id":%s,"result":{"thread":{"id":"thread-slow","name":null,"updatedAt":1,"turns":[]}}}\n' "$id" ;;
    *'"method":"thread/delete"'*) printf '{"id":%s,"result":{}}\n' "$id" ;;"#,
        );
        let service = launch_ready(&executable);
        service.send(Command::Read("thread-slow".into())).unwrap();
        // Codex output while the request waits does not fill the command bound.
        for index in 0..COMMAND_CAPACITY - 1 {
            let command = if index % 2 == 0 {
                Command::Delete(format!("thread-{index}"))
            } else {
                Command::Read(String::new())
            };
            service.send(command).unwrap();
        }
        assert!(matches!(
            service.events.recv_timeout(EVENT_TIMEOUT).unwrap(),
            Event::History(ConversationHistory { conversation, .. }) if conversation.id == "thread-slow"
        ));
        for index in 0..COMMAND_CAPACITY - 1 {
            match service.events.recv_timeout(EVENT_TIMEOUT).unwrap() {
                Event::Deleted(id) => assert_eq!(id, format!("thread-{index}")),
                Event::Failed {
                    operation: Operation::Read,
                    ..
                } => assert_eq!(index % 2, 1),
                event => panic!("unexpected event: {event:?}"),
            }
        }
    }

    #[test]
    fn stop_returns_at_once_and_a_new_service_starts_while_codex_stops() {
        let directory = tempfile::tempdir().unwrap();
        // The descendant keeps the output open after Codex exits, so only
        // the kill of the process group ends the old worker.
        let setup = r#"sleep 30 &
printf '%s
' "$!" > "$0.pid""#;
        let old = directory.path().join("old-codex");
        let new = directory.path().join("new-codex");
        fake_codex(&old, setup, "");
        fake_codex(&new, "", "");
        let mut service = launch_ready(&old);
        let started = Instant::now();
        service.stop();
        drop(service);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "{:?}",
            started.elapsed()
        );

        let replacement = launch_ready(&new);
        replacement.send(Command::Read(String::new())).unwrap();
        assert!(matches!(
            replacement.events.recv_timeout(EVENT_TIMEOUT).unwrap(),
            Event::Failed { .. }
        ));

        Service::wait_for_background_stops(Instant::now() + Duration::from_secs(5));
        assert_stopped(&old.with_extension("pid"));
    }

    #[test]
    fn stop_ends_a_request_in_flight() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        fake_codex(
            &executable,
            "",
            r#"    *'"method":"thread/read"'*)
      sleep 30 &
      printf '%s
' "$!" > "$0.pid"
      wait ;;"#,
        );
        let mut service = launch_ready(&executable);
        service.send(Command::Read("thread-1".into())).unwrap();
        let pid_file = executable.with_extension("pid");
        let deadline = Instant::now() + EVENT_TIMEOUT;
        while !fs::read_to_string(&pid_file).is_ok_and(|pid| pid.ends_with('\n')) {
            assert!(
                Instant::now() < deadline,
                "Codex did not receive the request"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let started = Instant::now();
        service.shutdown_and_wait(Duration::from_secs(3)).unwrap();
        // A wait for the worker alone kills Codex only after 2.25 seconds.
        // The limit leaves time for a slow CI runner.
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_stopped(&pid_file);
    }

    #[test]
    fn service_launches_off_window_thread_and_routes_commands() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        crate::assistant::write_test_executable(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$0.log"
  id=$(printf '%s' "$line" | sed -nE 's/.*"id":([0-9]+).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"id":%s,"result":{}}\n' "$id" ;;
    *'"method":"account/read"'*) printf '{"id":%s,"result":{"account":null,"requiresOpenaiAuth":true}}\n' "$id" ;;
    *'"method":"model/list"'*) printf '{"id":%s,"result":{"data":[],"nextCursor":null}}\n' "$id" ;;
    *'"method":"thread/start"'*) printf '{"id":%s,"result":{"thread":{"id":"thread-1","name":null,"updatedAt":1,"turns":[]}}}\n' "$id" ;;
    *'"method":"thread/items/list"'*) printf '{"id":%s,"result":{"data":[{"turnId":"turn-older","item":{"type":"userMessage","content":[{"type":"text","text":"Older"}]}}],"nextCursor":null}}\n' "$id" ;;
    *'"method":"thread/delete"'*) printf '{"id":%s,"result":{}}\n' "$id" ;;
  esac
done
"#,
        );
        let mut service = Service::launch(executable.clone(), Arc::new(|| {})).unwrap();
        assert!(matches!(
            service.events.recv_timeout(EVENT_TIMEOUT).unwrap(),
            Event::Ready(_)
        ));
        service
            .send(Command::Create(super::super::tools::definitions()))
            .unwrap();
        assert!(
            matches!(service.events.recv_timeout(EVENT_TIMEOUT).unwrap(), Event::Created(Conversation { id, .. }) if id == "thread-1")
        );
        service
            .send(Command::ReadOlder {
                thread_id: "thread-1".into(),
                cursor: "older".into(),
            })
            .unwrap();
        assert!(
            matches!(service.events.recv_timeout(EVENT_TIMEOUT).unwrap(), Event::HistoryPage(ConversationPage { thread_id, turns, older_cursor: None }) if thread_id == "thread-1" && turns.len() == 1)
        );
        service
            .shutdown_and_delete(vec!["thread-1".into()], Duration::from_secs(3))
            .unwrap();
        let requests = fs::read_to_string(executable.with_extension("log")).unwrap();
        assert!(requests.contains("\"method\":\"thread/delete\""));
        assert!(
            service
                .shutdown_and_delete(vec!["thread-2".into()], Duration::from_millis(1))
                .is_err()
        );
    }

    #[test]
    fn only_notifications_that_the_ui_reads_reach_the_event_channel() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        crate::assistant::write_test_executable(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -nE 's/.*"id":([0-9]+).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"id":%s,"result":{}}\n' "$id" ;;
    *'"method":"account/read"'*) printf '{"id":%s,"result":{"account":null,"requiresOpenaiAuth":true}}\n' "$id" ;;
    *'"method":"model/list"'*)
      printf '%s\n' '{"method":"account/rateLimits/updated","params":{}}'
      printf '%s\n' '{"method":"account/login/completed","params":{"loginId":"login-1","success":true}}'
      printf '%s\n' '{"method":"mcpServer/startupStatus/updated","params":{}}'
      printf '%s\n' '{"method":"account/updated","params":{"authMode":"chatgpt"}}'
      printf '%s\n' '{"method":"thread/name/updated","params":{"threadId":"thread-1","threadName":null}}'
      printf '%s\n' '{"method":"thread/name/updated","params":{"threadId":"thread-1","threadName":"Orders"}}'
      printf '{"id":%s,"result":{"data":[],"nextCursor":null}}\n' "$id" ;;
  esac
done
"#,
        );
        let mut service = Service::launch(executable, Arc::new(|| {})).unwrap();
        assert!(matches!(
            service.events.recv_timeout(EVENT_TIMEOUT).unwrap(),
            Event::Ready(_)
        ));
        let mut methods = Vec::new();
        // The title change is the last message, so every earlier notification
        // was either sent or dropped when it arrives.
        loop {
            match service.events.recv_timeout(EVENT_TIMEOUT).unwrap() {
                Event::Harness(AssistantEvent::Other { method, .. }) => methods.push(method),
                Event::Harness(AssistantEvent::TitleChanged { title, .. }) => {
                    assert_eq!(title, "Orders");
                    break;
                }
                event => panic!("unexpected event: {event:?}"),
            }
        }
        assert_eq!(methods, UI_NOTIFICATIONS);
        service.shutdown_and_wait(Duration::from_secs(3)).unwrap();
    }

    #[test]
    fn demo_cleanup_reports_codex_delete_error() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        crate::assistant::write_test_executable(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -nE 's/.*"id":([0-9]+).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"id":%s,"result":{}}\n' "$id" ;;
    *'"method":"account/read"'*) printf '{"id":%s,"result":{"account":null,"requiresOpenaiAuth":true}}\n' "$id" ;;
    *'"method":"model/list"'*) printf '{"id":%s,"result":{"data":[],"nextCursor":null}}\n' "$id" ;;
    *'"method":"thread/delete"'*) printf '{"id":%s,"error":{"code":-1,"message":"synthetic refusal"}}\n' "$id" ;;
  esac
done
"#,
        );
        let mut service = Service::launch(executable, Arc::new(|| {})).unwrap();
        assert!(matches!(
            service.events.recv_timeout(EVENT_TIMEOUT).unwrap(),
            Event::Ready(_)
        ));
        let error = service
            .shutdown_and_delete(vec!["thread-1".into()], Duration::from_secs(3))
            .unwrap_err();
        assert!(error.to_string().contains("synthetic refusal"));
    }

    #[test]
    fn shutdown_reaps_pipe_holding_descendants_after_leader_exits() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        crate::assistant::write_test_executable(
            &executable,
            r#"#!/bin/sh
sleep 30 &
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -nE 's/.*"id":([0-9]+).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"id":%s,"result":{}}\n' "$id" ;;
    *'"method":"account/read"'*) printf '{"id":%s,"result":{"account":null,"requiresOpenaiAuth":true}}\n' "$id" ;;
    *'"method":"model/list"'*) printf '{"id":%s,"result":{"data":[],"nextCursor":null}}\n' "$id" ;;
  esac
done
exit 0
"#,
        );
        let mut service = Service::launch(executable, Arc::new(|| {})).unwrap();
        assert!(matches!(
            service.events.recv_timeout(EVENT_TIMEOUT).unwrap(),
            Event::Ready(_)
        ));
        service.shutdown_and_wait(Duration::from_secs(3)).unwrap();
    }
}
