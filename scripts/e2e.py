#!/usr/bin/env python3
"""Disposable real-server acceptance tests. No saved profiles or production credentials."""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
COMPOSE = ROOT / "tests/e2e/compose.yml"


def run(args, *, timeout=180, check=True, capture=False):
    return subprocess.run(args, cwd=ROOT, check=check, timeout=timeout,
                          text=True, capture_output=capture)


def compose(*args, **kwargs):
    project = os.environ.get("QROW_E2E_PROJECT", "")
    if not re.fullmatch(r"qrow-e2e-[a-z0-9-]+", project):
        raise ValueError("QROW_E2E_PROJECT must identify a disposable qrow-e2e-* project")
    return run(["docker", "compose", "-f", str(COMPOSE), "-p", project, *args], **kwargs)


def ready():
    # Readiness is an authenticated SQL round trip; an open port is insufficient.
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        result = compose("exec", "-T", "kyuubi", "/opt/kyuubi/bin/beeline",
                         "-u", "jdbc:hive2://localhost:10009/default", "-n", "qrow",
                         "-p", "qrow-test-password", "-e", "SELECT 1",
                         timeout=150, check=False, capture=True)
        if result.returncode == 0:
            return
        time.sleep(2)
    raise RuntimeError("Kyuubi never became ready for authenticated SQL: " + result.stderr[-4000:])


def observe(action, token):
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


def bounded_command(args, timeout, log):
    # A timed-out Rust test can leave network threads blocked. Kill the whole test process group.
    with log.open("w") as output:
        process = subprocess.Popen(args, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
        except BaseException:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise
    print(log.read_text())
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
    ssh_host = os.environ.get("QROW_E2E_SSH")
    if ssh_host:
        code = "import socket; s=socket.socket(); s.bind(('127.0.0.1',0)); print(s.getsockname()[1])"
        # Pass one quoted remote command; the program contains no user-controlled input.
        import shlex
        return int(run(["ssh", ssh_host, "python3 -c " + shlex.quote(code)], capture=True).stdout)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("suite", choices=["backend", "native-ui", "observe"])
    parser.add_argument("action", nargs="?")
    parser.add_argument("token", nargs="?", default="unused")
    args = parser.parse_args()
    if args.suite == "observe":
        observe(args.action, args.token)
        return
    if args.suite == "native-ui":
        run(["sh", "scripts/native-e2e.sh", "--preflight"])
    project = "qrow-e2e-" + uuid.uuid4().hex[:12]
    os.environ["QROW_E2E_PROJECT"] = project
    artifacts = ROOT / "target/e2e" / project
    artifacts.mkdir(parents=True)
    os.environ["QROW_E2E_ARTIFACTS"] = str(artifacts)
    print(f"Artifacts: {artifacts}", flush=True)
    tunnel = None
    failure = None
    try:
        os.environ["QROW_E2E_BIND_PORT"] = str(free_port())
        compose("up", "-d", "--build", timeout=900)
        remote_port = int(compose("port", "kyuubi", "10009", capture=True).stdout.strip().rsplit(":", 1)[1])
        ssh_host = os.environ.get("QROW_E2E_SSH")
        if ssh_host:
            # Server ports remain on the Linux host's loopback interface.
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                local_port = listener.getsockname()[1]
            tunnel = subprocess.Popen(["ssh", "-N", "-o", "ExitOnForwardFailure=yes",
                                       "-o", "ServerAliveInterval=15", "-L",
                                       f"127.0.0.1:{local_port}:127.0.0.1:{remote_port}", ssh_host])
        else:
            local_port = remote_port
        os.environ["QROW_E2E_PORT"] = str(local_port)
        ready()
        if tunnel and tunnel.poll() is not None:
            raise RuntimeError("SSH port forwarding failed")
        (artifacts / "reference.json").write_text(json.dumps({
            "project": project, "kyuubi": "1.12.0", "spark": "3.5.3",
            "authentication": "LDAP", "spark_master": "standalone", "suite": args.suite,
        }, indent=2) + "\n")
        if args.suite == "backend":
            bounded_command(["cargo", "test", "--locked", "--no-default-features", "--test",
                             "live_kyuubi", "--", "--ignored", "--test-threads=1", "--nocapture"],
                            1200, artifacts / "backend.log")
        else:
            bounded_command(["sh", "scripts/native-e2e.sh"], 1200, artifacts / "native-ui.log")
    except BaseException as error:
        failure = error
        (artifacts / "failure.txt").write_text(str(error) + "\n")
    finally:
        try:
            collect(artifacts)
        except Exception as error:
            print(f"Artifact collection failed: {error}", file=sys.stderr)
            failure = failure or error
        try:
            compose("down", "--volumes", "--remove-orphans", timeout=90)
        except Exception as error:
            print(f"Fixture cleanup failed: {error}", file=sys.stderr)
            failure = failure or error
        if args.suite == "native-ui":
            try:
                run(["python3", "scripts/native-e2e-cleanup.py"])
            except Exception as error:
                failure = failure or error
        if tunnel:
            tunnel.terminate()
            tunnel.wait(timeout=10)
    if failure:
        raise failure


if __name__ == "__main__":
    main()
