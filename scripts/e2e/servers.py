#!/usr/bin/env python3
"""Verified downloads and process control for the native (local Java) fixture runtime."""
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
import sys
import tarfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/core"))
from environment import jdk_problem, target_dir  # noqa: E402

FIXTURE = ROOT / "tests/fixture/server"
DOWNLOAD_REPORT_INTERVAL = 15
DOWNLOAD_TIMEOUT_SECONDS = 120 * 60
DOWNLOAD_WATCHDOG_SECONDS = DOWNLOAD_TIMEOUT_SECONDS + 60
DOWNLOAD_MINIMUM_RATE = 256 * 1024
DOWNLOAD_MINIMUM_RATE_SECONDS = 60
OUTPUT_LOCK = threading.Lock()


def announce(message):
    # stderr: qtest runs this in its own process, and `--json` owns stdout.
    with OUTPUT_LOCK:
        print(f"[native-e2e] {message}", file=sys.stderr, flush=True)


def human_size(size):
    value = float(size)
    for unit in ("B", "KiB", "MiB", "GiB"):
        if value < 1024 or unit == "GiB":
            return f"{value:.1f} {unit}"
        value /= 1024


def elapsed_seconds(started):
    return f"{time.monotonic() - started:.0f}s"


def human_duration(seconds):
    seconds = max(0, round(seconds))
    if seconds < 60:
        return f"{seconds}s"
    minutes, seconds = divmod(seconds, 60)
    if minutes < 60:
        return f"{minutes}m {seconds:02d}s"
    hours, minutes = divmod(minutes, 60)
    return f"{hours}h {minutes:02d}m"


def report_download_progress(label, received, total, started, initial_received=0):
    elapsed = max(time.monotonic() - started, 1)
    rate = max(received - initial_received, 0) / elapsed
    if total:
        percent = min(received / total * 100, 100)
        eta = human_duration((total - received) / rate) if rate else "unknown"
        progress = (f"{human_size(received)} / {human_size(total)} received ({percent:.1f}%; "
                    f"{human_size(rate)}/s; ETA {eta}; {elapsed:.0f}s elapsed)")
    else:
        progress = f"{human_size(received)} received ({human_size(rate)}/s; {elapsed:.0f}s elapsed; total unavailable)"
    announce(f"{label}: download in progress: {progress}.")


def preflight():
    if os.uname().sysname != "Darwin":
        raise RuntimeError("Native E2E tests require macOS")
    missing = [command for command in ("curl",) if shutil.which(command) is None]
    if missing:
        raise RuntimeError("Missing required command: " + ", ".join(missing))
    problem = jdk_problem()
    if problem:
        raise RuntimeError(problem)
    return Path(os.environ["JAVA_HOME"])


def transfer(label, url, partial, total, more_sources):
    initial_received = partial.stat().st_size if partial.exists() else 0
    if initial_received:
        announce(f"{label}: resuming download at {human_size(initial_received)} / {human_size(total)}.")
    command = ["curl", "--fail", "--silent", "--show-error", "--location", "--continue-at", "-",
               "--max-time", str(DOWNLOAD_TIMEOUT_SECONDS)]
    if more_sources:
        # Leave a slow source early; the next source continues the same partial file.
        command += ["--speed-limit", str(DOWNLOAD_MINIMUM_RATE), "--speed-time", str(DOWNLOAD_MINIMUM_RATE_SECONDS)]
    command += ["--output", str(partial), url]
    total_text = f" ({human_size(total)} total)" if total else ""
    announce(f"{label}: downloading {url}{total_text}.")
    started = time.monotonic()
    process = subprocess.Popen(command)
    try:
        next_report = started + DOWNLOAD_REPORT_INTERVAL
        while process.poll() is None:
            now = time.monotonic()
            if now >= next_report:
                received = partial.stat().st_size if partial.exists() else initial_received
                report_download_progress(label, received, total, started, initial_received)
                next_report = now + DOWNLOAD_REPORT_INTERVAL
            if now - started >= DOWNLOAD_WATCHDOG_SECONDS:
                raise subprocess.TimeoutExpired(command, DOWNLOAD_WATCHDOG_SECONDS)
            time.sleep(1)
        if process.returncode:
            raise subprocess.CalledProcessError(process.returncode, command)
        if total and partial.stat().st_size != total:
            raise ValueError(f"Size mismatch for {partial}: expected {total}, got {partial.stat().st_size}")
    except BaseException as error:
        if process.poll() is None:
            process.kill()
        process.wait()
        announce(f"{label}: download failed after {elapsed_seconds(started)}: {error}")
        raise
    announce(f"{label}: download complete: {human_size(partial.stat().st_size)} in {elapsed_seconds(started)}.")


def download_cache():
    return target_dir() / "e2e-downloads"


def distribution(item, label=None):
    label = label or item["directory"]
    total = item.get("bytes")
    cache = download_cache()
    cache.mkdir(parents=True, exist_ok=True)
    urls = item["urls"]
    archive = cache / urls[0].rsplit("/", 1)[1]
    if not archive.exists():
        partial = archive.with_suffix(".partial")
        if total and partial.exists() and partial.stat().st_size == total:
            announce(f"{label}: completed partial download found ({human_size(total)}).")
        else:
            if total and partial.exists() and partial.stat().st_size > total:
                announce(f"{label}: discarding oversized partial download ({human_size(partial.stat().st_size)}).")
                partial.unlink()
            for index, url in enumerate(urls):
                more_sources = index + 1 < len(urls)
                try:
                    transfer(label, url, partial, total, more_sources)
                    break
                except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                    if not more_sources:
                        raise
                    announce(f"{label}: trying the next download source.")
        partial.rename(archive)
    else:
        announce(f"{label}: using cached archive {archive.name} ({human_size(archive.stat().st_size)}).")
    if total and archive.stat().st_size != total:
        raise ValueError(f"Size mismatch for {archive}: expected {total}, got {archive.stat().st_size}")
    announce(f"{label}: verifying SHA-512 checksum.")
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha512").hexdigest()
    if digest != item["sha512"]:
        raise ValueError(f"Checksum mismatch: {archive}")
    announce(f"{label}: checksum verified.")
    destination = cache / item["directory"]
    if archive.suffix != ".jar" and not destination.exists():
        announce(f"{label}: extracting {archive.name}.")
        with tarfile.open(archive) as source:
            source.extractall(cache, filter="data")
        announce(f"{label}: extraction complete.")
    elif archive.suffix != ".jar":
        announce(f"{label}: using cached extracted directory {destination}.")
    return destination


def downloads():
    manifest = json.loads((ROOT / "tests/fixture/native-downloads.json").read_text())
    cache = download_cache()
    announce(f"Checking {len(manifest)} native fixture dependencies in {cache}.")
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        paths = list(executor.map(distribution, manifest.values(), manifest.keys()))
    announce("Native fixture dependencies are ready.")
    return dict(zip(manifest, paths))


class Servers:
    def __init__(self, artifacts):
        self.artifacts = artifacts
        self.processes = []
        self.logs = []

    def start(self, name, args, env):
        path = self.artifacts / f"{name}.log"
        announce(f"Starting {name} (log: {path}).")
        output = path.open("w")
        self.logs.append(output)
        process = subprocess.Popen(args, cwd=self.artifacts, env=env, stdout=output,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        self.processes.append((name, process))
        announce(f"Started {name} (pid {process.pid}).")

    def check(self):
        for name, process in self.processes:
            if process.poll() is not None:
                raise RuntimeError(f"{name} exited: {(self.artifacts / f'{name}.log').read_text()[-4000:]}")

    def stop(self):
        # Include Spark executors and Kyuubi engines, not only their parent JVMs.
        if not self.processes:
            return
        announce("Stopping native fixture server process groups.")
        for _, process in reversed(self.processes):
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + 10
        for _, process in reversed(self.processes):
            try:
                process.wait(timeout=max(0.01, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                pass
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)
        for output in self.logs:
            output.close()
        announce("Native fixture server process groups stopped.")
