//! The Claude Code harness. Qrow starts the user's installed `claude`
//! program and talks to it over the stream-json protocol of `claude -p`.
//!
//! Each conversation has its own `claude` process, because one process runs
//! one session. A small control process without a session gives the models,
//! the account, and conversation titles. Qrow tools reach Claude Code as an
//! SDK MCP server: Claude Code sends each MCP message to Qrow as a control
//! request, and Qrow answers it on the same stream.
//!
//! Qrow never starts a Claude sign-in. The user signs in to Claude Code with
//! Anthropic's own flow, and Claude Code keeps the credentials.

use super::{
    AccountKind, AccountStatus, AssistantEvent, BASE_INSTRUCTIONS, Conversation,
    ConversationHistory, ConversationPage, HarnessFeatures, HarnessSnapshot, LoginStart,
    MAX_CONTEXT_BYTES, MAX_MESSAGE_BYTES, Model, ReasoningEffort, TitleRequest, ToolCall,
    ToolDefinition, ToolResult, Turn, TurnRequest, WORKSPACE_CONTEXT_SEPARATOR,
    codex::{
        Input, LaunchChild, WriteCommand, clean_title, kill_process_tree, read_stderr_tail,
        write_protocol_stream,
    },
    inbox::{Inbox, Message, ProcessSender, SendError},
    service::ProcessIds,
};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[cfg(all(test, unix))]
mod tests;

/// The oldest Claude Code version that has every protocol feature that Qrow
/// uses, for example `--permission-prompts`.
pub const MIN_VERSION: (u32, u32, u32) = (2, 1, 259);
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PENDING_MESSAGES: usize = 1_024;
const MAX_STDERR_BYTES: usize = 16 * 1024;
const MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;
const MAX_TITLE_MESSAGES: usize = 6;
const MAX_TITLE_MESSAGE_CHARS: usize = 2_000;
const MAX_TITLE_JOBS: usize = 8;
const TITLE_TIMEOUT: Duration = Duration::from_secs(60);
/// The conversation processes that stay alive. Each process uses about
/// 200 MB, so Qrow stops the idle process that it used least recently.
const MAX_LIVE_SESSIONS: usize = 3;
/// A conversation process without a turn for this time stops. The next
/// message starts it again.
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_millis(200);
/// The name of the SDK MCP server that gives the Qrow tools.
const MCP_SERVER: &str = "qrow";
/// The reasoning level that leaves the choice to Claude Code.
pub const DEFAULT_EFFORT: &str = "default";
#[cfg(not(test))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(test)]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// One `claude` process and its stream threads.
struct Process {
    child: Child,
    source: u64,
    writer: Option<SyncSender<WriteCommand>>,
    writer_thread: Option<JoinHandle<()>>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    stderr_tail: Arc<Mutex<VecDeque<u8>>>,
    /// Tells the output reader to stop waiting for room in a full inbox.
    closing: Arc<AtomicBool>,
    next_request: u64,
}

/// The process of one conversation.
struct Session {
    process: Process,
    model: Option<String>,
    effort: Option<String>,
    /// The turn that runs: the UUID of the user message that started it.
    turn: Option<String>,
    interrupted: bool,
    /// The text of the reply message that streams.
    streaming: Option<String>,
    /// Claude Code saves a session only after its first message, so Qrow
    /// stops only sessions that have one.
    started: bool,
    last_used: Instant,
}

/// A title request on the control process.
struct TitleJob {
    request_id: String,
    target: String,
    /// A rename or a delete of `target` cancels the job.
    cancelled: bool,
    started: Instant,
}

pub struct ClaudeHarness {
    executable: PathBuf,
    cwd: PathBuf,
    inbox: Inbox,
    control: Option<Process>,
    sessions: BTreeMap<String, Session>,
    tools: Vec<ToolDefinition>,
    next_source: u64,
    pids: ProcessIds,
    titles: Vec<TitleJob>,
    /// Title request IDs stay unique across control processes.
    next_title: u64,
    request_timeout: Duration,
    title_timeout: Duration,
    session_idle_timeout: Duration,
    /// The configuration folder of Claude Code, where it saves sessions.
    config_dir: Option<PathBuf>,
}

impl ClaudeHarness {
    /// Checks the version of Claude Code and starts the control process.
    pub(crate) fn launch_with_inbox(
        executable: &Path,
        cwd: &Path,
        mut inbox: Inbox,
        pids: ProcessIds,
    ) -> Result<Self> {
        anyhow::ensure!(cwd.is_dir(), "Claude Code working directory does not exist");
        check_version(executable)?;
        inbox.set_process_capacity(MAX_PENDING_MESSAGES);
        let config_dir = default_config_dir();
        let mut harness = Self {
            executable: executable.to_path_buf(),
            cwd: cwd.to_path_buf(),
            inbox,
            control: None,
            sessions: BTreeMap::new(),
            tools: super::tools::definitions(),
            next_source: 1,
            pids,
            titles: Vec::new(),
            next_title: 1,
            request_timeout: REQUEST_TIMEOUT,
            title_timeout: TITLE_TIMEOUT,
            session_idle_timeout: SESSION_IDLE_TIMEOUT,
            config_dir,
        };
        harness.start_control()?;
        Ok(harness)
    }

    /// The arguments of every Claude Code process: the stream-json protocol,
    /// no built-in tools, and no user settings, hooks, plugins, or MCP
    /// servers.
    fn common_arguments() -> Vec<OsString> {
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--tools",
            "",
            "--permission-mode",
            "dontAsk",
            "--permission-prompts",
            "none",
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    fn spawn(&mut self, arguments: Vec<OsString>) -> Result<Process> {
        let mut command = Command::new(&self.executable);
        command
            .args(arguments)
            .current_dir(&self.cwd)
            // The Qrow tools load at the start, so Claude does not search for them.
            .env("ENABLE_TOOL_SEARCH", "false")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn().context("Could not start Claude Code")?;
        let mut child = LaunchChild::new(child);
        let pid = child.child_mut().id();
        self.pids.add(pid);
        let stdin = child
            .child_mut()
            .stdin
            .take()
            .context("Claude Code did not provide standard input")?;
        let stdout = child
            .child_mut()
            .stdout
            .take()
            .context("Claude Code did not provide standard output")?;
        let stderr = child
            .child_mut()
            .stderr
            .take()
            .context("Claude Code did not provide standard error")?;
        let source = self.next_source;
        self.next_source += 1;
        let sender = self.inbox.process_sender(source);
        let closing = Arc::new(AtomicBool::new(false));
        let reader_closing = Arc::clone(&closing);
        let stdout_thread = thread::Builder::new()
            .name("qrow-claude-stdout".into())
            .spawn(move || read_lines(BufReader::new(stdout), &sender, &reader_closing))
            .context("Could not start Claude Code output reader")?;
        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_STDERR_BYTES)));
        let stderr_thread = {
            let tail = Arc::clone(&stderr_tail);
            thread::Builder::new()
                .name("qrow-claude-stderr".into())
                .spawn(move || read_stderr_tail(stderr, &tail))
                .context("Could not start Claude Code error reader")?
        };
        let (writer, write_commands) = mpsc::sync_channel(1);
        let writer_thread = thread::Builder::new()
            .name("qrow-claude-stdin".into())
            .spawn(move || write_protocol_stream(stdin, &write_commands))
            .context("Could not start Claude Code input writer")?;
        Ok(Process {
            child: child.into_inner(),
            source,
            writer: Some(writer),
            writer_thread: Some(writer_thread),
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            stderr_tail,
            closing,
            next_request: 1,
        })
    }

    /// Starts the control process and reads the models and the account.
    fn start_control(&mut self) -> Result<Value> {
        if let Some(process) = self.control.take() {
            self.stop_process(process);
            // The title requests of the old process get no answer. Each one
            // fails at the next input.
            self.expire_all_titles();
        }
        let mut arguments = Self::common_arguments();
        arguments.push("--no-session-persistence".into());
        let mut process = self.spawn(arguments)?;
        let response = self.request(
            &mut process,
            json!({ "subtype": "initialize", "hooks": null }),
        );
        match response {
            Ok(response) => {
                self.control = Some(process);
                Ok(response)
            }
            Err(error) => {
                self.stop_process(process);
                Err(error)
            }
        }
    }

    pub fn snapshot(&mut self) -> Result<HarnessSnapshot> {
        let response = self.start_control()?;
        Ok(snapshot_from_initialize(&response))
    }

    pub fn begin_login(&mut self) -> Result<LoginStart> {
        bail!("Sign in to Claude Code in Terminal with `claude auth login`")
    }

    pub fn cancel_login(&mut self, _login_id: &str) -> Result<()> {
        Ok(())
    }

    fn session_arguments(
        &self,
        flag: &str,
        session_id: &str,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Vec<OsString> {
        let mut arguments = Self::common_arguments();
        let mcp = json!({ "mcpServers": { MCP_SERVER: { "type": "sdk", "name": MCP_SERVER } } });
        arguments.extend(
            [
                "--include-partial-messages",
                "--mcp-config",
                &mcp.to_string(),
                "--allowedTools",
                &format!("mcp__{MCP_SERVER}__*"),
                "--system-prompt",
                BASE_INSTRUCTIONS,
                flag,
                session_id,
            ]
            .map(OsString::from),
        );
        if let Some(model) = model {
            arguments.extend(["--model".into(), model.into()]);
        }
        if let Some(effort) = effort.filter(|effort| *effort != DEFAULT_EFFORT) {
            arguments.extend(["--effort".into(), effort.into()]);
        }
        arguments
    }

    /// Starts the process of a conversation: a new session, or one that
    /// Claude Code saved.
    fn start_session(
        &mut self,
        session_id: &str,
        resume: bool,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<()> {
        self.stop_idle_sessions(MAX_LIVE_SESSIONS.saturating_sub(1), Instant::now());
        let flag = if resume { "--resume" } else { "--session-id" };
        let arguments = self.session_arguments(flag, session_id, model, effort);
        let mut process = self.spawn(arguments)?;
        let initialized = self.request(
            &mut process,
            json!({ "subtype": "initialize", "hooks": null, "sdkMcpServers": [MCP_SERVER] }),
        );
        if let Err(error) = initialized {
            self.stop_process(process);
            return Err(if resume {
                error.context("Claude Code could not open this conversation")
            } else {
                error
            });
        }
        self.sessions.insert(
            session_id.to_owned(),
            Session {
                process,
                model: model.map(str::to_owned),
                effort: effort.map(str::to_owned),
                turn: None,
                interrupted: false,
                streaming: None,
                started: resume,
                last_used: Instant::now(),
            },
        );
        Ok(())
    }

    /// Stops idle conversation processes until at most `keep` stay, least
    /// recently used first, and the processes without a turn for the idle
    /// time.
    fn stop_idle_sessions(&mut self, keep: usize, now: Instant) {
        loop {
            let idle = |session: &Session| session.turn.is_none() && session.started;
            let expired = self
                .sessions
                .iter()
                .find(|(_, session)| {
                    idle(session)
                        && now.saturating_duration_since(session.last_used)
                            >= self.session_idle_timeout
                })
                .map(|(id, _)| id.clone());
            let surplus = (self.sessions.len() > keep)
                .then(|| {
                    self.sessions
                        .iter()
                        .filter(|(_, session)| idle(session))
                        .min_by_key(|(_, session)| session.last_used)
                        .map(|(id, _)| id.clone())
                })
                .flatten();
            let Some(id) = expired.or(surplus) else {
                return;
            };
            if let Some(session) = self.sessions.remove(&id) {
                self.stop_process(session.process);
            }
        }
    }

    fn ensure_identifier(id: &str) -> Result<()> {
        Uuid::parse_str(id)
            .map(|_| ())
            .map_err(|_| anyhow!("Claude Code session identifier is invalid"))
    }

    pub fn create_conversation(&mut self, tools: &[ToolDefinition]) -> Result<Conversation> {
        anyhow::ensure!(!tools.is_empty(), "Assistant tools are unavailable");
        self.tools = tools.to_vec();
        let id = Uuid::new_v4().to_string();
        self.start_session(&id, false, None, None)?;
        Ok(Conversation {
            id,
            title: None,
            updated_at: unix_now(),
        })
    }

    pub fn resume_conversation(&mut self, thread_id: &str) -> Result<Conversation> {
        Self::ensure_identifier(thread_id)?;
        if !self.sessions.contains_key(thread_id) {
            self.start_session(thread_id, true, None, None)?;
        }
        Ok(Conversation {
            id: thread_id.to_owned(),
            title: None,
            updated_at: unix_now(),
        })
    }

    pub fn read_conversation(&mut self, _thread_id: &str) -> Result<ConversationHistory> {
        bail!("Claude Code does not give conversation history; Qrow shows its saved copy")
    }

    pub fn read_older_conversation(
        &mut self,
        _thread_id: &str,
        _cursor: &str,
    ) -> Result<ConversationPage> {
        bail!("Claude Code does not give conversation history; Qrow shows its saved copy")
    }

    /// Qrow keeps the title. The title of a running conversation also goes
    /// to Claude Code, for its own list of sessions.
    pub fn rename_conversation(&mut self, thread_id: &str, title: &str) -> Result<()> {
        Self::ensure_identifier(thread_id)?;
        anyhow::ensure!(
            !title.trim().is_empty() && title.chars().count() <= 120,
            "Conversation title is invalid"
        );
        self.cancel_title_jobs(thread_id);
        if let Some(mut session) = self.sessions.remove(thread_id) {
            let result = self.request(
                &mut session.process,
                json!({ "subtype": "rename_session", "title": title.trim(), "source": "host" }),
            );
            self.sessions.insert(thread_id.to_owned(), session);
            // An older Claude Code without the request still keeps the Qrow title.
            let _ = result;
        }
        Ok(())
    }

    pub fn generate_title(&mut self, request: TitleRequest) -> Result<()> {
        Self::ensure_identifier(&request.thread_id)?;
        if self
            .titles
            .iter()
            .any(|job| job.target == request.thread_id && !job.cancelled)
        {
            return Ok(());
        }
        anyhow::ensure!(
            self.titles.iter().filter(|job| !job.cancelled).count() < MAX_TITLE_JOBS,
            "Too many conversation titles are generating"
        );
        let description =
            title_description(&request.messages).context("Conversation has no text for a title")?;
        if self.control.is_none() {
            self.start_control()?;
        }
        let mut control = self.control.take().context("Claude Code is not running")?;
        let request_id = format!("qrow-title-{}", self.next_title);
        self.next_title += 1;
        let written = self.write(
            &mut control,
            &json!({
                "type": "control_request",
                "request_id": request_id,
                "request": {
                    "subtype": "generate_session_title",
                    "description": description,
                    "persist": false,
                },
            }),
        );
        self.control = Some(control);
        written?;
        self.titles.push(TitleJob {
            request_id,
            target: request.thread_id,
            cancelled: false,
            started: Instant::now(),
        });
        Ok(())
    }

    fn cancel_title_jobs(&mut self, target: &str) {
        for job in &mut self.titles {
            if job.target == target {
                job.cancelled = true;
            }
        }
    }

    /// Stops the process of a conversation and deletes the session files
    /// that Claude Code saved for it. A missing file is not an error.
    pub fn delete_conversation(&mut self, thread_id: &str) -> Result<()> {
        Self::ensure_identifier(thread_id)?;
        self.cancel_title_jobs(thread_id);
        if let Some(session) = self.sessions.remove(thread_id) {
            self.stop_process(session.process);
        }
        match &self.config_dir {
            Some(config) => delete_session_files(config, thread_id),
            None => Ok(()),
        }
    }

    pub fn start_turn(&mut self, request: TurnRequest) -> Result<Turn> {
        Self::ensure_identifier(&request.thread_id)?;
        anyhow::ensure!(!request.text.trim().is_empty(), "Message is empty");
        anyhow::ensure!(
            request.text.len() <= MAX_MESSAGE_BYTES,
            "Message is too large"
        );
        let context = serde_json::to_string(&request.context)?;
        anyhow::ensure!(
            context.len() <= MAX_CONTEXT_BYTES,
            "Workspace context is too large"
        );
        let model = request.model.as_deref();
        let effort = request.reasoning_effort.as_deref();
        // `max` is only a launch option, so the process starts again for it.
        // A session without a message is not saved yet, so it starts new.
        let mut saved = true;
        if let Some(session) = self.sessions.get(&request.thread_id)
            && effort == Some("max")
            && session.effort.as_deref() != Some("max")
            && session.turn.is_none()
            && let Some(session) = self.sessions.remove(&request.thread_id)
        {
            saved = session.started;
            self.stop_process(session.process);
        }
        if !self.sessions.contains_key(&request.thread_id) {
            self.start_session(&request.thread_id, saved, model, effort)?;
        }
        let mut session = self
            .sessions
            .remove(&request.thread_id)
            .context("Claude Code conversation is not running")?;
        let result = self.send_turn(&mut session, model, effort, &request.text, &context);
        self.sessions.insert(request.thread_id.clone(), session);
        result
    }

    fn send_turn(
        &mut self,
        session: &mut Session,
        model: Option<&str>,
        effort: Option<&str>,
        text: &str,
        context: &str,
    ) -> Result<Turn> {
        anyhow::ensure!(
            session.turn.is_none(),
            "Claude Code is still working on the previous message"
        );
        if session.model.as_deref() != model {
            self.request(
                &mut session.process,
                json!({ "subtype": "set_model", "model": model }),
            )?;
            session.model = model.map(str::to_owned);
        }
        if session.effort.as_deref() != effort {
            let level = effort.filter(|effort| *effort != DEFAULT_EFFORT);
            self.request(
                &mut session.process,
                json!({ "subtype": "apply_flag_settings", "settings": { "effortLevel": level } }),
            )?;
            session.effort = effort.map(str::to_owned);
        }
        let turn = Uuid::new_v4().to_string();
        self.write(
            &mut session.process,
            &json!({
                "type": "user",
                "uuid": turn,
                "session_id": "",
                "parent_tool_use_id": null,
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": format!("{text}{WORKSPACE_CONTEXT_SEPARATOR}{context}"),
                    }],
                },
            }),
        )?;
        session.turn = Some(turn.clone());
        session.interrupted = false;
        session.streaming = None;
        session.started = true;
        session.last_used = Instant::now();
        Ok(Turn {
            id: turn,
            status: "inProgress".into(),
        })
    }

    pub fn steer_turn(
        &mut self,
        _thread_id: &str,
        _turn_id: &str,
        _text: &str,
        _context: &Value,
    ) -> Result<()> {
        bail!("Claude Code cannot add a message to a running turn")
    }

    pub fn interrupt_turn(&mut self, thread_id: &str, turn_id: &str) -> Result<()> {
        Self::ensure_identifier(thread_id)?;
        let mut session = self
            .sessions
            .remove(thread_id)
            .context("Claude Code conversation is not running")?;
        let result = if session.turn.as_deref() == Some(turn_id) {
            session.interrupted = true;
            self.request(&mut session.process, json!({ "subtype": "interrupt" }))
                .map(|_| ())
        } else {
            Ok(())
        };
        self.sessions.insert(thread_id.to_owned(), session);
        result
    }

    pub fn answer_tool_call(&mut self, call: &ToolCall, result: ToolResult) -> Result<()> {
        let text = serde_json::to_string(&result.content)?;
        anyhow::ensure!(
            text.len() <= MAX_TOOL_RESULT_BYTES,
            "Tool result is too large"
        );
        let source = call
            .request_id
            .get("source")
            .and_then(Value::as_u64)
            .context("Tool call has no Claude Code process")?;
        let request_id = call
            .request_id
            .get("request")
            .and_then(Value::as_str)
            .context("Tool call has no control request")?
            .to_owned();
        let message_id = call.request_id.get("id").cloned().unwrap_or(Value::Null);
        let response = json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": { "mcp_response": {
                    "jsonrpc": "2.0",
                    "id": message_id,
                    "result": {
                        "content": [{ "type": "text", "text": text }],
                        "isError": !result.success,
                    },
                } },
            },
        });
        let Some(thread) = self
            .sessions
            .iter()
            .find(|(_, session)| session.process.source == source)
            .map(|(id, _)| id.clone())
        else {
            bail!("The Claude Code conversation of this tool call stopped");
        };
        let mut session = self.sessions.remove(&thread).expect("session is present");
        let written = self.write(&mut session.process, &response);
        self.sessions.insert(thread, session);
        written
    }

    /// Stops all processes. Waits a short time for each one, then stops its
    /// process group.
    pub fn shutdown(&mut self) -> Result<()> {
        let mut processes: Vec<_> = std::mem::take(&mut self.sessions)
            .into_values()
            .map(|session| session.process)
            .collect();
        processes.extend(self.control.take());
        for process in &mut processes {
            process.writer.take();
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE_PERIOD;
        for mut process in processes {
            while Instant::now() < deadline && process.child.try_wait()?.is_none() {
                thread::sleep(Duration::from_millis(10));
            }
            self.finish_process(&mut process);
        }
        Ok(())
    }

    /// Deletes conversations before the worker stops, for the demo.
    pub(crate) fn delete_conversations_on_shutdown(
        &mut self,
        ids: impl IntoIterator<Item = String>,
    ) -> Result<()> {
        let mut first_error = None;
        for id in ids {
            if let Err(error) = self.delete_conversation(&id) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn stop_process(&mut self, mut process: Process) {
        process.writer.take();
        self.finish_process(&mut process);
    }

    /// Ends the process group of `process`, also when its leader already
    /// exited, because a descendant can keep the output open. The process
    /// stays registered until its threads end, so that a stop can still end
    /// the group.
    fn finish_process(&mut self, process: &mut Process) {
        // The worker joins the reader here and does not read the inbox, so
        // the reader must not wait for room in it.
        process.closing.store(true, Ordering::Release);
        let _ = kill_process_tree(&mut process.child);
        let _ = process.child.wait();
        for thread in [
            process.writer_thread.take(),
            process.stdout_thread.take(),
            process.stderr_thread.take(),
        ]
        .into_iter()
        .flatten()
        {
            let _ = thread.join();
        }
        self.pids.remove(process.child.id());
    }

    fn write(&self, process: &mut Process, value: &Value) -> Result<()> {
        let mut bytes =
            serde_json::to_vec(value).context("Could not encode a Claude Code message")?;
        bytes.push(b'\n');
        anyhow::ensure!(
            bytes.len() <= MAX_LINE_BYTES,
            "Claude Code message exceeded the protocol size limit"
        );
        let writer = process
            .writer
            .as_ref()
            .context("Claude Code input is closed")?;
        let (completed, completion) = mpsc::sync_channel(1);
        writer
            .try_send(WriteCommand { bytes, completed })
            .map_err(|error| match error {
                TrySendError::Full(_) => anyhow!("Claude Code input is busy"),
                TrySendError::Disconnected(_) => anyhow!("Claude Code input is closed"),
            })?;
        match completion.recv_timeout(self.request_timeout) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => bail!("{error}{}", diagnostic_suffix(process)),
            Err(RecvTimeoutError::Timeout) => bail!(
                "Claude Code did not accept a message within {} seconds",
                self.request_timeout.as_secs_f32()
            ),
            Err(RecvTimeoutError::Disconnected) => {
                bail!("Claude Code input closed{}", diagnostic_suffix(process))
            }
        }
    }

    /// Sends a control request to `process` and waits for its response.
    /// The MCP setup requests of the process get their answers meanwhile.
    /// Other messages wait in the inbox for `next_input`.
    fn request(&mut self, process: &mut Process, request: Value) -> Result<Value> {
        let request_id = format!("qrow-{}", process.next_request);
        process.next_request += 1;
        let subtype = request
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or("request")
            .to_owned();
        self.write(
            process,
            &json!({ "type": "control_request", "request_id": request_id, "request": request }),
        )?;
        let deadline = Instant::now() + self.request_timeout;
        loop {
            let message = match self.inbox.receive(Some(deadline)) {
                Some(Message::Output(source, Ok(message))) if source == process.source => message,
                Some(Message::Output(source, Err(error))) if source == process.source => {
                    bail!("{error}{}", diagnostic_suffix(process))
                }
                Some(Message::OutputClosed(source)) if source == process.source => {
                    let status = process.child.try_wait().ok().flatten();
                    bail!(
                        "Claude Code {}{}",
                        status.map_or("closed its output".into(), |status| format!(
                            "exited with {status}"
                        )),
                        diagnostic_suffix(process)
                    )
                }
                Some(Message::Stop) => bail!("Assistant is stopping"),
                Some(
                    message
                    @ (Message::Command(_) | Message::Output(..) | Message::OutputClosed(_)),
                ) => {
                    self.inbox.hold(message);
                    continue;
                }
                Some(Message::Codex(_) | Message::CodexClosed) => continue,
                None => bail!(
                    "Claude Code did not answer {subtype} within {} seconds{}",
                    self.request_timeout.as_secs_f32(),
                    diagnostic_suffix(process)
                ),
            };
            if message.get("type").and_then(Value::as_str) == Some("control_response")
                && message
                    .pointer("/response/request_id")
                    .and_then(Value::as_str)
                    == Some(request_id.as_str())
            {
                let response = message.get("response").cloned().unwrap_or(Value::Null);
                if response.get("subtype").and_then(Value::as_str) == Some("error") {
                    bail!(
                        "Claude Code refused {subtype}: {}",
                        response
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("no reason given")
                    );
                }
                return Ok(response.get("response").cloned().unwrap_or(Value::Null));
            }
            if self.answer_setup(process, &message)? {
                continue;
            }
            self.inbox
                .hold(Message::Output(process.source, Ok(message)));
        }
    }

    /// Answers the MCP setup messages and the control requests that Qrow
    /// does not allow. Returns false for other messages.
    fn answer_setup(&self, process: &mut Process, message: &Value) -> Result<bool> {
        if message.get("type").and_then(Value::as_str) != Some("control_request") {
            return Ok(false);
        }
        let request_id = message.get("request_id").cloned().unwrap_or(Value::Null);
        let request = message.get("request").unwrap_or(&Value::Null);
        if request.get("subtype").and_then(Value::as_str) != Some("mcp_message") {
            // Qrow answers no permission prompts, hooks, or dialogs.
            self.write(
                process,
                &json!({
                    "type": "control_response",
                    "response": { "subtype": "error", "request_id": request_id, "error": "Qrow does not allow this request" },
                }),
            )?;
            return Ok(true);
        }
        let mcp = request.get("message").unwrap_or(&Value::Null);
        let method = mcp.get("method").and_then(Value::as_str).unwrap_or("");
        let result = match method {
            "tools/call" => return Ok(false),
            "initialize" => json!({
                "protocolVersion": mcp.pointer("/params/protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": MCP_SERVER, "version": env!("CARGO_PKG_VERSION") },
            }),
            "tools/list" => json!({
                "tools": self.tools.iter().map(|tool| json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                })).collect::<Vec<_>>(),
            }),
            _ => json!({}),
        };
        let mcp_response = match mcp.get("id") {
            Some(id) if method.starts_with("notifications/") || method.is_empty() => {
                json!({ "jsonrpc": "2.0", "id": id, "result": {} })
            }
            Some(id) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            None => json!({ "jsonrpc": "2.0", "result": {} }),
        };
        self.write(
            process,
            &json!({
                "type": "control_response",
                "response": { "subtype": "success", "request_id": request_id, "response": { "mcp_response": mcp_response } },
            }),
        )?;
        Ok(true)
    }

    /// Waits for the next command, event, or stop request. Returns `None`
    /// at `deadline`. A conversation process that stops ends its turn with
    /// an error; the other conversations continue.
    pub(crate) fn next_input(&mut self, deadline: Option<Instant>) -> Result<Option<Input>> {
        loop {
            let now = Instant::now();
            if let Some(event) = self.expire_title_jobs(now) {
                return Ok(Some(Input::Event(event)));
            }
            self.stop_idle_sessions(MAX_LIVE_SESSIONS, now);
            let wait = [deadline, self.title_deadline(), self.idle_deadline()]
                .into_iter()
                .flatten()
                .min();
            let message = match self.inbox.next(wait) {
                None if deadline.is_none_or(|deadline| Instant::now() < deadline) => continue,
                None => return Ok(None),
                Some(Message::Command(command)) => return Ok(Some(Input::Command(*command))),
                Some(Message::Stop) => return Ok(Some(Input::Stop)),
                Some(Message::Codex(_) | Message::CodexClosed) => continue,
                Some(Message::Output(source, Ok(message))) => (source, message),
                Some(Message::Output(source, Err(error))) => {
                    if let Some(event) = self.process_failed(source, &error) {
                        return Ok(Some(Input::Event(event)));
                    }
                    continue;
                }
                Some(Message::OutputClosed(source)) => {
                    if let Some(event) = self.process_failed(source, "closed its output") {
                        return Ok(Some(Input::Event(event)));
                    }
                    continue;
                }
            };
            if let Some(event) = self.event(message.0, message.1)? {
                return Ok(Some(Input::Event(event)));
            }
        }
    }

    fn idle_deadline(&self) -> Option<Instant> {
        self.sessions
            .values()
            .filter(|session| session.turn.is_none() && session.started)
            .map(|session| session.last_used + self.session_idle_timeout)
            .min()
    }

    /// Makes every waiting title request expire now.
    fn expire_all_titles(&mut self) {
        let expired = Instant::now()
            .checked_sub(self.title_timeout)
            .unwrap_or_else(Instant::now);
        for job in &mut self.titles {
            job.started = expired;
        }
    }

    fn title_deadline(&self) -> Option<Instant> {
        self.titles
            .iter()
            .map(|job| job.started + self.title_timeout)
            .min()
    }

    fn expire_title_jobs(&mut self, now: Instant) -> Option<AssistantEvent> {
        while let Some(position) = self
            .titles
            .iter()
            .position(|job| now.saturating_duration_since(job.started) >= self.title_timeout)
        {
            let job = self.titles.remove(position);
            if !job.cancelled {
                return Some(AssistantEvent::TitleFailed {
                    thread_id: job.target,
                });
            }
        }
        None
    }

    /// Ends the process `source` after its output failed or closed.
    fn process_failed(&mut self, source: u64, reason: &str) -> Option<AssistantEvent> {
        if self
            .control
            .as_ref()
            .is_some_and(|control| control.source == source)
        {
            // The next title request starts the control process again. Each
            // waiting title request fails at the next input.
            let control = self.control.take()?;
            self.stop_process(control);
            self.expire_all_titles();
            return self.expire_title_jobs(Instant::now());
        }
        let thread = self
            .sessions
            .iter()
            .find(|(_, session)| session.process.source == source)
            .map(|(id, _)| id.clone())?;
        let mut session = self.sessions.remove(&thread)?;
        let suffix = diagnostic_suffix(&session.process);
        let turn = session.turn.take();
        self.stop_process(session.process);
        turn.map(|turn| AssistantEvent::TurnCompleted {
            thread_id: thread,
            turn: Turn {
                id: turn,
                status: "failed".into(),
            },
            error: Some(format!("Claude Code {reason}{suffix}")),
        })
    }

    /// Turns a message of the process `source` into an event.
    fn event(&mut self, source: u64, message: Value) -> Result<Option<AssistantEvent>> {
        if self
            .control
            .as_ref()
            .is_some_and(|control| control.source == source)
        {
            return Ok(self.control_event(&message));
        }
        let Some(thread) = self
            .sessions
            .iter()
            .find(|(_, session)| session.process.source == source)
            .map(|(id, _)| id.clone())
        else {
            return Ok(None);
        };
        let mut session = self.sessions.remove(&thread).expect("session is present");
        let event = self.session_event(&thread, &mut session, &message);
        self.sessions.insert(thread, session);
        event
    }

    fn control_event(&mut self, message: &Value) -> Option<AssistantEvent> {
        if message.get("type").and_then(Value::as_str) != Some("control_response") {
            return None;
        }
        let request_id = message.pointer("/response/request_id")?.as_str()?;
        let position = self
            .titles
            .iter()
            .position(|job| job.request_id == request_id)?;
        let job = self.titles.remove(position);
        if job.cancelled {
            return None;
        }
        let title = message
            .pointer("/response/response/title")
            .and_then(Value::as_str)
            .and_then(clean_title);
        Some(match title {
            Some(title) => AssistantEvent::TitleChanged {
                thread_id: job.target,
                title,
            },
            None => AssistantEvent::TitleFailed {
                thread_id: job.target,
            },
        })
    }

    fn session_event(
        &mut self,
        thread: &str,
        session: &mut Session,
        message: &Value,
    ) -> Result<Option<AssistantEvent>> {
        let turn = session.turn.clone().unwrap_or_default();
        match message.get("type").and_then(Value::as_str) {
            Some("control_request") => {
                if self.answer_setup(&mut session.process, message)? {
                    return Ok(None);
                }
                let mcp = message.pointer("/request/message").unwrap_or(&Value::Null);
                let name = mcp
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let request_id = message
                    .get("request_id")
                    .and_then(Value::as_str)
                    .context("Claude Code tool call has no request ID")?
                    .to_owned();
                Ok(Some(AssistantEvent::ToolCall(ToolCall {
                    request_id: json!({
                        "source": session.process.source,
                        "request": request_id,
                        "id": mcp.get("id").cloned().unwrap_or(Value::Null),
                    }),
                    call_id: request_id,
                    thread_id: thread.to_owned(),
                    turn_id: turn,
                    name: super::tools::canonical_name(name).to_owned(),
                    arguments: mcp
                        .pointer("/params/arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                })))
            }
            // Subagent messages have a parent tool call. Qrow allows no subagents.
            Some("stream_event")
                if message.get("parent_tool_use_id").is_none_or(Value::is_null) =>
            {
                let event = message.get("event").unwrap_or(&Value::Null);
                match event.get("type").and_then(Value::as_str) {
                    Some("message_start") => {
                        session.streaming = Some(String::new());
                        Ok(None)
                    }
                    Some("content_block_delta")
                        if event.pointer("/delta/type").and_then(Value::as_str)
                            == Some("text_delta") =>
                    {
                        let text = event
                            .pointer("/delta/text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        session
                            .streaming
                            .get_or_insert_with(String::new)
                            .push_str(&text);
                        Ok((!text.is_empty() && session.turn.is_some()).then(|| {
                            AssistantEvent::MessageDelta {
                                thread_id: thread.to_owned(),
                                turn_id: turn,
                                text,
                            }
                        }))
                    }
                    Some("message_stop") => Ok(Self::complete_message(thread, session)),
                    _ => Ok(None),
                }
            }
            Some("result") => {
                let completed = Self::complete_message(thread, session);
                let turn = message
                    .get("user_message_uuid")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| session.turn.clone())
                    .unwrap_or_default();
                if completed.is_some() {
                    // The final text goes first. The result follows as the
                    // next message, which the inbox keeps.
                    self.inbox
                        .hold(Message::Output(session.process.source, Ok(message.clone())));
                    return Ok(completed);
                }
                let interrupted = std::mem::take(&mut session.interrupted);
                session.turn = None;
                session.last_used = Instant::now();
                let success = message.get("subtype").and_then(Value::as_str) == Some("success")
                    && message.get("is_error").and_then(Value::as_bool) != Some(true);
                let (status, error) = if success {
                    ("completed", None)
                } else if interrupted {
                    ("interrupted", None)
                } else {
                    ("failed", Some(result_error(message)))
                };
                Ok(Some(AssistantEvent::TurnCompleted {
                    thread_id: thread.to_owned(),
                    turn: Turn {
                        id: turn,
                        status: status.into(),
                    },
                    error,
                }))
            }
            _ => Ok(None),
        }
    }

    /// The full text of the reply message that ended.
    fn complete_message(thread: &str, session: &mut Session) -> Option<AssistantEvent> {
        let text = session.streaming.take().filter(|text| !text.is_empty())?;
        Some(AssistantEvent::MessageCompleted {
            thread_id: thread.to_owned(),
            turn_id: session.turn.clone()?,
            text,
        })
    }
}

impl Drop for ClaudeHarness {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Reads one JSON message from each line. A line longer than the limit is
/// skipped, and Qrow keeps the process.
fn read_lines(mut stdout: impl BufRead, sender: &ProcessSender, closing: &AtomicBool) {
    loop {
        let mut bytes = Vec::new();
        let read = stdout
            .by_ref()
            .take((MAX_LINE_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes);
        match read {
            Ok(0) => return,
            Ok(count) if count > MAX_LINE_BYTES => {
                // Skip the rest of the line.
                let mut rest = Vec::new();
                if !bytes.ends_with(b"\n") && stdout.read_until(b'\n', &mut rest).is_err() {
                    return;
                }
                continue;
            }
            Ok(_) => {}
            Err(error) => {
                let _ = sender.send(Err(format!("Could not read Claude Code output: {error}")));
                return;
            }
        }
        if bytes.ends_with(b"\n") {
            bytes.pop();
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // Claude Code can write a line of plain text, for example a warning.
        let Ok(message) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        // A full inbox waits for the worker, so Claude Code waits too. No
        // message is lost: the full text of a reply comes from its parts.
        // The output of a process that stops is not necessary.
        let mut message = Ok(message);
        loop {
            match sender.send(message) {
                Ok(()) => break,
                Err((SendError::Full, _)) if closing.load(Ordering::Acquire) => return,
                Err((SendError::Full, pending)) => {
                    message = pending;
                    thread::sleep(Duration::from_millis(10));
                }
                Err((SendError::Disconnected, _)) => return,
            }
        }
    }
}

fn diagnostic_suffix(process: &Process) -> String {
    let Ok(tail) = process.stderr_tail.lock() else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&tail.iter().copied().collect::<Vec<_>>())
        .trim()
        .to_owned();
    // Claude Code reports a missing sign-in or session on standard error.
    // Qrow shows its last line, which has no credentials.
    text.lines()
        .last()
        .map(|line| line.chars().take(300).collect::<String>())
        .filter(|line| !line.is_empty())
        .map_or_else(String::new, |line| format!(": {line}"))
}

fn result_error(message: &Value) -> String {
    message
        .get("result")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            message
                .get("errors")
                .and_then(Value::as_array)
                .map(|errors| {
                    errors
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
        })
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| {
            format!(
                "Claude Code turn ended with {}.",
                message
                    .get("subtype")
                    .and_then(Value::as_str)
                    .unwrap_or("an error")
            )
        })
}

/// The models and the account from the `initialize` response.
fn snapshot_from_initialize(response: &Value) -> HarnessSnapshot {
    let account = response.get("account").unwrap_or(&Value::Null);
    let text = |key: &str| account.get(key).and_then(Value::as_str);
    let kind = if text("apiProvider").is_some_and(|provider| provider != "firstParty")
        || text("apiKeySource").is_some_and(|source| source != "none")
    {
        AccountKind::ApiKey
    } else if text("tokenSource") == Some("none")
        || account.as_object().is_none_or(|a| a.is_empty())
    {
        AccountKind::SignedOut
    } else {
        AccountKind::Claude {
            plan: text("subscriptionType").map(str::to_owned),
        }
    };
    let models = response
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("value")?.as_str()?.to_owned();
            let display_name = model
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_owned();
            let levels: Vec<_> = model
                .get("supportedEffortLevels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            // The first choice leaves the level to Claude Code.
            let reasoning_efforts = if levels.is_empty() {
                Vec::new()
            } else {
                std::iter::once(DEFAULT_EFFORT.to_owned())
                    .chain(levels)
                    .map(|id| ReasoningEffort { id })
                    .collect()
            };
            Some(Model {
                is_default: id == "default",
                id,
                display_name,
                default_reasoning_effort: DEFAULT_EFFORT.into(),
                reasoning_efforts,
                service_tiers: Vec::new(),
            })
        })
        .collect();
    HarnessSnapshot {
        account: AccountStatus { kind },
        models,
        features: HarnessFeatures {
            steer: false,
            history: false,
            sign_in: false,
        },
    }
}

/// The recent messages of a conversation as the description of a title
/// request.
fn title_description(messages: &[(&'static str, String)]) -> Option<String> {
    let recent = &messages[messages.len().saturating_sub(MAX_TITLE_MESSAGES)..];
    let text: Vec<_> = recent
        .iter()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(role, text)| {
            let role = if *role == "user" { "User" } else { "Assistant" };
            let text: String = text.trim().chars().take(MAX_TITLE_MESSAGE_CHARS).collect();
            format!("{role}: {text}")
        })
        .collect();
    (!text.is_empty()).then(|| text.join("\n\n"))
}

/// Reads `claude --version` and checks it against `MIN_VERSION`.
fn check_version(executable: &Path) -> Result<()> {
    let output = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("Could not start Claude Code")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = parse_version(&text)
        .with_context(|| format!("Claude Code reported an unknown version: {}", text.trim()))?;
    if version < MIN_VERSION {
        let (major, minor, patch) = MIN_VERSION;
        bail!(
            "Claude Code {} is too old. Update it to {major}.{minor}.{patch} or later with `claude update`.",
            text.split_whitespace().next().unwrap_or("")
        );
    }
    Ok(())
}

fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.split_whitespace().next()?.split('.');
    let mut number = || parts.next()?.parse::<u32>().ok();
    Some((number()?, number()?, number()?))
}

/// Deletes the saved session `id` from the project folders of Claude Code:
/// its transcript and the folder of its tool results. Only names that are
/// the UUID of the session match.
fn delete_session_files(config: &Path, id: &str) -> Result<()> {
    let projects = config.join("projects");
    let entries = match std::fs::read_dir(&projects) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("Could not read Claude Code sessions"),
    };
    for project in entries.flatten() {
        let folder = project.path();
        if !folder.is_dir() {
            continue;
        }
        match std::fs::remove_file(folder.join(format!("{id}.jsonl"))) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error).context("Could not delete the Claude Code session");
            }
            _ => {}
        }
        let tool_results = folder.join(id);
        if tool_results.is_dir() {
            std::fs::remove_dir_all(&tool_results)
                .context("Could not delete the Claude Code session files")?;
        }
    }
    Ok(())
}

/// Deletes the saved session `id` of Claude Code without a running harness,
/// for a Claude Code conversation while Codex runs.
pub fn delete_saved_session(id: &str) -> Result<()> {
    ClaudeHarness::ensure_identifier(id)?;
    match default_config_dir() {
        Some(config) => delete_session_files(&config, id),
        None => Ok(()),
    }
}

/// The configuration folder of Claude Code, where it saves sessions.
fn default_config_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}
