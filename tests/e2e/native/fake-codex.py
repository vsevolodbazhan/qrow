#!/usr/bin/env python3
"""Deterministic Codex app-server fixture for the native Qrow UI test."""

import json
import os
import sys
import time

# Like Codex, save a thread only after its first turn. The state directory
# keeps thread IDs unique and saved threads resumable across app restarts.
state_dir = os.path.join(os.environ["QROW_DATA_DIR"], "fake-codex")
os.makedirs(os.path.join(state_dir, "rollouts"), exist_ok=True)
counter_path = os.path.join(state_dir, "thread-counter")
live_threads = set()


def has_rollout(thread):
    return thread in live_threads or os.path.exists(os.path.join(state_dir, "rollouts", thread))


def save_rollout(thread):
    open(os.path.join(state_dir, "rollouts", thread), "w").close()


def missing_rollout(thread):
    return {"code": -32600, "message": f"no rollout found for thread id {thread}"}


def next_thread_number():
    try:
        with open(counter_path) as counter:
            number = int(counter.read()) + 1
    except FileNotFoundError:
        number = 1
    with open(counter_path, "w") as counter:
        counter.write(str(number))
    return number


thread_id = None
turn_number = 0
pending_edit = None
pending_run = None


def send(message):
    print(json.dumps(message, separators=(",", ":")), flush=True)


for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    request_id = request.get("id")
    if method == "initialize":
        time.sleep(2)  # Keep the startup controls visible long enough for UI checks.
        send({"id": request_id, "result": {}})
    elif method == "account/read":
        send(
            {
                "id": request_id,
                "result": {
                    "account": {"type": "chatgpt", "planType": "plus"},
                    "requiresOpenaiAuth": True,
                },
            }
        )
    elif method == "model/list":
        send(
            {
                "id": request_id,
                "result": {
                    "data": [
                        {
                            "id": "synthetic-model",
                            "displayName": "Synthetic Model",
                            "description": "Test model",
                            "isDefault": True,
                            "defaultReasoningEffort": "medium",
                            "supportedReasoningEfforts": [
                                {"reasoningEffort": "medium", "description": "Balanced"}
                            ],
                            "defaultServiceTier": "standard",
                            "serviceTiers": [
                                {
                                    "id": "standard",
                                    "name": "Standard",
                                    "description": "Normal speed",
                                },
                                {
                                    "id": "fast",
                                    "name": "Fast",
                                    "description": "Lower latency",
                                },
                            ],
                        }
                    ],
                    "nextCursor": None,
                },
            }
        )
    elif method in {"thread/start", "thread/resume", "thread/read"}:
        if method == "thread/start":
            thread_id = f"synthetic-thread-{next_thread_number()}"
            live_threads.add(thread_id)
        elif not has_rollout(request["params"]["threadId"]):
            send({"id": request_id, "error": missing_rollout(request["params"]["threadId"])})
            continue
        else:
            thread_id = request["params"]["threadId"]
            live_threads.add(thread_id)
        send(
            {
                "id": request_id,
                "result": {
                    "thread": {
                        "id": thread_id,
                        "name": None,
                        "updatedAt": 1,
                        "turns": [],
                    }
                },
            }
        )
    elif method == "thread/delete":
        # Test deletion of a conversation whose Codex history is missing.
        send({"id": request_id, "error": missing_rollout(request["params"]["threadId"])})
    elif method in {"thread/name/set", "turn/interrupt"}:
        send({"id": request_id, "result": {}})
    elif method == "thread/items/list":
        send({"id": request_id, "result": {"data": [], "nextCursor": None}})
    elif method == "turn/steer":
        send({"id": request_id, "result": {"turnId": f"synthetic-turn-{turn_number}"}})
    elif method == "turn/start":
        params = request["params"]
        if params["threadId"] not in live_threads:
            send({"id": request_id, "error": missing_rollout(params["threadId"])})
            continue
        thread_id = params["threadId"]
        save_rollout(thread_id)
        turn_number += 1
        turn_id = f"synthetic-turn-{turn_number}"
        message = params["input"][0]["text"]
        send(
            {
                "id": request_id,
                "result": {
                    "turn": {"id": turn_id, "status": "inProgress", "items": []}
                },
            }
        )
        if message.startswith("Return to the latest message"):
            time.sleep(1)
        if message.startswith("Write SELECT 1"):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]
            pending_edit = turn_id
            send(
                {
                    "id": 9000 + turn_number,
                    "method": "item/tool/call",
                    "params": {
                        "threadId": thread_id,
                        "turnId": turn_id,
                        "callId": f"edit-{turn_number}",
                        "tool": "edit_selected_tab_sql",
                        "arguments": {
                            "version": 1,
                            "tab_id": tab["id"],
                            "connection_id": tab["connection_id"],
                            "editor_revision": tab["editor_revision"],
                            "edits": [
                                {
                                    "start": 0,
                                    "end": len(tab["sql"].encode("utf-8")),
                                    "replacement": "SELECT 1",
                                }
                            ],
                        },
                    },
                }
            )
        elif message.startswith("Run selected SQL"):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]
            pending_run = turn_id
            send(
                {
                    "id": 9000 + turn_number,
                    "method": "item/tool/call",
                    "params": {
                        "threadId": thread_id,
                        "turnId": turn_id,
                        "callId": f"run-{turn_number}",
                        "tool": "run_selected_tab_query",
                        "arguments": {
                            "version": 1,
                            "tab_id": tab["id"],
                            "connection_id": tab["connection_id"],
                            "editor_revision": tab["editor_revision"],
                        },
                    },
                }
            )
        else:
            answer = (
                "\n\n".join(f"Line {number}: synthetic assistant text" for number in range(1, 41))
                if message.startswith("Show many lines")
                else (
                    "**I can help with this query.**\n\n"
                    "Use `SELECT 1` to check the selected tab.\n\n"
                    "| Column | Value |\n| --- | --- |\n| Result | 1 |"
                )
            )
            send(
                {
                    "method": "item/agentMessage/delta",
                    "params": {
                        "threadId": thread_id,
                        "turnId": turn_id,
                        "delta": answer,
                    },
                }
            )
            send(
                {
                    "method": "turn/completed",
                    "params": {
                        "threadId": thread_id,
                        "turn": {"id": turn_id, "status": "completed", "items": []},
                    },
                }
            )
    elif (
        request_id is not None
        and (pending_edit or pending_run)
        and request_id == 9000 + turn_number
    ):
        turn_id = pending_edit or pending_run
        text = request["result"]["contentItems"][0]["text"]
        result = json.loads(text)
        if request["result"]["success"]:
            message = "I updated the SQL." if pending_edit else "I ran the query."
        else:
            message = f"Tool failed: {result}"
        send(
            {
                "method": "item/agentMessage/delta",
                "params": {
                    "threadId": thread_id,
                    "turnId": turn_id,
                    "delta": message,
                },
            }
        )
        send(
            {
                "method": "turn/completed",
                "params": {
                    "threadId": thread_id,
                    "turn": {"id": turn_id, "status": "completed", "items": []},
                },
            }
        )
        pending_edit = None
        pending_run = None
