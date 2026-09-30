//! The single input channel of the assistant worker.
//!
//! Window commands and Codex output arrive on one channel, so the worker
//! sleeps until one of them has work. Each source has its own bound: Codex
//! output cannot use the space for window commands.

use super::service::Command;
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    time::Instant,
};

pub(crate) enum Message {
    /// A command is much larger than the other messages.
    Command(Box<Command>),
    /// A protocol message from Codex, or the reason that Codex output is unreadable.
    Codex(Result<Value, String>),
    /// The Codex output reader stopped and sends nothing more.
    CodexClosed,
    /// The window stops the worker.
    Stop,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SendError {
    /// The messages of this source that wait for the worker fill its bound.
    Full,
    /// The worker stopped.
    Disconnected,
}

/// Counts the messages of one source from send until the worker takes them.
#[derive(Clone)]
struct Bound {
    queued: Arc<AtomicUsize>,
    capacity: usize,
}

impl Bound {
    fn new(capacity: usize) -> Self {
        Self {
            queued: Arc::new(AtomicUsize::new(0)),
            capacity,
        }
    }

    fn send(&self, sender: &mpsc::Sender<Message>, message: Message) -> Result<(), SendError> {
        if self.queued.fetch_add(1, Ordering::AcqRel) >= self.capacity {
            self.release();
            return Err(SendError::Full);
        }
        sender.send(message).map_err(|_| {
            self.release();
            SendError::Disconnected
        })
    }

    fn release(&self) {
        self.queued.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Sends window commands to the worker.
pub(crate) struct CommandSender {
    sender: mpsc::Sender<Message>,
    bound: Bound,
}

impl CommandSender {
    pub(crate) fn send(&self, command: Command) -> Result<(), SendError> {
        self.bound
            .send(&self.sender, Message::Command(Box::new(command)))
    }

    /// Asks the worker to stop. A full command bound does not delay this message.
    pub(crate) fn stop(&self) {
        let _ = self.sender.send(Message::Stop);
    }
}

/// Sends Codex output to the worker. When it is dropped, the worker receives
/// `CodexClosed`.
pub(crate) struct OutputSender {
    sender: mpsc::Sender<Message>,
    bound: Bound,
}

impl OutputSender {
    pub(crate) fn send(&self, output: Result<Value, String>) -> Result<(), SendError> {
        self.bound.send(&self.sender, Message::Codex(output))
    }
}

impl Drop for OutputSender {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::CodexClosed);
    }
}

pub(crate) struct Inbox {
    receiver: mpsc::Receiver<Message>,
    /// Keeps the channel open, so that a receive waits and does not fail.
    sender: mpsc::Sender<Message>,
    commands: Bound,
    output: Bound,
    /// Messages that arrived while a request waited for its response, in arrival order.
    held: VecDeque<Message>,
    held_output: usize,
    output_closed: bool,
    stopped: bool,
}

impl Inbox {
    /// At most `command_capacity` commands wait for the worker. A command waits
    /// until `next` returns it, also while the inbox holds it.
    pub(crate) fn channel(command_capacity: usize) -> (CommandSender, Self) {
        let (sender, receiver) = mpsc::channel();
        let commands = Bound::new(command_capacity);
        let command_sender = CommandSender {
            sender: sender.clone(),
            bound: commands.clone(),
        };
        let inbox = Self {
            receiver,
            sender,
            commands,
            output: Bound::new(0),
            held: VecDeque::new(),
            held_output: 0,
            output_closed: false,
            stopped: false,
        };
        (command_sender, inbox)
    }

    /// Makes the sender for the Codex output reader. At most `capacity` Codex
    /// messages wait until the inbox receives them.
    pub(crate) fn output_sender(&mut self, capacity: usize) -> OutputSender {
        self.output = Bound::new(capacity);
        OutputSender {
            sender: self.sender.clone(),
            bound: self.output.clone(),
        }
    }

    /// Waits for a new message and skips the held messages. Returns `None` at
    /// the deadline. After `CodexClosed`, it returns `CodexClosed` again.
    pub(crate) fn receive(&mut self, deadline: Option<Instant>) -> Option<Message> {
        if self.output_closed {
            return Some(Message::CodexClosed);
        }
        let message = match deadline {
            None => self.receiver.recv().ok()?,
            Some(deadline) => match self
                .receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
            },
        };
        match &message {
            Message::Codex(_) => self.output.release(),
            Message::CodexClosed => self.output_closed = true,
            Message::Stop => self.stopped = true,
            Message::Command(_) => {}
        }
        Some(message)
    }

    /// Keeps a message for `next`.
    pub(crate) fn hold(&mut self, message: Message) {
        if matches!(message, Message::Codex(_)) {
            self.held_output += 1;
        }
        self.held.push_back(message);
    }

    /// The number of held Codex messages.
    pub(crate) fn held_output(&self) -> usize {
        self.held_output
    }

    /// Takes the next message for the worker loop: `Stop` after a stop
    /// request, then the held messages, then new messages.
    pub(crate) fn next(&mut self, deadline: Option<Instant>) -> Option<Message> {
        if self.stopped {
            return Some(Message::Stop);
        }
        let message = match self.held.pop_front() {
            Some(message) => {
                if matches!(message, Message::Codex(_)) {
                    self.held_output -= 1;
                }
                message
            }
            None => self.receive(deadline)?,
        };
        if matches!(message, Message::Command(_)) {
            self.commands.release();
        }
        Some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn command(id: &str) -> Command {
        Command::Read(id.into())
    }

    fn read_id(message: Option<Message>) -> String {
        match message {
            Some(Message::Command(command)) => match *command {
                Command::Read(id) => id,
                _ => panic!("expected a read command"),
            },
            _ => panic!("expected a read command"),
        }
    }

    #[test]
    fn codex_output_does_not_use_the_command_bound() {
        let (commands, mut inbox) = Inbox::channel(2);
        let output = inbox.output_sender(3);
        for index in 0..3 {
            output.send(Ok(json!({ "index": index }))).unwrap();
        }
        assert_eq!(output.send(Ok(json!({}))), Err(SendError::Full));
        commands.send(command("a")).unwrap();
        commands.send(command("b")).unwrap();
        assert_eq!(commands.send(command("c")), Err(SendError::Full));

        // Codex output frees its place when the inbox receives it.
        assert!(matches!(inbox.next(None), Some(Message::Codex(Ok(_)))));
        output.send(Ok(json!({}))).unwrap();
        assert_eq!(output.send(Ok(json!({}))), Err(SendError::Full));
    }

    #[test]
    fn held_commands_keep_their_place_until_the_worker_takes_them() {
        let (commands, mut inbox) = Inbox::channel(2);
        let output = inbox.output_sender(4);
        commands.send(command("a")).unwrap();
        output.send(Ok(json!({ "method": "notice" }))).unwrap();
        commands.send(command("b")).unwrap();
        // A request receives all three messages and holds them.
        for _ in 0..3 {
            let message = inbox.receive(None).unwrap();
            inbox.hold(message);
        }
        assert_eq!(inbox.held_output(), 1);
        assert_eq!(commands.send(command("c")), Err(SendError::Full));

        assert_eq!(read_id(inbox.next(None)), "a");
        assert!(matches!(inbox.next(None), Some(Message::Codex(Ok(_)))));
        assert_eq!(inbox.held_output(), 0);
        commands.send(command("c")).unwrap();
        assert_eq!(read_id(inbox.next(None)), "b");
        assert_eq!(read_id(inbox.next(None)), "c");
        assert!(
            inbox
                .next(Some(Instant::now() + Duration::from_millis(1)))
                .is_none()
        );
    }

    #[test]
    fn stop_passes_a_full_bound_and_stays_until_the_end() {
        let (commands, mut inbox) = Inbox::channel(1);
        commands.send(command("a")).unwrap();
        commands.stop();
        let message = inbox.receive(None).unwrap();
        inbox.hold(message);
        assert!(matches!(inbox.receive(None), Some(Message::Stop)));
        // The held command does not run after a stop request.
        assert!(matches!(inbox.next(None), Some(Message::Stop)));
        assert!(matches!(inbox.next(None), Some(Message::Stop)));
    }

    #[test]
    fn closed_output_stays_closed() {
        let (_commands, mut inbox) = Inbox::channel(1);
        let output = inbox.output_sender(1);
        output.send(Ok(json!({}))).unwrap();
        drop(output);
        assert!(matches!(inbox.receive(None), Some(Message::Codex(Ok(_)))));
        assert!(matches!(inbox.receive(None), Some(Message::CodexClosed)));
        assert!(matches!(inbox.next(None), Some(Message::CodexClosed)));
    }
}
