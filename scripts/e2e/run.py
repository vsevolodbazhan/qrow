#!/usr/bin/env python3
"""Disposable real-server acceptance tests. No saved profiles or production credentials."""
import argparse
import io
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
COMPOSE = ROOT / "tests/e2e/compose.yml"
COMMAND_HEARTBEAT_SECONDS = 30
OUTPUT_LOCK = threading.Lock()


def announce(message):
    with OUTPUT_LOCK:
        print(f"[e2e] {message}", flush=True)


def command_text(args):
    return shlex.join(str(arg) for arg in args)


def run(args, *, timeout=180, check=True, capture=False):
    return subprocess.run(args, cwd=ROOT, check=check, timeout=timeout,
                          text=True, capture_output=capture)


def require_commands(*commands):
    missing = [command for command in commands if shutil.which(command) is None]
    if missing:
        raise RuntimeError("Missing required command: " + ", ".join(missing))


def compose(*args, **kwargs):
    project = os.environ.get("QROW_E2E_PROJECT", "")
    if not re.fullmatch(r"qrow-e2e-[a-z0-9-]+", project):
        raise ValueError("QROW_E2E_PROJECT must identify a disposable qrow-e2e-* project")
    return run(["docker", "compose", "-f", str(COMPOSE), "-p", project, *args], **kwargs)


def ready():
    # Readiness is an authenticated SQL round trip; an open port is insufficient.
    deadline = time.monotonic() + 180
    started = deadline - 180
    attempt = 0
    last_report = started
    announce("Waiting for authenticated Kyuubi SQL readiness (timeout: 180s).")
    while True:
        now = time.monotonic()
        if now >= deadline:
            break
        attempt += 1
        result = compose("exec", "-T", "kyuubi", "/opt/kyuubi/bin/beeline",
                         "-u", "jdbc:hive2://localhost:10009/default", "-n", "qrow",
                         "-p", "qrow-test-password", "-e", "SELECT 1",
                         timeout=150, check=False, capture=True)
        if result.returncode == 0:
            announce(f"Kyuubi is ready after {time.monotonic() - started:.0f}s (attempt {attempt}).")
            return
        if now - last_report >= 15:
            announce(f"Still waiting for Kyuubi readiness after {now - started:.0f}s "
                     f"(attempt {attempt}, last exit code {result.returncode}).")
            last_report = now
        time.sleep(2)
    raise RuntimeError("Kyuubi never became ready for authenticated SQL: " + result.stderr[-4000:])


def observe(action, token):
    if action == "count" and os.environ.get("QROW_E2E_NATIVE_EVIDENCE"):
        print(native_evidence_count(Path(os.environ["QROW_E2E_NATIVE_EVIDENCE"]), token))
        return
    if action == "count":
        if not re.fullmatch(r"[a-zA-Z0-9_-]+\.(started|interrupted|completed|ended)", token):
            raise ValueError("Invalid evidence filename")
        if token.endswith(".ended"):
            reference = compose("exec", "-T", "spark-worker", "cat", "/evidence/" + token.removesuffix(".ended") + ".task", capture=True, timeout=5).stdout.strip()
            if not re.fullmatch(r"app-[a-zA-Z0-9_-]+", reference):
                raise ValueError("Invalid Spark task reference")
            token = reference + ".ended"
        result = compose("exec", "-T", "spark-worker", "sh", "-c",
                         'if [ -f "$1" ]; then wc -l < "$1"; else echo 0; fi',
                         "sh", "/evidence/" + token, capture=True, timeout=5)
        print(result.stdout.strip())
    elif action == "kill-engine":
        compose("exec", "-T", "kyuubi", "pkill", "-9", "-f",
                "[o]rg.apache.kyuubi.engine.spark.SparkSQLEngine", timeout=5)
    elif action == "restart-server":
        compose("restart", "-t", "1", "kyuubi", timeout=30)
    elif action == "ready":
        ready()
    else:
        raise ValueError("Unknown observer action")


def native_evidence_count(root, token):
    if not re.fullmatch(r"[a-zA-Z0-9_-]+\.(started|interrupted|completed|ended)", token):
        raise ValueError("Invalid evidence filename")
    if token.endswith(".ended"):
        reference = (root / (token.removesuffix(".ended") + ".task")).read_text().strip()
        if not re.fullmatch(r"app-[a-zA-Z0-9_-]+", reference):
            raise ValueError("Invalid Spark task reference")
        token = reference + ".ended"
    path = root / token
    return len(path.read_text().splitlines()) if path.exists() else 0


def bounded_command(args, timeout, log):
    # A timed-out command can leave child processes blocked. Kill the whole process group.
    command = command_text(args)
    started = time.monotonic()
    announce(f"Running {command} (timeout: {timeout}s; live log: {log}).")
    with log.open("w") as output:
        process = subprocess.Popen(args, cwd=ROOT, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True, bufsize=1,
                                   start_new_session=True)
        reader = None
        if isinstance(process.stdout, io.TextIOBase):
            def forward_output():
                try:
                    for line in iter(process.stdout.readline, ""):
                        with OUTPUT_LOCK:
                            output.write(line)
                            output.flush()
                            print(line, end="", flush=True)
                except (OSError, ValueError):
                    pass

            reader = threading.Thread(target=forward_output, daemon=True)
            reader.start()

        heartbeat_stop = threading.Event()

        def report_progress():
            while not heartbeat_stop.wait(COMMAND_HEARTBEAT_SECONDS):
                announce(f"Still running {command} ({time.monotonic() - started:.0f}s elapsed; "
                         f"live log: {log}).")

        heartbeat = threading.Thread(target=report_progress, daemon=True)
        heartbeat.start()
        try:
            code = process.wait(timeout=timeout)
        except BaseException as error:
            announce(f"Stopping {command} after {time.monotonic() - started:.0f}s: {error}")
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise
        finally:
            heartbeat_stop.set()
            heartbeat.join(timeout=1)
            if reader is not None:
                reader.join(timeout=5)
                if reader.is_alive():
                    process.stdout.close()
                    reader.join(timeout=1)
            output.flush()
    if reader is None and log.exists():
        contents = log.read_text()
        if contents:
            print(contents, end="", flush=True)
    announce(f"Finished {command} with exit code {code} after {time.monotonic() - started:.0f}s.")
    if code:
        raise RuntimeError(f"Command failed ({code}); see {log}")


def collect(artifacts):
    result = compose("logs", "--no-color", "--timestamps", capture=True, check=False, timeout=30)
    (artifacts / "compose.log").write_text(result.stdout + result.stderr)
    result = compose("ps", "--all", "--format", "json", capture=True, check=False, timeout=10)
    (artifacts / "containers.json").write_text(result.stdout)
    for service, source, destination in [
        ("kyuubi", "/opt/kyuubi/logs", "kyuubi-logs"),
        ("kyuubi", "/opt/kyuubi/work", "engine-work"),
        ("spark-worker", "/evidence", "executor-evidence"),
        ("spark-worker", "/opt/spark/work", "spark-executor-logs"),
    ]:
        compose("cp", f"{service}:{source}", str(artifacts / destination), check=False, timeout=30)


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("suite", choices=["backend", "macos", "observe"])
    parser.add_argument("action", nargs="?")
    parser.add_argument("token", nargs="?", default="unused")
    parser.add_argument("--runtime", choices=["native", "docker"], default="native",
                        help="Server runtime for native UI tests; backend tests always use Docker")
    args = parser.parse_args()
    if args.suite == "observe":
        if not os.environ.get("QROW_E2E_NATIVE_EVIDENCE"):
            require_commands("docker")
        observe(args.action, args.token)
        return
    if args.suite == "macos" and args.runtime == "native":
        require_commands("curl")
        announce("Starting native macOS E2E with the isolated Java fixture.")
        import servers
        servers.run()
        return
    require_commands("cargo", "docker")
    if args.suite == "macos":
        run(["sh", "scripts/e2e/driver.sh", "--preflight"])
    project = "qrow-e2e-" + uuid.uuid4().hex[:12]
    os.environ["QROW_E2E_PROJECT"] = project
    artifacts = ROOT / "target/e2e" / project
    artifacts.mkdir(parents=True)
    os.environ["QROW_E2E_ARTIFACTS"] = str(artifacts)
    announce(f"Artifacts: {artifacts}")
    failure = None
    try:
        os.environ["QROW_E2E_BIND_PORT"] = str(free_port())
        announce(f"Starting disposable Docker fixture {project}.")
        compose("up", "-d", "--build", timeout=900)
        local_port = int(compose("port", "kyuubi", "10009", capture=True).stdout.strip().rsplit(":", 1)[1])
        os.environ["QROW_E2E_PORT"] = str(local_port)
        ready()
        (artifacts / "reference.json").write_text(json.dumps({
            "project": project, "kyuubi": "1.12.0", "spark": "3.5.3",
            "authentication": "LDAP", "spark_master": "standalone", "suite": args.suite,
        }, indent=2) + "\n")
        if args.suite == "backend":
            bounded_command(["cargo", "test", "--locked", "--no-default-features", "--test",
                             "live_kyuubi", "--", "--ignored", "--test-threads=1", "--nocapture"],
                            1200, artifacts / "backend.log")
        else:
            bounded_command(["sh", "scripts/e2e/driver.sh"], 1200, artifacts / "native-ui.log")
    except BaseException as error:
        failure = error
        (artifacts / "failure.txt").write_text(str(error) + "\n")
    finally:
        try:
            announce(f"Collecting E2E evidence in {artifacts}.")
            collect(artifacts)
            announce("E2E evidence collection complete.")
        except Exception as error:
            print(f"Artifact collection failed: {error}", file=sys.stderr)
            failure = failure or error
        try:
            announce(f"Removing disposable Docker fixture {project}.")
            compose("down", "--volumes", "--remove-orphans", timeout=90)
            announce("Disposable Docker fixture removed.")
        except Exception as error:
            print(f"Fixture cleanup failed: {error}", file=sys.stderr)
            failure = failure or error
        if args.suite == "macos":
            try:
                run(["python3", "scripts/e2e/keychain.py"])
            except Exception as error:
                failure = failure or error
    if failure:
        raise failure


if __name__ == "__main__":
    main()
