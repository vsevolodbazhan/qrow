#!/usr/bin/env python3
"""Launch time, idle memory, and idle CPU of the release app.

Starts target/release/qrow with a temporary workspace that has one synthetic
connection and an indented query, waits until it reports that its UI is ready,
lets it idle until the caret of the focused editor stops blinking and Qrow
returns its free memory, and samples it with ps and footprint. The physical
footprint is the memory that Activity Monitor shows. The first launch after a
build is slower while macOS checks the new executable, so one launch warms up
and the median of the next launches counts. Prints one QROW_PERF line for each
measurement and exits with 1 above a budget.
"""
import json
import os
from pathlib import Path
import signal
import statistics
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "core"))
from environment import target_dir  # noqa: E402

READY = "Qrow GPUI initialized"
# The caret blinks for 10 seconds after the editor gets focus, and each blink
# repaints the window. 5 seconds after the window stops drawing, Qrow returns
# free malloc pages to macOS one time.
IDLE_SECONDS = 18
SAMPLE_SECONDS = 5
# Large regressions only; `./qtest compare` measures small ones.
BUDGETS = {"app.launch_to_ui": 1500, "app.idle_rss": 400, "app.idle_footprint": 400, "app.idle_cpu": 5}
LAUNCHES = 3
PROFILE_ID = "00000000-0000-4000-8000-000000000001"
WORKSPACE = {
    "version": 4,
    "profiles": [{
        "id": PROFILE_ID, "name": "perf", "host": "kyuubi.example.invalid", "port": 10009,
        "username": "synthetic", "database": "default", "parameters": {},
        "lifecycle": {"idle_seconds": 900, "keep_alive_seconds": 30, "keep_alive_sql": "SELECT 1"},
    }],
    "tabs": [{
        "id": "00000000-0000-4000-8000-000000000002", "title": "Query 1", "profile": PROFILE_ID,
        "sql": "select *\nfrom\n    (\n        select\n            src,\n            avg(rate) over (\n"
               "                partition by src\n                order by pdate\n            ) as rate\n"
               "        from rates\n    )\nwhere rate > 0.1\n",
    }],
    "active_tab": 0,
}


def cpu_seconds(pid):
    """Cumulative CPU time of the process, from ps (m:ss.cc or h:mm:ss)."""
    text = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    seconds = 0.0
    for part in text.split(":"):
        seconds = seconds * 60 + float(part)
    return seconds


def rss_megabytes(pid):
    text = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return int(text) / 1024


def footprint_megabytes(pid):
    text = subprocess.run(["footprint", "-f", "bytes", "--noCategories", str(pid)],
                          capture_output=True, text=True).stdout
    for line in text.splitlines():
        name, _, value = line.strip().partition(": ")
        if name == "phys_footprint":
            return int(value.split()[0]) / 1024 / 1024
    raise RuntimeError(f"footprint did not report phys_footprint of {pid}")


def measure(executable):
    with tempfile.TemporaryDirectory() as directory:
        (Path(directory) / "workspace.json").write_text(json.dumps(WORKSPACE))
        env = dict(os.environ, QROW_DATA_DIR=directory)
        started = time.monotonic()
        process = subprocess.Popen([str(executable)], env=env, stderr=subprocess.PIPE, text=True,
                                   start_new_session=True)
        try:
            deadline = started + 60
            for line in process.stderr:
                if READY in line:
                    break
                if time.monotonic() > deadline:
                    raise RuntimeError("Qrow did not report a ready UI")
            else:
                raise RuntimeError(f"Qrow exited with {process.wait()} before its UI was ready")
            launch = (time.monotonic() - started) * 1000
            time.sleep(IDLE_SECONDS)
            before = cpu_seconds(process.pid)
            time.sleep(SAMPLE_SECONDS)
            cpu = (cpu_seconds(process.pid) - before) / SAMPLE_SECONDS * 100
            rss = rss_megabytes(process.pid)
            footprint = footprint_megabytes(process.pid)
        finally:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=10)
    return {"app.launch_to_ui": (launch, "ms"), "app.idle_rss": (rss, "MB"),
            "app.idle_footprint": (footprint, "MB"), "app.idle_cpu": (cpu, "%")}


def main():
    executable = target_dir() / "release/qrow"
    measure(executable)
    runs = [measure(executable) for _ in range(LAUNCHES)]
    over = []
    for probe, (_, unit) in runs[0].items():
        value = statistics.median(run[probe][0] for run in runs)
        budget = BUDGETS[probe]
        print("QROW_PERF " + json.dumps({"probe": probe, "value": value, "unit": unit, "budget": budget}), flush=True)
        if value > budget:
            over.append(f"{probe} is {value:.2f} {unit}, over its budget of {budget} {unit}")
    if over:
        sys.exit("\n".join(over))


if __name__ == "__main__":
    main()
