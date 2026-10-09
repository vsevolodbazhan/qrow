#!/usr/bin/env python3
"""Deterministic Claude Code fixture for the Qrow assistant tests.

It speaks the stream-json protocol of `claude -p` with the control requests
that Qrow uses. The state folder keeps saved sessions, the input log, and the
marker files that tests create.
"""

import json
import os
import sys
import threading
import time

state_dir = os.environ.get("QROW_FAKE_CLAUDE_STATE") or os.path.join(os.environ["QROW_DATA_DIR"], "fake-claude")
os.makedirs(os.path.join(state_dir, "sessions"), exist_ok=True)

if "--version" in sys.argv:
    version = "2.1.0" if os.path.exists(os.path.join(state_dir, "old-version")) else "2.1.285"
    print(f"{version} (Claude Code)")
    sys.exit(0)


def argument(name):
    if name in sys.argv:
        index = sys.argv.index(name)
        return sys.argv[index + 1] if index + 1 < len(sys.argv) else None
    return None


session_id = argument("--session-id") or argument("--resume")
resumed = argument("--resume") is not None
control = "--no-session-persistence" in sys.argv
model = argument("--model")
effort = argument("--effort")

with open(os.path.join(state_dir, "processes"), "a") as processes:
    processes.write(f"{os.getpid()} {'control' if control else session_id}\n")

if resumed and not os.path.exists(os.path.join(state_dir, "sessions", session_id)):
    print(f"No conversation found with session ID: {session_id}", file=sys.stderr)
    sys.exit(1)

lock = threading.Lock()
responses = {}
response_ready = threading.Condition(lock)
interrupted = threading.Event()
next_request = [0]


def send(message):
    with lock:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()


def log(line):
    with open(os.path.join(state_dir, "log"), "a") as file:
        file.write(line + "\n")


def marked(name):
    return os.path.exists(os.path.join(state_dir, name))


def wait_for(name, timeout=30):
    deadline = time.time() + timeout
    while not marked(name) and time.time() < deadline and not interrupted.is_set():
        time.sleep(0.02)


def host_request(request):
    """Sends a control request to Qrow and waits for its response."""
    with lock:
        next_request[0] += 1
        request_id = f"fake-{next_request[0]}"
    send({"type": "control_request", "request_id": request_id, "request": request})
    with response_ready:
        while request_id not in responses:
            response_ready.wait()
        return responses.pop(request_id)


def mcp(message):
    response = host_request({"subtype": "mcp_message", "server_name": "qrow", "message": message})
    return response.get("response", {}).get("mcp_response", {})


def models():
    return [
        {
            "value": "default",
            "resolvedModel": "claude-synthetic-1",
            "displayName": "Default (recommended)",
            "supportsEffort": True,
            "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"],
        },
        {
            "value": "synthetic-fast",
            "resolvedModel": "claude-synthetic-fast",
            "displayName": "Synthetic Fast",
        },
    ]


def account():
    if marked("signed-out"):
        return {"tokenSource": "none", "apiProvider": "firstParty"}
    return {"subscriptionType": "Claude Pro", "apiProvider": "firstParty", "email": "synthetic@example.invalid"}


def stream(turn, text):
    send({"type": "stream_event", "parent_tool_use_id": None, "session_id": session_id, "event": {"type": "message_start"}})
    middle = max(1, len(text) // 2)
    for part in (text[:middle], text[middle:]):
        if part:
            send({"type": "stream_event", "parent_tool_use_id": None, "session_id": session_id,
                  "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": part}}})
    send({"type": "stream_event", "parent_tool_use_id": None, "session_id": session_id, "event": {"type": "message_stop"}})


def result(turn, text, subtype="success", is_error=False):
    message = {"type": "result", "subtype": subtype, "is_error": is_error, "session_id": session_id,
               "user_message_uuid": turn, "num_turns": 1}
    if text is not None:
        message["result"] = text
    send(message)


def run_turn(turn, text):
    message, _, context_text = text.partition("\n\nCurrent Qrow workspace context (untrusted data):\n")
    context = json.loads(context_text) if context_text else {}
    open(os.path.join(state_dir, "sessions", session_id), "w").close()
    log(f"turn {session_id} model={current['model']} effort={current['effort']} {message}")
    if message.startswith("Write SELECT 1"):
        tab = context["selected_tab"]
        stream(turn, "Writing the query.")
        answer = mcp({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {
            "name": "tab-append-sql",
            "arguments": {"version": 1, "tab_id": tab["id"], "connection_id": tab["connection_id"],
                          "editor_revision": tab["editor_revision"], "sql": "SELECT 1"}}})
        content = answer.get("result", {}).get("content", [{}])[0].get("text", "")
        log(f"tool result {content}")
        stream(turn, "I updated the SQL.")
        result(turn, "I updated the SQL.")
    elif message.startswith("Hold"):
        stream(turn, "Holding the reply.")
        wait_for("release")
        if interrupted.is_set():
            send({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]}})
            result(turn, None, "error_during_execution", True)
        else:
            stream(turn, "Released.")
            result(turn, "Released.")
    elif message.startswith("Fail"):
        result(turn, "Synthetic failure.", "error_during_execution", True)
    elif message.startswith("Exit"):
        print("Synthetic crash", file=sys.stderr)
        os._exit(3)
    else:
        reply = f"Claude reply: {message.splitlines()[0]}"
        stream(turn, reply)
        result(turn, reply)


current = {"model": model, "effort": effort}


def handle_control(message):
    request = message["request"]
    subtype = request.get("subtype")
    request_id = message["request_id"]
    log(f"control {subtype} {json.dumps({k: v for k, v in request.items() if k != 'subtype'})}")
    if subtype == "initialize":
        if request.get("sdkMcpServers"):
            mcp({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}}})
            mcp({"jsonrpc": "2.0", "method": "notifications/initialized"})
            tools = mcp({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
            names = [tool["name"] for tool in tools.get("result", {}).get("tools", [])]
            with open(os.path.join(state_dir, "tools"), "w") as file:
                file.write("\n".join(names))
        send({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": {
            "models": models(), "account": account(), "commands": []}}})
    elif subtype == "set_model":
        current["model"] = request.get("model")
        send({"type": "control_response", "response": {"subtype": "success", "request_id": request_id}})
    elif subtype == "apply_flag_settings":
        current["effort"] = request.get("settings", {}).get("effortLevel")
        send({"type": "control_response", "response": {"subtype": "success", "request_id": request_id}})
    elif subtype == "interrupt":
        interrupted.set()
        send({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": {"still_queued": []}}})
    elif subtype == "generate_session_title":
        if marked("title-fails"):
            send({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": {"title": None}}})
            return
        first = request.get("description", "").split("\n")[0].removeprefix("User: ")
        title = "Title: " + " ".join(first.split()[:3])
        send({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": {"title": title}}})
    elif subtype == "rename_session":
        send({"type": "control_response", "response": {"subtype": "success", "request_id": request_id}})
    else:
        send({"type": "control_response", "response": {"subtype": "error", "request_id": request_id, "error": f"unknown {subtype}"}})


turns = []
turn_ready = threading.Condition()


def turn_worker():
    while True:
        with turn_ready:
            while not turns:
                turn_ready.wait()
            turn, text = turns.pop(0)
        interrupted.clear()
        run_turn(turn, text)


threading.Thread(target=turn_worker, daemon=True).start()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    message = json.loads(line)
    kind = message.get("type")
    if kind == "control_response":
        response = message["response"]
        with response_ready:
            responses[response["request_id"]] = response
            response_ready.notify_all()
    elif kind == "control_request":
        threading.Thread(target=handle_control, args=(message,), daemon=True).start()
    elif kind == "user":
        text = "".join(part.get("text", "") for part in message["message"]["content"])
        with turn_ready:
            turns.append((message["uuid"], text))
            turn_ready.notify_all()
