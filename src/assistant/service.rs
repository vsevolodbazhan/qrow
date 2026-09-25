//! Background ownership of the Codex process and its synchronous protocol.
//!
//! Qrow's window thread sends commands and receives events. It never waits for
//! a Codex response, including while a query-tool call waits for the worker.

use super::{
    AssistantEvent, AssistantHarness, CodexHarness, Conversation, ConversationHistory,
    HarnessSnapshot, ToolCall, ToolDefinition, ToolResult, Turn, TurnRequest,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

const COMMAND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 1_024;
const EVENT_POLL: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub enum Command {
    Login,
    Refresh,
    Create(Vec<ToolDefinition>),
    Resume(String),
    Read(String),
    Rename {
        thread_id: String,
        title: String,
    },
    Delete(String),
    Start(TurnRequest),
    Steer {
        thread_id: String,
        turn_id: String,
        text: String,
    },
    Interrupt {
        thread_id: String,
        turn_id: String,
    },
    Answer {
        call: ToolCall,
        result: ToolResult,
    },
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Login,
    Refresh,
    Create,
    Resume,
    Read,
    Rename,
    Delete,
    Start,
    Steer,
    Interrupt,
    Answer,
}

impl Command {
    fn operation(&self) -> Operation {
        match self {
            Self::Login => Operation::Login,
            Self::Refresh => Operation::Refresh,
            Self::Create(_) => Operation::Create,
            Self::Resume(_) => Operation::Resume,
            Self::Read(_) => Operation::Read,
            Self::Rename { .. } => Operation::Rename,
            Self::Delete(_) => Operation::Delete,
            Self::Start(_) => Operation::Start,
            Self::Steer { .. } => Operation::Steer,
            Self::Interrupt { .. } => Operation::Interrupt,
            Self::Answer { .. } => Operation::Answer,
            Self::Shutdown => unreachable!(),
        }
    }

    fn identifier(&self) -> Option<String> {
        match self {
            Self::Resume(id) | Self::Read(id) | Self::Delete(id) => Some(id.clone()),
            Self::Rename { thread_id, .. }
            | Self::Steer { thread_id, .. }
            | Self::Interrupt { thread_id, .. } => Some(thread_id.clone()),
            Self::Start(request) => Some(request.thread_id.clone()),
            Self::Answer { call, .. } => Some(call.call_id.clone()),
            Self::Login | Self::Refresh | Self::Create(_) | Self::Shutdown => None,
        }
    }
}

#[derive(Debug)]
pub enum Event {
    Ready(HarnessSnapshot),
    Snapshot(HarnessSnapshot),
    LoginUrl(String),
    Created(Conversation),
    Resumed(Conversation),
    History(ConversationHistory),
    Renamed(String),
    Deleted(String),
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
    commands: mpsc::SyncSender<Command>,
    pub events: mpsc::Receiver<Event>,
    stopping: Arc<AtomicBool>,
    pid: Arc<AtomicU32>,
    done: mpsc::Receiver<()>,
    stopped: bool,
    cleanup_ids: Arc<Mutex<Vec<String>>>,
}

struct DoneSignal(mpsc::Sender<()>);
impl Drop for DoneSignal {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

impl Service {
    pub fn launch(executable: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<Self> {
        let (commands, command_rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (event_tx, events) = mpsc::sync_channel(EVENT_CAPACITY);
        let (done_tx, done) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let pid = Arc::new(AtomicU32::new(0));
        let cleanup_ids = Arc::new(Mutex::new(Vec::<String>::new()));
        let thread_stopping = Arc::clone(&stopping);
        let thread_pid = Arc::clone(&pid);
        let thread_cleanup_ids = Arc::clone(&cleanup_ids);
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
                let mut harness =
                    match CodexHarness::launch_with_pid(&executable, directory.path(), move |id| {
                        thread_pid.store(id, Ordering::Release);
                    }) {
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
                loop {
                    if thread_stopping.load(Ordering::Acquire) {
                        break;
                    }
                    match command_rx.try_recv() {
                        Ok(Command::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => break,
                        Ok(command) => {
                            let event = execute(&mut harness, command);
                            if !emit(event) {
                                break;
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                    match harness.next_event(EVENT_POLL) {
                        Ok(Some(event)) => {
                            if !emit(Event::Harness(event)) {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            emit(Event::Disconnected(error.to_string()));
                            break;
                        }
                    }
                }
                if let Ok(mut ids) = thread_cleanup_ids.lock() {
                    for id in ids.drain(..) {
                        let _ = harness.delete_conversation(&id);
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
            pid,
            done,
            stopped: false,
            cleanup_ids,
        })
    }

    pub fn send(&self, command: Command) -> Result<(), &'static str> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => "Assistant is busy; try again.",
                mpsc::TrySendError::Disconnected(_) => "Assistant is disconnected.",
            })
    }

    pub fn shutdown_and_wait(&mut self, timeout: Duration) -> std::io::Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        self.stopping.store(true, Ordering::Release);
        let _ = self.commands.try_send(Command::Shutdown);
        if self.done.recv_timeout(timeout - timeout / 4).is_ok() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            let pid = self.pid.load(Ordering::Acquire);
            if pid > 1 {
                let _ = super::codex::terminate_process_group(pid);
            }
        }
        #[cfg(windows)]
        {
            let pid = self.pid.load(Ordering::Acquire);
            if pid > 0 {
                let _ = super::codex::terminate_process_tree(pid);
            }
        }
        self.done
            .recv_timeout(timeout / 4)
            .map_err(|_| std::io::Error::other("Codex assistant did not stop"))
    }

    pub fn shutdown_and_delete(
        &mut self,
        ids: Vec<String>,
        timeout: Duration,
    ) -> std::io::Result<()> {
        if let Ok(mut cleanup) = self.cleanup_ids.lock() {
            *cleanup = ids;
        }
        self.shutdown_and_wait(timeout)
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.shutdown_and_wait(Duration::from_secs(2));
    }
}

fn execute(harness: &mut dyn AssistantHarness, command: Command) -> Event {
    let operation = command.operation();
    let id = command.identifier();
    let result = match command {
        Command::Login => harness.begin_login().map(Event::LoginUrl),
        Command::Refresh => harness.snapshot().map(Event::Snapshot),
        Command::Create(tools) => harness.create_conversation(&tools).map(Event::Created),
        Command::Resume(id) => harness.resume_conversation(&id).map(Event::Resumed),
        Command::Read(id) => harness.read_conversation(&id).map(Event::History),
        Command::Rename { thread_id, title } => harness
            .rename_conversation(&thread_id, &title)
            .map(|()| Event::Renamed(thread_id)),
        Command::Delete(id) => harness
            .delete_conversation(&id)
            .map(|()| Event::Deleted(id)),
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
        } => harness
            .steer_turn(&thread_id, &turn_id, &text)
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
        Command::Shutdown => unreachable!(),
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
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn service_launches_off_window_thread_and_routes_commands() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        fs::write(&executable, r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$0.log"
  id=$(printf '%s' "$line" | sed -nE 's/.*"id":([0-9]+).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"id":%s,"result":{}}\n' "$id" ;;
    *'"method":"account/read"'*) printf '{"id":%s,"result":{"account":null,"requiresOpenaiAuth":true}}\n' "$id" ;;
    *'"method":"model/list"'*) printf '{"id":%s,"result":{"data":[],"nextCursor":null}}\n' "$id" ;;
    *'"method":"thread/start"'*) printf '{"id":%s,"result":{"thread":{"id":"thread-1","name":null,"updatedAt":1,"turns":[]}}}\n' "$id" ;;
    *'"method":"thread/delete"'*) printf '{"id":%s,"result":{}}\n' "$id" ;;
  esac
done
"#).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let mut service = Service::launch(executable.clone(), Arc::new(|| {})).unwrap();
        assert!(matches!(
            service.events.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::Ready(_)
        ));
        service
            .send(Command::Create(super::super::tools::definitions()))
            .unwrap();
        assert!(
            matches!(service.events.recv_timeout(Duration::from_secs(5)).unwrap(), Event::Created(Conversation { id, .. }) if id == "thread-1")
        );
        service
            .shutdown_and_delete(vec!["thread-1".into()], Duration::from_secs(3))
            .unwrap();
        let requests = fs::read_to_string(executable.with_extension("log")).unwrap();
        assert!(requests.contains("\"method\":\"thread/delete\""));
    }
}
