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
# Qrow asks for a conversation title in an ephemeral thread that Codex never saves.
title_threads = set()


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


def generated_title(prompt):
    # Title the first user message so that the driver can tell which text Qrow sent.
    start = prompt.index('<message role="user">\n') + len('<message role="user">\n')
    words = prompt[start:prompt.index("\n</message>", start)].split()
    return "Title: " + " ".join(words[:3])


thread_id = None
turn_number = 0
pending_edit = None
pending_run = None
pending_workspace = None
pending_read_retry = None
unknown_tab_id = "00000000-0000-0000-0000-000000000001"


def wide_table():
    # The rows match a reply that the transcript cut off. The driver's table
    # check depends on these text widths.
    rows = [
        (2866700, "16:09:01", "yandex.org"),
        (2866682, "16:30:38", "sbscr"),
        (2866652, "16:13:34", "yandex.org"),
        (2866964, "16:38:26", "yandex.org"),
        (2866859, "16:06:59", "sbscr"),
        (2866801, "16:55:37", "yandex.org"),
        (2866661, "16:15:20", "google.org"),
        (2866651, "16:11:08", "yandex.org"),
        (2866799, "16:59:18", "yandex.org"),
        (2866762, "16:34:18", "google.org"),
    ]
    table = "".join(f"| {click} | 2010-02-19 {time} | gate | {marker} |\n" for click, time, marker in rows)
    return (
        "Ran a synthetic wide table query: `SELECT * FROM avia.clicks LIMIT 10;` on the Clicks tab. "
        "It returned 10 rows. Here\u2019s a compact preview; `LIMIT 10` does not guarantee row order.\n\n"
        "| click_id | created_at | type | marker |\n| --: | --- | --- | --- |\n" + table
    )


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
    elif method == "thread/start" and request["params"].get("ephemeral"):
        title_thread = f"synthetic-title-{len(title_threads) + 1}"
        title_threads.add(title_thread)
        send({"id": request_id, "result": {"thread": {"id": title_thread, "name": None, "updatedAt": 1, "turns": []}}})
    elif method == "turn/start" and request["params"]["threadId"] in title_threads:
        params = request["params"]
        title_turn = f"{params['threadId']}-turn"
        send({"id": request_id, "result": {"turn": {"id": title_turn, "status": "inProgress", "items": []}}})
        reply = {"type": "agentMessage", "text": json.dumps({"title": generated_title(params["input"][0]["text"])})}
        send({"method": "item/completed", "params": {"threadId": params["threadId"], "turnId": title_turn, "item": reply}})
        send(
            {
                "method": "turn/completed",
                "params": {"threadId": params["threadId"], "turn": {"id": title_turn, "status": "completed", "items": []}},
            }
        )
    elif method == "thread/unsubscribe":
        title_threads.discard(request["params"]["threadId"])
        send({"id": request_id, "result": {"status": "unsubscribed"}})
    elif method == "thread/name/set":
        send({"id": request_id, "result": {}})
        send(
            {
                "method": "thread/name/updated",
                "params": {"threadId": request["params"]["threadId"], "threadName": request["params"]["name"]},
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
    elif method == "turn/interrupt":
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
        if message.startswith(("Write SELECT 1", "Write SELECT 2")):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]
            pending_edit = turn_id
            query = "SELECT 1" if message.startswith("Write SELECT 1") else "SELECT 2"
            send(
                {
                    "id": 9000 + turn_number,
                    "method": "item/tool/call",
                    "params": {
                        "threadId": thread_id,
                        "turnId": turn_id,
                        "callId": f"edit-{turn_number}",
                        "tool": "append_selected_tab_sql",
                        "arguments": {
                            "version": 1,
                            "tab_id": tab["id"],
                            "connection_id": tab["connection_id"],
                            "editor_revision": tab["editor_revision"],
                            "sql": query,
                        },
                    },
                }
            )
        elif message.startswith("Append two SQL statements with edit tool"):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]
            pending_edit = turn_id
            end = len(tab["sql"].encode("utf-8"))
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
                            "edits": [{
                                "start": end,
                                "end": end,
                                "replacement": "\n\nSELECT 1;\n\nSELECT 2",
                            }],
                        },
                    },
                }
            )
        elif message.startswith("Run tab selected after rename"):
            marker = os.path.join(state_dir, "retarget-ready")
            deadline = time.monotonic() + 15
            while not os.path.exists(marker) and time.monotonic() < deadline:
                time.sleep(0.05)
            pending_read_retry = turn_id
            send(
                {
                    "id": 9000 + turn_number,
                    "method": "item/tool/call",
                    "params": {
                        "threadId": thread_id,
                        "turnId": turn_id,
                        "callId": f"read-{turn_number}",
                        "tool": "read_tab_sql",
                        "arguments": {"version": 1, "tab_id": unknown_tab_id},
                    },
                }
            )
        elif message.startswith(("Run selected SQL", "Run first SQL by range")):
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
                            **({"statement_range": {"start": 0, "end": len("SELECT 0;".encode("utf-8"))}}
                               if message.startswith("Run first SQL by range") else {}),
                        },
                    },
                }
            )
        else:
            answer = (
                "\n\n".join(f"Line {number}: synthetic assistant text" for number in range(1, 41))
                if message.startswith("Show many lines")
                else wide_table()
                if message.startswith("Show a wide table")
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
        and (pending_edit or pending_run or pending_workspace or pending_read_retry)
        and request_id == 9000 + turn_number
    ):
        turn_id = pending_edit or pending_run or pending_workspace or pending_read_retry
        text = request["result"]["contentItems"][0]["text"]
        result = json.loads(text)
        if pending_read_retry:
            pending_read_retry = None
            assert request["result"]["success"] and result["sql"] == "SELECT 99;"
            pending_workspace = turn_id
            send(
                {
                    "id": 9000 + turn_number,
                    "method": "item/tool/call",
                    "params": {
                        "threadId": thread_id,
                        "turnId": turn_id,
                        "callId": f"workspace-{turn_number}",
                        "tool": "get_workspace_context",
                        "arguments": {"version": 1},
                    },
                }
            )
            continue
        if pending_workspace:
            pending_workspace = None
            if request["result"]["success"]:
                tab = result["selected_tab"]
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
                                "tab_id": unknown_tab_id,
                                "connection_id": tab["connection_id"],
                                "editor_revision": tab["editor_revision"],
                            },
                        },
                    }
                )
                continue
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
