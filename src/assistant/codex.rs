use super::{
    AccountKind, AccountStatus, AssistantHarness, HarnessSnapshot, Model, ReasoningEffort,
    ServiceTier,
};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    ffi::OsStr,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_PROTOCOL_LINE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PENDING_MESSAGES: usize = 1_024;
const MAX_MODEL_PAGES: usize = 100;
const MAX_STDERR_BYTES: usize = 16 * 1024;
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_millis(200);
#[cfg(not(test))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(test)]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub struct CodexHarness {
    child: Child,
    writer: Option<SyncSender<WriteCommand>>,
    writer_thread: Option<JoinHandle<()>>,
    messages: Receiver<Result<Value, String>>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    stderr_tail: Arc<Mutex<VecDeque<u8>>>,
    reader_overflowed: Arc<AtomicBool>,
    request_timeout: Duration,
    next_id: u64,
    pending_messages: VecDeque<Value>,
}

impl CodexHarness {
    pub fn launch(executable: impl AsRef<OsStr>, cwd: &Path) -> Result<Self> {
        anyhow::ensure!(cwd.is_dir(), "Codex working directory does not exist");
        let mut command = Command::new(executable);
        command
            .arg("app-server")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command
            .spawn()
            .context("Could not start Codex app-server")?;
        let mut child = LaunchChild::new(child);
        let stdin = child
            .child_mut()
            .stdin
            .take()
            .context("Codex app-server did not provide standard input")?;
        let stdout = child
            .child_mut()
            .stdout
            .take()
            .context("Codex app-server did not provide standard output")?;
        let stderr = child
            .child_mut()
            .stderr
            .take()
            .context("Codex app-server did not provide standard error")?;
        let (message_tx, messages) = mpsc::sync_channel(MAX_PENDING_MESSAGES + 1);
        let reader_overflowed = Arc::new(AtomicBool::new(false));
        let stdout_thread = thread::Builder::new()
            .name("qrow-codex-stdout".into())
            .spawn({
                let overflowed = Arc::clone(&reader_overflowed);
                move || read_protocol_stream(BufReader::new(stdout), &message_tx, &overflowed)
            })
            .context("Could not start Codex output reader")?;
        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_STDERR_BYTES)));
        let stderr_thread = {
            let tail = Arc::clone(&stderr_tail);
            thread::Builder::new()
                .name("qrow-codex-stderr".into())
                .spawn(move || read_stderr_tail(stderr, &tail))
                .context("Could not start Codex error reader")?
        };
        let (writer, write_commands) = mpsc::sync_channel(1);
        let writer_thread = thread::Builder::new()
            .name("qrow-codex-stdin".into())
            .spawn(move || write_protocol_stream(stdin, &write_commands))
            .context("Could not start Codex input writer")?;
        let mut harness = Self {
            child: child.into_inner(),
            writer: Some(writer),
            writer_thread: Some(writer_thread),
            messages,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            stderr_tail,
            reader_overflowed,
            request_timeout: REQUEST_TIMEOUT,
            next_id: 1,
            pending_messages: VecDeque::new(),
        };
        if let Err(error) = harness.initialize() {
            let _ = harness.shutdown();
            return Err(error);
        }
        Ok(harness)
    }

    fn initialize(&mut self) -> Result<()> {
        let _: Value = self.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "qrow",
                    "title": "Qrow",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": true,
                },
            }),
        )?;
        self.notify("initialized", json!({}))
    }

    /// Takes protocol notifications and server requests received while Qrow
    /// waited for a response.
    pub fn take_pending_messages(&mut self) -> Vec<Value> {
        self.pending_messages.drain(..).collect()
    }

    fn account(&mut self) -> Result<AccountStatus> {
        let response: AccountResponse =
            self.request("account/read", json!({ "refreshToken": false }))?;
        let kind = match response.account {
            None => AccountKind::SignedOut,
            Some(account) => match account
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
            {
                "chatgpt" => AccountKind::ChatGpt {
                    plan: account
                        .get("planType")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                },
                "apiKey" => AccountKind::ApiKey,
                other => AccountKind::Other(other.to_owned()),
            },
        };
        Ok(AccountStatus {
            kind,
            requires_openai_auth: response.requires_openai_auth,
        })
    }

    fn models(&mut self) -> Result<Vec<Model>> {
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = BTreeSet::new();
        for _ in 0..MAX_MODEL_PAGES {
            let response: ModelListResponse = self.request(
                "model/list",
                json!({
                    "cursor": cursor,
                    "limit": 100,
                    "includeHidden": false,
                }),
            )?;
            models.extend(response.data.into_iter().map(Model::from));
            let Some(next_cursor) = response.next_cursor else {
                return Ok(models);
            };
            if !seen_cursors.insert(next_cursor.clone()) {
                bail!("Codex model list returned a repeated cursor");
            }
            cursor = Some(next_cursor);
        }
        bail!("Codex model list exceeded {MAX_MODEL_PAGES} pages")
    }

    fn request<T: for<'de> Deserialize<'de>>(
        &mut self,
        method: &'static str,
        params: Value,
    ) -> Result<T> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("Codex request identifier overflowed")?;
        self.write(&Request { id, method, params })?;
        let deadline = Instant::now()
            .checked_add(self.request_timeout)
            .context("Codex request deadline overflowed")?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!(
                    "Codex app-server did not respond within {} seconds{}",
                    self.request_timeout.as_secs_f32(),
                    self.diagnostic_suffix()
                );
            }
            let message = self.read_message(remaining)?;
            let response_id = message.get("id").and_then(Value::as_u64);
            if message.get("method").is_none() && response_id == Some(id) {
                if let Some(error) = message.get("error") {
                    let code = error.get("code").cloned().unwrap_or(Value::Null);
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown app-server error");
                    bail!("Codex request {method} failed ({code}): {message}");
                }
                let result = message.get("result").cloned().with_context(|| {
                    format!("Codex response to {method} did not contain a result")
                })?;
                return serde_json::from_value(result)
                    .with_context(|| format!("Codex response to {method} had an invalid shape"));
            }
            if message.get("id").is_some() && message.get("method").is_none() {
                match response_id {
                    Some(response_id) if response_id < id => continue,
                    _ => bail!("Codex app-server returned an unexpected response identifier"),
                }
            }
            if self.pending_messages.len() == MAX_PENDING_MESSAGES {
                bail!("Codex app-server sent too many unsolicited messages");
            }
            self.pending_messages.push_back(message);
        }
    }

    fn notify(&mut self, method: &'static str, params: Value) -> Result<()> {
        self.write(&Notification { method, params })
    }

    fn write(&mut self, value: &impl Serialize) -> Result<()> {
        let mut bytes = serde_json::to_vec(value).context("Could not encode Codex request")?;
        bytes.push(b'\n');
        anyhow::ensure!(
            bytes.len() <= MAX_PROTOCOL_LINE_BYTES,
            "Codex request exceeded the protocol size limit"
        );
        let writer = self
            .writer
            .as_ref()
            .context("Codex app-server input is closed")?;
        let (completed, completion) = mpsc::sync_channel(1);
        writer
            .try_send(WriteCommand { bytes, completed })
            .map_err(|error| match error {
                TrySendError::Full(_) => anyhow!("Codex app-server input is busy"),
                TrySendError::Disconnected(_) => anyhow!("Codex app-server input is closed"),
            })?;
        match completion.recv_timeout(self.request_timeout) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => bail!("{error}{}", self.diagnostic_suffix()),
            Err(RecvTimeoutError::Timeout) => bail!(
                "Codex app-server did not accept a request within {} seconds{}",
                self.request_timeout.as_secs_f32(),
                self.diagnostic_suffix()
            ),
            Err(RecvTimeoutError::Disconnected) => {
                bail!("Codex app-server input closed{}", self.diagnostic_suffix())
            }
        }
    }

    fn read_message(&mut self, timeout: Duration) -> Result<Value> {
        if self.reader_overflowed.load(Ordering::Acquire) {
            bail!("Codex app-server sent too many queued messages");
        }
        match self.messages.recv_timeout(timeout) {
            Ok(Ok(message)) => Ok(message),
            Ok(Err(error)) => bail!("{error}{}", self.diagnostic_suffix()),
            Err(RecvTimeoutError::Timeout) => {
                bail!(
                    "Codex app-server did not respond within {} seconds{}",
                    self.request_timeout.as_secs_f32(),
                    self.diagnostic_suffix()
                )
            }
            Err(RecvTimeoutError::Disconnected) => {
                let status = self.child.try_wait().ok().flatten();
                let reason = match status {
                    Some(status) => format!("exited with {status}"),
                    None => "closed its output".to_owned(),
                };
                bail!("Codex app-server {reason}{}", self.diagnostic_suffix())
            }
        }
    }

    fn diagnostic_suffix(&self) -> String {
        let Ok(tail) = self.stderr_tail.lock() else {
            return String::new();
        };
        if tail.is_empty() {
            String::new()
        } else {
            let truncation = if tail.len() == MAX_STDERR_BYTES {
                " (truncated)"
            } else {
                ""
            };
            format!(
                "; Codex wrote diagnostic output{truncation}; details are hidden to protect sensitive data"
            )
        }
    }

    fn join_readers(&mut self) {
        if let Some(thread) = self.writer_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stdout_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
}

struct LaunchChild {
    child: Option<Child>,
}

impl LaunchChild {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("launch child is present")
    }

    fn into_inner(mut self) -> Child {
        self.child.take().expect("launch child is present")
    }
}

impl Drop for LaunchChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = kill_process_tree(&mut child);
            let _ = child.wait();
        }
    }
}

struct WriteCommand {
    bytes: Vec<u8>,
    completed: SyncSender<std::result::Result<(), String>>,
}

fn write_protocol_stream(mut stdin: ChildStdin, commands: &Receiver<WriteCommand>) {
    while let Ok(command) = commands.recv() {
        let result = stdin
            .write_all(&command.bytes)
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("Could not send request to Codex app-server: {error}"));
        let failed = result.is_err();
        let _ = command.completed.try_send(result);
        if failed {
            return;
        }
    }
}

fn read_protocol_stream(
    mut stdout: impl BufRead,
    messages: &SyncSender<Result<Value, String>>,
    overflowed: &AtomicBool,
) {
    loop {
        let mut bytes = Vec::new();
        let read = stdout
            .by_ref()
            .take((MAX_PROTOCOL_LINE_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes);
        let count = match read {
            Ok(count) => count,
            Err(error) => {
                send_protocol_message(
                    messages,
                    Err(format!("Could not read Codex app-server output: {error}")),
                    overflowed,
                );
                return;
            }
        };
        if count == 0 {
            return;
        }
        if count > MAX_PROTOCOL_LINE_BYTES || !bytes.ends_with(b"\n") {
            send_protocol_message(
                messages,
                Err("Codex app-server sent an oversized or incomplete message".into()),
                overflowed,
            );
            return;
        }
        bytes.pop();
        let message = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Codex app-server sent invalid JSON: {error}"));
        if !send_protocol_message(messages, message, overflowed) {
            return;
        }
    }
}

fn send_protocol_message(
    messages: &SyncSender<Result<Value, String>>,
    message: Result<Value, String>,
    overflowed: &AtomicBool,
) -> bool {
    match messages.try_send(message) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            overflowed.store(true, Ordering::Release);
            false
        }
        Err(TrySendError::Disconnected(_)) => false,
    }
}

fn read_stderr_tail(mut stderr: impl Read, tail: &Mutex<VecDeque<u8>>) {
    let mut buffer = [0_u8; 4096];
    loop {
        let count = match stderr.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        let Ok(mut tail) = tail.lock() else {
            return;
        };
        tail.extend(&buffer[..count]);
        while tail.len() > MAX_STDERR_BYTES {
            tail.pop_front();
        }
    }
}

#[cfg(unix)]
fn kill_process_tree(child: &mut Child) -> std::io::Result<()> {
    let group = format!("-{}", child.id());
    let status = Command::new("/bin/kill")
        .args(["-KILL", "--", &group])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if status.is_ok_and(|status| status.success()) || child.try_wait()?.is_some() {
        Ok(())
    } else {
        child.kill()
    }
}

#[cfg(not(unix))]
fn kill_process_tree(child: &mut Child) -> std::io::Result<()> {
    child.kill()
}

impl AssistantHarness for CodexHarness {
    fn snapshot(&mut self) -> Result<HarnessSnapshot> {
        Ok(HarnessSnapshot {
            account: self.account()?,
            models: self.models()?,
        })
    }

    fn shutdown(&mut self) -> Result<()> {
        self.writer.take();
        let checks = SHUTDOWN_GRACE_PERIOD.as_millis() / 10;
        for _ in 0..checks {
            if self.child.try_wait()?.is_some() {
                self.join_readers();
                return Ok(());
            }
            thread::sleep(Duration::from_millis(10));
        }
        kill_process_tree(&mut self.child).context("Could not stop Codex app-server")?;
        self.child
            .wait()
            .context("Could not wait for Codex app-server")?;
        self.join_readers();
        Ok(())
    }
}

impl Drop for CodexHarness {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[derive(Serialize)]
struct Request {
    id: u64,
    method: &'static str,
    params: Value,
}

#[derive(Serialize)]
struct Notification {
    method: &'static str,
    params: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountResponse {
    account: Option<Value>,
    requires_openai_auth: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelListResponse {
    data: Vec<CodexModel>,
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexModel {
    id: String,
    display_name: String,
    description: String,
    is_default: bool,
    default_reasoning_effort: String,
    supported_reasoning_efforts: Vec<CodexReasoningEffort>,
    #[serde(default)]
    default_service_tier: Option<String>,
    #[serde(default)]
    service_tiers: Vec<CodexServiceTier>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexReasoningEffort {
    reasoning_effort: String,
    description: String,
}

#[derive(Deserialize)]
struct CodexServiceTier {
    id: String,
    name: String,
    description: String,
}

impl From<CodexModel> for Model {
    fn from(model: CodexModel) -> Self {
        Self {
            id: model.id,
            display_name: model.display_name,
            description: model.description,
            is_default: model.is_default,
            default_reasoning_effort: model.default_reasoning_effort,
            reasoning_efforts: model
                .supported_reasoning_efforts
                .into_iter()
                .map(|effort| ReasoningEffort {
                    id: effort.reasoning_effort,
                    description: effort.description,
                })
                .collect(),
            default_service_tier: model.default_service_tier,
            service_tiers: model
                .service_tiers
                .into_iter()
                .map(|tier| ServiceTier {
                    id: tier.id,
                    name: tier.name,
                    description: tier.description,
                })
                .collect(),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, io::Cursor, os::unix::fs::PermissionsExt};

    fn write_executable(path: &Path, script: &str) {
        fs::write(path, script).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn handshake_reads_account_and_paginated_model_capabilities() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        write_executable(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
    printf '%s\n' "$line" >> requests.jsonl
    case "$line" in
        *'"method":"initialize"'*)
            printf '%s\n' '{"id":1,"result":{"codexHome":"/tmp/codex","platformFamily":"unix","platformOs":"macos","userAgent":"fake"}}'
            ;;
        *'"method":"account/read"'*)
            printf '%s\n' '{"id":2,"result":{"account":{"type":"chatgpt","email":"person@example.com","planType":"plus"},"requiresOpenaiAuth":true}}'
            ;;
        *'"method":"model/list"'*'"cursor":"next"'*)
            printf '%s\n' '{"id":4,"result":{"data":[{"id":"model-2","model":"model-2","displayName":"Model 2","description":"Second","hidden":false,"isDefault":false,"defaultReasoningEffort":"low","supportedReasoningEfforts":[{"reasoningEffort":"low","description":"Fast"}],"serviceTiers":[]}],"nextCursor":null}}'
            ;;
        *'"method":"model/list"'*)
            printf '%s\n' '{"id":3,"result":{"data":[{"id":"model-1","model":"model-1","displayName":"Model 1","description":"First","hidden":false,"isDefault":true,"defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"reasoningEffort":"medium","description":"Balanced"}],"defaultServiceTier":"standard","serviceTiers":[{"id":"standard","name":"Standard","description":"Normal speed"},{"id":"fast","name":"Fast","description":"Lower latency"}]}],"nextCursor":"next"}}'
            ;;
    esac
        done
"#,
        );

        let mut harness = CodexHarness::launch(&executable, directory.path()).unwrap();
        let snapshot = harness.snapshot().unwrap();

        assert_eq!(
            snapshot.account().kind(),
            &AccountKind::ChatGpt {
                plan: Some("plus".into())
            }
        );
        assert!(snapshot.account().requires_openai_auth());
        assert_eq!(snapshot.models().len(), 2);
        assert_eq!(snapshot.models()[0].id(), "model-1");
        assert!(snapshot.models()[0].is_default());
        assert_eq!(snapshot.models()[0].default_reasoning_effort(), "medium");
        assert_eq!(snapshot.models()[0].reasoning_efforts()[0].id(), "medium");
        assert_eq!(snapshot.models()[0].service_tiers()[1].id(), "fast");
        assert_eq!(
            snapshot.models()[0].default_service_tier(),
            Some("standard")
        );
        assert_eq!(snapshot.models()[1].id(), "model-2");

        harness.shutdown().unwrap();
        let requests = fs::read_to_string(directory.path().join("requests.jsonl")).unwrap();
        assert!(requests.contains(r#""experimentalApi":true"#));
        assert!(requests.contains(r#""method":"initialized""#));
        assert!(requests.contains(r#""refreshToken":false"#));
        assert!(requests.contains(r#""cursor":"next""#));
    }

    #[test]
    fn repeated_model_cursor_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        write_executable(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
    case "$line" in
        *'"method":"initialize"'*)
            printf '%s\n' '{"id":1,"result":{"codexHome":"/tmp/codex","platformFamily":"unix","platformOs":"macos","userAgent":"fake"}}'
            ;;
        *'"method":"account/read"'*)
            printf '%s\n' '{"id":2,"result":{"account":null,"requiresOpenaiAuth":true}}'
            ;;
        *'"method":"model/list"'*)
            id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
            printf '{"id":%s,"result":{"data":[],"nextCursor":"same"}}\n' "$id"
            ;;
    esac
done
"#,
        );

        let mut harness = CodexHarness::launch(&executable, directory.path()).unwrap();
        let error = harness.snapshot().unwrap_err();

        assert!(error.to_string().contains("repeated cursor"));
    }

    #[test]
    fn invalid_initialize_response_fails_and_stops_the_process() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        write_executable(
            &executable,
            r#"#!/bin/sh
IFS= read -r line
printf '%s\n' 'not-json'
sleep 30
"#,
        );

        let started = std::time::Instant::now();
        let error = match CodexHarness::launch(&executable, directory.path()) {
            Ok(_) => panic!("invalid JSON must fail initialization"),
            Err(error) => error,
        };

        assert!(
            error.to_string().contains("invalid JSON"),
            "unexpected error: {error:#}"
        );
        assert!(started.elapsed() < REQUEST_TIMEOUT.saturating_mul(3));
    }

    #[test]
    fn server_request_is_available_to_the_caller() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        write_executable(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
    case "$line" in
        *'"method":"initialize"'*)
            printf '%s\n' '{"id":1,"method":"item/tool/call","params":{"name":"read_context"}}'
            printf '%s\n' '{"id":1,"result":{}}'
            ;;
    esac
done
"#,
        );

        let mut harness = CodexHarness::launch(&executable, directory.path()).unwrap();
        let pending = harness.take_pending_messages();

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["id"], 1);
        assert_eq!(pending[0]["method"], "item/tool/call");
    }

    #[test]
    fn late_response_is_discarded_before_the_next_response() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        write_executable(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
    case "$line" in
        *'"method":"initialize"'*)
            printf '%s\n' '{"id":1,"result":{}}'
            ;;
        *'"method":"account/read"'*)
            if [ "$id" = 2 ]; then sleep 1; fi
            printf '{"id":%s,"result":{"account":null,"requiresOpenaiAuth":true}}\n' "$id"
            ;;
        *'"method":"model/list"'*)
            printf '{"id":%s,"result":{"data":[],"nextCursor":null}}\n' "$id"
            ;;
    esac
done
"#,
        );
        let mut harness = CodexHarness::launch(&executable, directory.path()).unwrap();
        harness.request_timeout = Duration::from_millis(100);

        let first_error = harness.snapshot().unwrap_err();
        assert!(first_error.to_string().contains("did not respond"));
        harness.request_timeout = Duration::from_secs(2);
        let snapshot = harness.snapshot().unwrap();

        assert_eq!(snapshot.account().kind(), &AccountKind::SignedOut);
        assert!(snapshot.models().is_empty());
    }

    #[test]
    fn future_response_identifier_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        write_executable(
            &executable,
            r#"#!/bin/sh
IFS= read -r line
printf '%s\n' '{"id":2,"result":{}}'
"#,
        );

        let error = match CodexHarness::launch(&executable, directory.path()) {
            Ok(_) => panic!("a future response identifier must fail initialization"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("unexpected response identifier"));
    }

    #[test]
    fn stale_responses_cannot_extend_request_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        write_executable(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
    case "$line" in
        *'"method":"initialize"'*)
            printf '%s\n' '{"id":1,"result":{}}'
            ;;
        *'"method":"account/read"'*)
            i=0
            while [ "$i" -lt 20 ]; do
                printf '%s\n' '{"id":1,"result":{}}'
                sleep 0.05
                i=$((i + 1))
            done
            sleep 30
            ;;
    esac
done
"#,
        );
        let mut harness = CodexHarness::launch(&executable, directory.path()).unwrap();
        harness.request_timeout = Duration::from_millis(250);
        let started = Instant::now();

        let error = harness.snapshot().unwrap_err();

        assert!(error.to_string().contains("did not respond"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn blocked_protocol_write_times_out() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        write_executable(
            &executable,
            r#"#!/bin/sh
IFS= read -r line
printf '%s\n' '{"id":1,"result":{}}'
IFS= read -r line
sleep 30
"#,
        );
        let mut harness = CodexHarness::launch(&executable, directory.path()).unwrap();
        harness.request_timeout = Duration::from_millis(500);
        let started = std::time::Instant::now();

        let error = harness
            .notify("test/large", json!({ "payload": "x".repeat(1_000_000) }))
            .unwrap_err();

        assert!(error.to_string().contains("did not accept"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn pending_message_bound_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        write_executable(
            &executable,
            r#"#!/bin/sh
IFS= read -r line
i=0
while [ "$i" -le 1024 ]; do
    printf '{"method":"notice","params":{"index":%s}}\n' "$i"
    i=$((i + 1))
done
printf '%s\n' '{"id":1,"result":{}}'
"#,
        );

        let error = match CodexHarness::launch(&executable, directory.path()) {
            Ok(_) => panic!("an unbounded notification stream must fail initialization"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("too many unsolicited messages"));
    }

    #[test]
    fn timeout_reports_stderr_without_exposing_its_contents() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        write_executable(
            &executable,
            r#"#!/bin/sh
IFS= read -r line
printf '%s\n' 'sign in required' >&2
sleep 30
"#,
        );

        let started = std::time::Instant::now();
        let error = match CodexHarness::launch(&executable, directory.path()) {
            Ok(_) => panic!("a silent app-server must time out"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("did not respond"));
        assert!(error.to_string().contains("diagnostic output"));
        assert!(!error.to_string().contains("sign in required"));
        assert!(started.elapsed() < REQUEST_TIMEOUT.saturating_mul(3));
    }

    #[test]
    fn stderr_tail_keeps_only_the_latest_bounded_bytes() {
        let mut input = vec![b'a'; MAX_STDERR_BYTES];
        input.extend(vec![b'b'; 4_096]);
        let tail = Mutex::new(VecDeque::new());

        read_stderr_tail(Cursor::new(input), &tail);

        let tail = tail.lock().unwrap();
        assert_eq!(tail.len(), MAX_STDERR_BYTES);
        assert!(tail.iter().rev().take(4_096).all(|byte| *byte == b'b'));
    }

    #[test]
    fn forced_shutdown_stops_descendant_processes() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-codex");
        write_executable(
            &executable,
            r#"#!/bin/sh
sleep 30 &
printf '%s\n' "$!" > descendant.pid
IFS= read -r line
while :; do sleep 1; done
"#,
        );

        let error = match CodexHarness::launch(&executable, directory.path()) {
            Ok(_) => panic!("a silent app-server must time out"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("did not respond"));
        let descendant = fs::read_to_string(directory.path().join("descendant.pid")).unwrap();
        let output = Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", descendant.trim()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);

        assert!(
            !output.status.success() || state.trim_start().starts_with('Z'),
            "descendant remained active with state {state:?}"
        );
    }

    #[test]
    fn protocol_reader_rejects_oversized_and_incomplete_messages() {
        let mut oversized = vec![b'x'; MAX_PROTOCOL_LINE_BYTES + 1];
        oversized.push(b'\n');
        let (tx, rx) = mpsc::sync_channel(2);
        let overflowed = AtomicBool::new(false);
        read_protocol_stream(Cursor::new(oversized), &tx, &overflowed);
        assert!(
            rx.recv()
                .unwrap()
                .unwrap_err()
                .contains("oversized or incomplete")
        );

        let (tx, rx) = mpsc::sync_channel(2);
        let overflowed = AtomicBool::new(false);
        read_protocol_stream(Cursor::new(br#"{"id":1}"#), &tx, &overflowed);
        assert!(
            rx.recv()
                .unwrap()
                .unwrap_err()
                .contains("oversized or incomplete")
        );
    }

    #[test]
    fn protocol_reader_records_idle_queue_overflow() {
        let input = b"{\"method\":\"one\"}\n{\"method\":\"two\"}\n";
        let (tx, _rx) = mpsc::sync_channel(1);
        let overflowed = AtomicBool::new(false);

        read_protocol_stream(Cursor::new(input), &tx, &overflowed);

        assert!(overflowed.load(Ordering::Acquire));
    }
}
