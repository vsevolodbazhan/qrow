#!/usr/bin/env python3
"""Launch time, idle memory, and idle CPU of the release app.

Starts target/release/qrow with an empty temporary workspace, waits until it
reports that its UI is ready, lets it idle until the caret of the focused
editor stops blinking, and samples it with ps. The first launch after a build
is slower while macOS checks the new executable, so one launch warms up and
the median of the next launches counts. Prints one QROW_PERF line for each
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

ROOT = Path(__file__).resolve().parents[2]
READY = "Qrow GPUI initialized"
# The caret blinks for 10 seconds after the editor gets focus, and each blink
# repaints the window.
IDLE_SECONDS = 12
SAMPLE_SECONDS = 5
# Large regressions only; `./qtest compare` measures small ones.
BUDGETS = {"app.launch_to_ui": 1500, "app.idle_rss": 400, "app.idle_cpu": 5}
LAUNCHES = 3


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


def measure(executable):
    with tempfile.TemporaryDirectory() as directory:
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
        finally:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=10)
    return {"app.launch_to_ui": (launch, "ms"), "app.idle_rss": (rss, "MB"), "app.idle_cpu": (cpu, "%")}


def main():
    executable = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release/qrow"
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
