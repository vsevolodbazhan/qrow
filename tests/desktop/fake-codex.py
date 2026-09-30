#!/usr/bin/env python3
"""Deterministic Codex app-server fixture for the native Qrow UI test."""

import json
import os
import sys
import threading
import time

# Like Codex, save a thread only after its first turn. The state directory
# keeps thread IDs unique and saved threads resumable across app restarts.
state_dir = os.path.join(os.environ["QROW_DATA_DIR"], "fake-codex")
os.makedirs(os.path.join(state_dir, "rollouts"), exist_ok=True)
counter_path = os.path.join(state_dir, "thread-counter")
live_threads = set()
# Qrow asks for a conversation title in an ephemeral thread that Codex never saves.
title_threads = set()
# The driver creates this file to start without an account.
signed_out_path = os.path.join(state_dir, "signed-out")


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
pending_message = ""
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


def bold_reply():
    # The first paragraph matches a reply that the transcript cut off at its
    # right edge. Inline code makes Markdown lay out a paragraph as an inline
    # flow. There, every line of the bold paragraph overflows if the line
    # wrapper measures bold text in the regular face.
    sentence = "Bold words wrap inside the transcript so that no letter is hidden at the right edge."
    return (
        "There were **44,266,382 distinct searches** in `avia.searches` for the last seven "
        "complete days, September 20–26, 2026.\n\n"
        f"`pdate` **{' '.join([sentence] * 4)}**"
    )


send_lock = threading.Lock()


def send(message):
    with send_lock:
        print(json.dumps(message, separators=(",", ":")), flush=True)


# Held turns run in their own threads, like turns of several Codex threads.
# Each tool call has a handler for its answer.
call_handlers = {}
answered_calls = set()
next_call_id = [20000]
handlers_lock = threading.Lock()


def call_tool(thread, turn, tool, arguments, handler, replay=False):
    with handlers_lock:
        next_call_id[0] += 1
        call_id = next_call_id[0]
        call_handlers[call_id] = handler
    call = {
        "id": call_id,
        "method": "item/tool/call",
        "params": {
            "threadId": thread,
            "turnId": turn,
            "callId": f"held-{call_id}",
            "tool": tool,
            "arguments": arguments,
        },
    }
    send(call)
    if replay:
        # Codex sends a waiting call again when a client resumes its thread.
        send(call)


def wait_for_marker(name, seconds=60):
    """Waits until the test creates the marker file `name`, or for `seconds`."""
    marker = os.path.join(state_dir, name)
    deadline = time.monotonic() + seconds
    while not os.path.exists(marker) and time.monotonic() < deadline:
        time.sleep(0.05)


def finish_turn(thread, turn, text, marker=None):
    def finish():
        if marker:
            wait_for_marker(marker)
        send({"method": "item/agentMessage/delta", "params": {"threadId": thread, "turnId": turn, "delta": text}})
        send(
            {
                "method": "turn/completed",
                "params": {"threadId": thread, "turn": {"id": turn, "status": "completed", "items": []}},
            }
        )

    threading.Thread(target=finish, daemon=True).start()


def hold_turn(thread, turn, label, tab):
    """Waits for the driver, appends a query to the conversation tab, and asks to run it."""
    wait_for_marker(f"release-{label}")
    sql = {"Alpha": "SELECT 11", "Beta": "SELECT 22"}[label]

    def appended(success, result):
        if not success:
            finish_turn(thread, turn, f"Tool failed: {result}")
            return
        arguments = {
            "version": 1,
            "tab_id": result["tab_id"],
            "connection_id": tab["connection_id"],
        }
        if label == "Beta":
            arguments["editor_revision"] = result["editor_revision"]
        call_tool(thread, turn, "run_selected_tab_query", arguments, ran, replay=True)

    def ran(success, result):
        outcome = "ran" if success else result.get("error", {}).get("code", "failed")
        # The test selects another conversation before the Beta turn ends.
        finish_turn(thread, turn, f"Finished {label}: {outcome}", marker="finish-Beta" if label == "Beta" else None)

    arguments = {
        "version": 1,
        "tab_id": tab["id"],
        "connection_id": tab["connection_id"],
        "editor_revision": tab["editor_revision"],
        "sql": sql,
    }
    call_tool(thread, turn, "append_selected_tab_sql", arguments, appended)


def sign_in_elsewhere():
    # Like a sign-in in another Codex client after a failed sign-in in Qrow.
    wait_for_marker("sign-in-elsewhere", 30)
    os.remove(signed_out_path)
    send({"method": "account/updated", "params": {"authMode": "chatgpt", "planType": "plus"}})


for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    request_id = request.get("id")
    if method is None and request_id in call_handlers:
        with handlers_lock:
            if request_id in answered_calls:
                # Qrow must answer a replayed call only once.
                open(os.path.join(state_dir, "duplicate-answer"), "w").close()
                continue
            answered_calls.add(request_id)
            handler = call_handlers[request_id]
        result = request["result"]
        handler(result["success"], json.loads(result["contentItems"][0]["text"]))
        continue
    if method == "initialize":
        if os.path.exists(os.path.join(state_dir, "hold-initialize")):
            # Keep the startup controls visible through the UI checks.
            wait_for_marker("initialize-release")
        send({"id": request_id, "result": {}})
    elif method == "account/read":
        signed_out = os.path.exists(signed_out_path)
        send(
            {
                "id": request_id,
                "result": {
                    "account": None if signed_out else {"type": "chatgpt", "planType": "plus"},
                    "requiresOpenaiAuth": True,
                },
            }
        )
    elif method == "account/login/start":
        # A sign-in URL would open the browser, so the sign-in fails before that.
        send({"id": request_id, "error": {"code": -32603, "message": "Login server error: port 1455 is in use"}})
        threading.Thread(target=sign_in_elsewhere, daemon=True).start()
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
        prompt = params["input"][0]["text"]
        if "Hold title generation" in prompt:
            open(os.path.join(state_dir, "title-generation-pending"), "w").close()
            wait_for_marker("title-generation-release")
        fail_once = os.path.join(state_dir, "title-failure-once")
        if "Fail title generation" in prompt and not os.path.exists(fail_once):
            open(fail_once, "w").close()
            title_reply = "No title"
        else:
            title_reply = json.dumps({"title": generated_title(prompt)})
        reply = {"type": "agentMessage", "text": title_reply}
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
        pending_message = message
        send(
            {
                "id": request_id,
                "result": {
                    "turn": {"id": turn_id, "status": "inProgress", "items": []}
                },
            }
        )
        if message.startswith("Hold parallel "):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            label = message.split()[2]
            threading.Thread(
                target=hold_turn, args=(thread_id, turn_id, label, context["selected_tab"]), daemon=True
            ).start()
            continue
        if message.startswith("Append and run SQL without revision"):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]

            def ran_append_query(success, result):
                finish_turn(thread_id, turn_id, "I ran the appended query." if success else f"Tool failed: {result}")

            def appended_for_run(success, result):
                if not success:
                    finish_turn(thread_id, turn_id, f"Tool failed: {result}")
                    return
                call_tool(thread_id, turn_id, "run_selected_tab_query", {
                    "version": 1,
                    "tab_id": result["tab_id"],
                    "connection_id": tab["connection_id"],
                }, ran_append_query)

            call_tool(thread_id, turn_id, "append_selected_tab_sql", {
                "version": 1,
                "tab_id": tab["id"],
                "connection_id": tab["connection_id"],
                "editor_revision": tab["editor_revision"],
                "sql": "-- Assistant value\nSELECT 3 AS assistant_value",
            }, appended_for_run)
            continue
        if message.startswith("Title before first reply"):
            open(os.path.join(state_dir, "first-reply-pending"), "w").close()

            def release_first_reply():
                wait_for_marker("first-reply-release")
                finish_turn(thread_id, turn_id, "I can help with this query.")

            threading.Thread(target=release_first_reply, daemon=True).start()
            continue
        if message.startswith("Return to the latest message"):
            # Keep the turn open until the test observed the working indicator.
            wait_for_marker("latest-release")
        if message.startswith("Disconnect Codex"):
            # Stop during the turn, like a Codex crash.
            sys.exit(0)
        if message.startswith("Write query with another tab ID"):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]
            other = next(item for item in context["tabs"] if item["id"] != tab["id"])
            wait_for_marker("wrong-tab-ready", 15)
            pending_edit = turn_id
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
                            "tab_id": other["id"],
                            "connection_id": other["connection_id"],
                            "editor_revision": tab["editor_revision"],
                            "sql": "-- Tables in dwh_meta\nSHOW TABLES IN dwh_meta",
                        },
                    },
                }
            )
        elif message.startswith(("Write SELECT 1", "Write SELECT 2", "Write a long query")):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]
            pending_edit = turn_id
            if message.startswith("Write a long query"):
                # Qrow formats a long query in its own layout, also when the
                # model wrote it on several lines. The comment stays as written.
                query = (
                    "-- Bookings by state\n"
                    "SELECT state, COUNT(*) AS bookings, MAX(booked_at) AS last_booked_at\n"
                    "FROM integrations.bookings GROUP BY state;"
                )
            else:
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
        elif message.startswith("Append then retarget and run without revision"):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]

            def rejected_implicit_run(success, result):
                code = result.get("error", {}).get("code")
                finish_turn(thread_id, turn_id, f"Implicit run rejected: {code}" if not success else "Implicit run unexpectedly accepted")

            def retargeted_run(success, result):
                if success or result.get("error", {}).get("code") != "approval_cancelled":
                    finish_turn(thread_id, turn_id, f"Retarget failed: {result}")
                    return
                call_tool(thread_id, turn_id, "run_selected_tab_query", {
                    "version": 1,
                    "tab_id": tab["id"],
                    "connection_id": tab["connection_id"],
                }, rejected_implicit_run)

            def appended_before_retarget(success, result):
                if not success:
                    finish_turn(thread_id, turn_id, f"Append failed: {result}")
                    return
                call_tool(thread_id, turn_id, "run_selected_tab_query", {
                    "version": 1,
                    "tab_id": tab["id"],
                    "connection_id": tab["connection_id"],
                    "editor_revision": result["editor_revision"],
                    "statement_range": tab["statement_ranges"][0],
                }, retargeted_run)

            call_tool(thread_id, turn_id, "append_selected_tab_sql", {
                "version": 1,
                "tab_id": tab["id"],
                "connection_id": tab["connection_id"],
                "editor_revision": tab["editor_revision"],
                "sql": "-- Third statement\nSELECT 3",
            }, appended_before_retarget)
        elif message.startswith("Rewrite the last statement with edit tool"):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]
            pending_edit = turn_id
            # The driver seeds lowercase keywords and a tab size of 4.
            rewrite_style = context["sql_style"]
            last = tab["statement_ranges"][-1]
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
                                "start": last["start"],
                                "end": last["end"],
                                # Models often write a query on one line. Qrow formats it.
                                "replacement": (
                                    "SELECT state, COUNT(*) AS bookings, MAX(booked_at) AS "
                                    "last_booked_at FROM integrations.bookings GROUP BY state"
                                ),
                            }],
                        },
                    },
                }
            )
        elif message.startswith("Run tab selected after rename"):
            wait_for_marker("retarget-ready", 15)
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
        elif message.startswith("Run 170 rows and read results"):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]
            read_ids = set()
            omitted = []

            def read_page(offset):
                def received(success, result):
                    if not success:
                        finish_turn(thread_id, turn_id, f"Tool failed: {result}")
                        return
                    next_offset = result["next_offset"]
                    if next_offset <= offset:
                        finish_turn(thread_id, turn_id, f"Read results stalled at {offset}")
                        return
                    read_ids.update(int(row[0]) for row in result["rows"])
                    omitted.extend(result.get("omitted_row_offsets", []))
                    if result["more_downloaded_rows"]:
                        read_page(next_offset)
                    elif next_offset == 170 and read_ids == set(range(170)) - {25} and omitted == [25]:
                        finish_turn(thread_id, turn_id, "Read all 170 row positions.")
                    else:
                        finish_turn(thread_id, turn_id, f"Bad result paging: {next_offset}, {len(read_ids)}, {omitted}")

                call_tool(thread_id, turn_id, "read_results", {
                    "version": 1, "tab_id": tab["id"], "offset": offset, "count": 100,
                }, received)

            def ran(success, result):
                if not success or result.get("downloaded_rows") != 170:
                    finish_turn(thread_id, turn_id, f"Tool failed: {result}")
                    return
                read_ids.update(int(row[0]) for row in result["rows"])
                omitted.extend(result.get("omitted_row_offsets", []))
                read_page(result["next_offset"])

            call_tool(thread_id, turn_id, "run_selected_tab_query", {
                "version": 1, "tab_id": tab["id"],
                "connection_id": tab["connection_id"],
                "editor_revision": tab["editor_revision"],
            }, ran)
        elif message.startswith(("Run selected SQL", "Run first SQL by range")):
            context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
            tab = context["selected_tab"]
            if message.startswith("Run first SQL by range"):
                assert tab["statement_ranges"] == [
                    {"start": 0, "end": 9},
                    {"start": 11, "end": 20},
                    {"start": 22, "end": 30},
                ]
                assert tab["statement_ranges_truncated"] is False
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
                            **({"statement_range": tab["statement_ranges"][0]}
                               if message.startswith("Run first SQL by range") else {}),
                        },
                    },
                }
            )
        else:
            if message.startswith(("Report the SQL style", "Report the tab SQL")):
                context = json.loads(params["additionalContext"]["qrow_workspace"]["value"])
                style = context["sql_style"]
            answer = (
                f"SQL style: {style['keyword_case']}, {style['indent_spaces']} spaces"
                if message.startswith("Report the SQL style")
                else f"Tab SQL: {context['selected_tab']['sql']}"
                if message.startswith("Report the tab SQL")
                else "\n\n".join(f"Line {number}: synthetic assistant text" for number in range(1, 41))
                if message.startswith("Show many lines")
                else wide_table()
                if message.startswith("Show a wide table")
                else bold_reply()
                if message.startswith("Show a bold reply")
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
            # An unknown tab ID reads the conversation tab, not the tab that the
            # user selected during the turn.
            assert request["result"]["success"] and result["sql"] == "SELECT 1;"
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
        if not request["result"]["success"]:
            message = f"Tool failed: {result}"
        elif pending_run:
            # A finished query returns its first rows, so the model needs no read_results call.
            expected = {"with approval": [["1"]], "automatically": [["2"]]}
            rows = next((rows for key, rows in expected.items() if key in pending_message), None)
            if "next_offset" not in result or (rows is not None and result.get("rows") != rows):
                message = f"Tool failed: the query result has no expected rows: {result}"
            else:
                message = "I ran the query."
        elif pending_message.startswith("Append two SQL statements with edit tool"):
            message = (
                "I updated the SQL."
                if result.get("selected_range") == {"start": 22, "end": 30}
                else f"Tool failed: the edited statement was not selected: {result}"
            )
        elif pending_message.startswith("Rewrite the last statement with edit tool"):
            expected_style = {"keyword_case": "lowercase", "indent_spaces": 4}
            message = (
                f"Tool failed: the context has SQL style {rewrite_style}"
                if rewrite_style != expected_style
                else "I formatted the SQL."
                if result.get("formatted") is True
                else f"Tool failed: the rewritten query was not formatted: {result}"
            )
        elif pending_message.startswith("Write a long query"):
            # The selected range covers the formatted statement, not its comment.
            statement = (
                "SELECT\n  state,\n  COUNT(*) AS bookings,\n  MAX(booked_at) AS last_booked_at\n"
                "FROM integrations.bookings\nGROUP BY state;"
            )
            sql_bytes = result.get("sql_bytes", 0)
            message = (
                "I formatted the SQL."
                if result.get("formatted") is True
                and result.get("statement_range")
                == {"start": sql_bytes - len(statement), "end": sql_bytes}
                else f"Tool failed: the long query was not formatted or selected: {result}"
            )
        elif result.get("statement_range") is not None:
            message = "I updated the SQL."
        else:
            message = f"Tool failed: the edit result has no range: {result}"
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
