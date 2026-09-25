#!/usr/bin/env python3
"""Deterministic Codex app-server fixture for the native Qrow UI test."""

import json
import sys

THREAD = "synthetic-thread-1"
turn_number = 0
pending_edit = None


def send(message):
    print(json.dumps(message, separators=(",", ":")), flush=True)


for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    request_id = request.get("id")
    if method == "initialize":
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
        send(
            {
                "id": request_id,
                "result": {
                    "thread": {
                        "id": THREAD,
                        "name": None,
                        "updatedAt": 1,
                        "turns": [],
                    }
                },
            }
        )
    elif method in {"thread/delete", "thread/name/set", "turn/interrupt"}:
        send({"id": request_id, "result": {}})
    elif method == "thread/items/list":
        send({"id": request_id, "result": {"data": [], "nextCursor": None}})
    elif method == "turn/steer":
        send({"id": request_id, "result": {"turnId": f"synthetic-turn-{turn_number}"}})
    elif method == "turn/start":
        turn_number += 1
        turn_id = f"synthetic-turn-{turn_number}"
        params = request["params"]
        message = params["input"][0]["text"]
        send(
            {
                "id": request_id,
                "result": {
                    "turn": {"id": turn_id, "status": "inProgress", "items": []}
                },
            }
        )
        if message.startswith("Write SELECT 1"):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]
            pending_edit = turn_id
            send(
                {
                    "id": 9000 + turn_number,
                    "method": "item/tool/call",
                    "params": {
                        "threadId": THREAD,
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
        else:
            send(
                {
                    "method": "item/agentMessage/delta",
                    "params": {
                        "threadId": THREAD,
                        "turnId": turn_id,
                        "delta": "I can help with this query.",
                    },
                }
            )
            send(
                {
                    "method": "turn/completed",
                    "params": {
                        "threadId": THREAD,
                        "turn": {"id": turn_id, "status": "completed", "items": []},
                    },
                }
            )
    elif request_id is not None and pending_edit and request_id == 9000 + turn_number:
        turn_id = pending_edit
        text = request["result"]["contentItems"][0]["text"]
        result = json.loads(text)
        message = (
            "I updated the SQL."
            if request["result"]["success"]
            else f"Edit failed: {result}"
        )
        send(
            {
                "method": "item/agentMessage/delta",
                "params": {
                    "threadId": THREAD,
                    "turnId": turn_id,
                    "delta": message,
                },
            }
        )
        send(
            {
                "method": "turn/completed",
                "params": {
                    "threadId": THREAD,
                    "turn": {"id": turn_id, "status": "completed", "items": []},
                },
            }
        )
        pending_edit = None
