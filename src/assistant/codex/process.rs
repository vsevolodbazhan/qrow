//! The Codex process: launch, protocol streams, stderr, and process-tree
//! shutdown.
use super::*;

pub(in crate::assistant) struct LaunchChild {
    child: Option<Child>,
}

impl LaunchChild {
    pub(in crate::assistant) fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    pub(in crate::assistant) fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("launch child is present")
    }

    pub(in crate::assistant) fn into_inner(mut self) -> Child {
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

pub(in crate::assistant) struct WriteCommand {
    pub(in crate::assistant) bytes: Vec<u8>,
    pub(in crate::assistant) completed: SyncSender<std::result::Result<(), String>>,
}

pub(in crate::assistant) fn write_protocol_stream(
    mut stdin: ChildStdin,
    commands: &Receiver<WriteCommand>,
) {
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

pub(super) fn read_protocol_stream(
    mut stdout: impl BufRead,
    messages: &OutputSender,
    overflowed: &AtomicBool,
) {
    let read_error = |error: std::io::Error| {
        send_protocol_message(
            messages,
            Err(format!("Could not read Codex app-server output: {error}")),
            overflowed,
        );
    };
    let incomplete = || {
        send_protocol_message(
            messages,
            Err("Codex app-server sent an incomplete message".into()),
            overflowed,
        );
    };
    loop {
        let mut bytes = Vec::new();
        let read = stdout
            .by_ref()
            .take((MAX_PROTOCOL_LINE_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes);
        let count = match read {
            Ok(count) => count,
            Err(error) => return read_error(error),
        };
        if count == 0 {
            return;
        }
        let complete = bytes.ends_with(b"\n");
        if count > MAX_PROTOCOL_LINE_BYTES {
            // Qrow discards the message and keeps the session. The start of
            // the message tells which request fails.
            bytes.truncate(OVERSIZED_HEAD_BYTES);
            if !complete {
                match discard_line(&mut stdout) {
                    Ok(true) => {}
                    Ok(false) => return incomplete(),
                    Err(error) => return read_error(error),
                }
            }
            if let Some(message) = oversized_message(&bytes)
                && !send_protocol_message(messages, Ok(message), overflowed)
            {
                return;
            }
            continue;
        }
        if !complete {
            return incomplete();
        }
        bytes.pop();
        let message = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Codex app-server sent invalid JSON: {error}"));
        if !send_protocol_message(messages, message, overflowed) {
            return;
        }
    }
}

/// Reads up to the next line end. Returns false at the end of the output.
fn discard_line(reader: &mut impl BufRead) -> std::io::Result<bool> {
    loop {
        let buffer = match reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if buffer.is_empty() {
            return Ok(false);
        }
        if let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            reader.consume(end + 1);
            return Ok(true);
        }
        let length = buffer.len();
        reader.consume(length);
    }
}

/// The message that replaces an oversized message, from the start of that
/// message. A response becomes an error response for its request. A request
/// from Codex becomes a request that Qrow refuses. Qrow skips a notification
/// and a message without a readable `id`.
fn oversized_message(head: &[u8]) -> Option<Value> {
    let (id, method) = protocol_head(head);
    let id = id?;
    Some(if method.is_some() {
        json!({ "id": id, "method": OVERSIZED_REQUEST_METHOD, "params": {} })
    } else {
        json!({
            "id": id,
            "error": {
                "code": OVERSIZED_RESPONSE_CODE,
                "message": format!(
                    "The response is larger than {} MB",
                    MAX_PROTOCOL_LINE_BYTES / (1024 * 1024)
                ),
            },
        })
    })
}

/// Reads the top-level `id` and `method` fields from the start of a JSON
/// object. The object can end before its closing brace.
pub(super) fn protocol_head(head: &[u8]) -> (Option<Value>, Option<String>) {
    let skip_space = |mut position: usize| {
        while head.get(position).is_some_and(u8::is_ascii_whitespace) {
            position += 1;
        }
        position
    };
    let (mut id, mut method) = (None, None);
    let mut position = skip_space(0);
    if head.get(position) != Some(&b'{') {
        return (id, method);
    }
    position += 1;
    loop {
        position = skip_space(position);
        let Some(key_end) = json_value_end(head, position) else {
            break;
        };
        let Ok(key) = serde_json::from_slice::<String>(&head[position..key_end]) else {
            break;
        };
        position = skip_space(key_end);
        if head.get(position) != Some(&b':') {
            break;
        }
        position = skip_space(position + 1);
        let Some(value_end) = json_value_end(head, position) else {
            break;
        };
        let value = &head[position..value_end];
        match key.as_str() {
            "id" => {
                id = serde_json::from_slice::<Value>(value)
                    .ok()
                    .filter(|id| id.is_u64() || id.is_string());
            }
            "method" => method = serde_json::from_slice::<String>(value).ok(),
            _ => {}
        }
        position = skip_space(value_end);
        if head.get(position) != Some(&b',') {
            break;
        }
        position += 1;
    }
    (id, method)
}

/// The end of the JSON value at `start`, or `None` when `bytes` ends first.
fn json_value_end(bytes: &[u8], start: usize) -> Option<usize> {
    match bytes.get(start)? {
        b'"' => {
            let mut position = start + 1;
            while let Some(byte) = bytes.get(position) {
                match byte {
                    b'\\' => position += 2,
                    b'"' => return Some(position + 1),
                    _ => position += 1,
                }
            }
            None
        }
        b'{' | b'[' => {
            let mut depth = 0_usize;
            let mut position = start;
            while let Some(byte) = bytes.get(position) {
                match byte {
                    b'"' => {
                        position = json_value_end(bytes, position)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(position + 1);
                        }
                    }
                    _ => {}
                }
                position += 1;
            }
            None
        }
        _ => bytes[start..]
            .iter()
            .position(|byte| matches!(byte, b',' | b'}' | b']') || byte.is_ascii_whitespace())
            .map(|length| start + length),
    }
}

/// Sends Codex output to the worker. Under backpressure it drops streamed
/// fragments only, because a completed turn reloads its history.
fn send_protocol_message(
    messages: &OutputSender,
    message: Result<Value, String>,
    overflowed: &AtomicBool,
) -> bool {
    let delta = message.as_ref().is_ok_and(|value| {
        value.get("method").and_then(Value::as_str) == Some("item/agentMessage/delta")
    });
    match messages.send(message) {
        Ok(()) => true,
        Err(SendError::Full) if delta => true,
        Err(SendError::Full) => {
            overflowed.store(true, Ordering::Release);
            false
        }
        Err(SendError::Disconnected) => false,
    }
}

pub(in crate::assistant) fn read_stderr_tail(mut stderr: impl Read, tail: &Mutex<VecDeque<u8>>) {
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
pub(in crate::assistant) fn kill_process_tree(child: &mut Child) -> std::io::Result<()> {
    if terminate_process_group(child.id()).is_ok() || child.try_wait()?.is_some() {
        Ok(())
    } else {
        child.kill()
    }
}

#[cfg(unix)]
pub(crate) fn terminate_process_group(pid: u32) -> std::io::Result<()> {
    if pid <= 1 {
        return Err(std::io::Error::other("invalid Codex process identifier"));
    }
    let group = format!("-{pid}");
    let status = Command::new("/bin/kill")
        .args(["-KILL", "--", &group])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("could not stop Codex process group"))
    }
}

#[cfg(not(unix))]
pub(in crate::assistant) fn kill_process_tree(child: &mut Child) -> std::io::Result<()> {
    child.kill()
}

#[cfg(windows)]
pub(crate) fn terminate_process_tree(pid: u32) -> std::io::Result<()> {
    if pid == 0 {
        return Err(std::io::Error::other("invalid Codex process identifier"));
    }
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("could not stop Codex process tree"))
    }
}
