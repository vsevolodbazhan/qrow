#!/usr/bin/env python3
"""Disposable LDAP, ZooKeeper, Spark, and Kyuubi servers for end-to-end tests.

Both runtimes give the same interface: start, wait for an authenticated SQL
response, warm the engine of each synthetic user, and stop. A fixture writes
its state to a JSON file, so another process can use it and stop it.

Usage: fixture.py up [--runtime auto|docker|native] | down | status | observe ACTION TOKEN
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
COMPOSE = ROOT / "tests/fixture/compose.yml"
SERVER = ROOT / "tests/fixture/server"
# Synthetic LDAP users of tests/fixture/server/users.ldif.
USERS = {"qrow": "qrow-test-password", "other": "other-test-password"}
# Tests connect as this user. Readiness starts its Spark engine, so the first
# test does not wait for an engine start. The Spark worker has room for one
# engine, so readiness does not start an engine for another user.
TEST_USER = "qrow"
READY_SECONDS = 180
PROJECT = re.compile(r"qrow-e2e-[a-z0-9-]+")
EVIDENCE = re.compile(r"[a-zA-Z0-9_-]+\.(started|interrupted|completed|ended)")


def announce(message):
    print(f"[fixture] {message}", file=sys.stderr, flush=True)


def target_dir():
    return Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))


def default_state():
    return target_dir() / "qtest" / "fixture.json"


def state_path():
    return Path(os.environ.get("QROW_FIXTURE_STATE") or default_state())


def free_ports(count):
    listeners = [socket.socket() for _ in range(count)]
    try:
        for listener in listeners:
            listener.bind(("127.0.0.1", 0))
        return [listener.getsockname()[1] for listener in listeners]
    finally:
        for listener in listeners:
            listener.close()


def docker_available():
    return shutil.which("docker") is not None and subprocess.run(
        ["docker", "info"], capture_output=True, timeout=20).returncode == 0


def wait_ready(fixture):
    """An authenticated SQL round trip as the test user; an open port is not enough."""
    started = time.monotonic()
    for user, password in [(TEST_USER, USERS[TEST_USER])]:
        announce(f"Waiting for authenticated SQL as {user} (timeout: {READY_SECONDS}s).")
        attempt = 0
        last_report = time.monotonic()
        while True:
            attempt += 1
            fixture.check_alive()
            result = fixture.beeline(user, password)
            if result.returncode == 0:
                announce(f"{user} is ready after {time.monotonic() - started:.0f}s (attempt {attempt}).")
                break
            now = time.monotonic()
            if now - started >= READY_SECONDS:
                raise RuntimeError(f"Kyuubi never became ready for {user}: {result.stderr[-4000:]}")
            if now - last_report >= 15:
                announce(f"Still waiting for {user} after {now - started:.0f}s "
                         f"(attempt {attempt}, exit code {result.returncode}).")
                last_report = now
            time.sleep(2)


def evidence_file(token):
    if not EVIDENCE.fullmatch(token):
        raise ValueError("Invalid evidence filename")
    return token


def reference(runtime):
    return {"kyuubi": "1.12.0", "spark": "3.5.3", "authentication": "LDAP",
            "spark_master": "standalone", "runtime": runtime}


class DockerFixture:
    runtime = "docker"

    def __init__(self, project, bind_port, port, artifacts):
        if not PROJECT.fullmatch(project):
            raise ValueError("A Docker fixture must use a disposable qrow-e2e-* project")
        self.project, self.bind_port, self.port, self.artifacts = project, bind_port, port, Path(artifacts)

    @classmethod
    def start(cls, artifacts):
        project = "qrow-e2e-" + uuid.uuid4().hex[:12]
        (bind_port,) = free_ports(1)
        fixture = cls(project, bind_port, 0, artifacts)
        announce(f"Starting Docker fixture {project}.")
        try:
            fixture.compose("up", "-d", "--build", timeout=900)
            published = fixture.compose("port", "kyuubi", "10009", capture=True).stdout.strip()
            fixture.port = int(published.rsplit(":", 1)[1])
            wait_ready(fixture)
        except BaseException:
            fixture.collect(Path(artifacts))
            fixture.stop()
            raise
        return fixture

    def compose(self, *args, timeout=180, check=True, capture=False):
        env = dict(os.environ, QROW_E2E_PROJECT=self.project, QROW_E2E_BIND_PORT=str(self.bind_port))
        return subprocess.run(["docker", "compose", "-f", str(COMPOSE), "-p", self.project, *args],
                              cwd=ROOT, env=env, check=check, timeout=timeout, text=True,
                              capture_output=capture)

    def beeline(self, user, password):
        return self.compose("exec", "-T", "kyuubi", "/opt/kyuubi/bin/beeline",
                            "-u", "jdbc:hive2://localhost:10009/default", "-n", user, "-p", password,
                            "-e", "SELECT 1", timeout=150, check=False, capture=True)

    def check_alive(self):
        pass

    def healthy(self):
        try:
            return self.beeline("qrow", USERS["qrow"]).returncode == 0
        except (OSError, subprocess.TimeoutExpired):
            return False

    def evidence_count(self, token):
        token = evidence_file(token)
        if token.endswith(".ended"):
            task = self.compose("exec", "-T", "spark-worker", "cat",
                                "/evidence/" + token.removesuffix(".ended") + ".task",
                                capture=True, timeout=5).stdout.strip()
            if not re.fullmatch(r"app-[a-zA-Z0-9_-]+", task):
                raise ValueError("Invalid Spark task reference")
            token = task + ".ended"
        result = self.compose("exec", "-T", "spark-worker", "sh", "-c",
                              'if [ -f "$1" ]; then wc -l < "$1"; else echo 0; fi',
                              "sh", "/evidence/" + token, capture=True, timeout=5)
        return int(result.stdout.strip())

    def kill_engine(self):
        self.compose("exec", "-T", "kyuubi", "pkill", "-9", "-f",
                     "[o]rg.apache.kyuubi.engine.spark.SparkSQLEngine", timeout=5)

    def restart_server(self):
        self.compose("restart", "-t", "1", "kyuubi", timeout=30)

    def collect(self, artifacts):
        artifacts.mkdir(parents=True, exist_ok=True)
        result = self.compose("logs", "--no-color", "--timestamps", capture=True, check=False, timeout=30)
        (artifacts / "compose.log").write_text(result.stdout + result.stderr)
        result = self.compose("ps", "--all", "--format", "json", capture=True, check=False, timeout=10)
        (artifacts / "containers.json").write_text(result.stdout)
        for service, source, destination in [
            ("kyuubi", "/opt/kyuubi/logs", "kyuubi-logs"),
            ("kyuubi", "/opt/kyuubi/work", "engine-work"),
            ("spark-worker", "/evidence", "executor-evidence"),
            ("spark-worker", "/opt/spark/work", "spark-executor-logs"),
        ]:
            self.compose("cp", f"{service}:{source}", str(artifacts / destination), check=False, timeout=30,
                         capture=True)
        (artifacts / "reference.json").write_text(json.dumps(reference(self.runtime), indent=2) + "\n")

    def stop(self):
        announce(f"Removing Docker fixture {self.project}.")
        result = self.compose("down", "--volumes", "--remove-orphans", timeout=90, check=False, capture=True)
        if result.returncode:
            raise RuntimeError(f"Could not remove {self.project}: {result.stderr[-2000:]}")

    def env(self):
        return {"QROW_E2E_PROJECT": self.project, "QROW_E2E_PORT": str(self.port)}

    def state(self):
        return {"runtime": self.runtime, "project": self.project, "bind_port": self.bind_port,
                "port": self.port, "artifacts": str(self.artifacts)}

    @classmethod
    def from_state(cls, state):
        return cls(state["project"], state["bind_port"], state["port"], state["artifacts"])


class NativeFixture:
    """Local Java processes. Used where Docker is not available, like hosted macOS runners."""
    runtime = "native"
    NAMES = ("ldap", "zookeeper", "spark-master", "spark-worker", "kyuubi")

    def __init__(self, project, port, artifacts, processes, beeline_command, server_env):
        self.project, self.port, self.artifacts = project, port, Path(artifacts)
        self.processes = processes  # [name, pid] in start order
        self.beeline_command, self.server_env = beeline_command, server_env
        self.children = {}

    @classmethod
    def start(cls, artifacts):
        import servers
        artifacts = Path(artifacts)
        artifacts.mkdir(parents=True, exist_ok=True)
        java_home = servers.preflight()
        paths = servers.downloads()
        project = "qrow-e2e-" + uuid.uuid4().hex[:12]
        evidence = artifacts / "executor-evidence"
        evidence.mkdir(exist_ok=True)
        classes = artifacts / "classes"
        classes.mkdir(exist_ok=True)
        announce("Compiling the Java fixture classes.")
        subprocess.run([str(java_home / "bin/javac"), "-cp", f"{paths['spark']}/jars/*:{paths['ldap']}",
                        "-d", str(classes), str(SERVER / "Blocking.java"), str(SERVER / "Ldap.java")],
                       check=True, timeout=180)
        jar = artifacts / "qrow-fixture.jar"
        subprocess.run([str(java_home / "bin/jar"), "cf", str(jar), "-C", str(classes), "."],
                       check=True, timeout=60)
        ldap_port, zk_port, master_port, worker_port, port = free_ports(5)
        conf = artifacts / "conf"
        conf.mkdir(exist_ok=True)
        config = (SERVER / "kyuubi-defaults.conf").read_text()
        for old, new in {
            "0.0.0.0": "127.0.0.1", "10009": str(port), "ldap://ldap:389": f"ldap://127.0.0.1:{ldap_port}",
            "zookeeper:2181": f"127.0.0.1:{zk_port}", "spark://spark-master:7077": f"spark://127.0.0.1:{master_port}",
            "spark.driver.host kyuubi": "spark.driver.host 127.0.0.1", "/opt/spark/jars/qrow-fixture.jar": str(jar),
        }.items():
            config = config.replace(old, new)
        config += ("\nkyuubi.frontend.protocols THRIFT_BINARY\n"
                   "spark.kyuubi.frontend.thrift.binary.bind.host 127.0.0.1\n"
                   "spark.kyuubi.frontend.thrift.binary.bind.port 0\n"
                   f"\nspark.driver.extraClassPath {jar}\n"
                   f"spark.driver.extraJavaOptions -Dqrow.evidence.dir={evidence}\n"
                   f"spark.executor.extraJavaOptions -Dqrow.evidence.dir={evidence}\n")
        (conf / "kyuubi-defaults.conf").write_text(config)
        (conf / "zoo.cfg").write_text(f"tickTime=2000\ndataDir={artifacts}/zookeeper\nclientPort={zk_port}\n"
                                      "clientPortAddress=127.0.0.1\nadmin.enableServer=false\n")
        env = dict(os.environ, SPARK_HOME=str(paths["spark"]), KYUUBI_HOME=str(paths["kyuubi"]),
                   KYUUBI_CONF_DIR=str(conf), KYUUBI_LOG_DIR=str(artifacts / "kyuubi-logs"),
                   KYUUBI_PID_DIR=str(artifacts / "pid"), KYUUBI_WORK_DIR_ROOT=str(artifacts / "engine-work"),
                   KYUUBI_JAVA_OPTS="-Xmx512m", SPARK_LOCAL_IP="127.0.0.1", SPARK_DAEMON_MEMORY="256m",
                   SPARK_LOG_DIR=str(artifacts / "spark-logs"), SPARK_WORKER_DIR=str(artifacts / "spark-work"),
                   QROW_E2E_NATIVE_EVIDENCE=str(evidence))
        java = str(java_home / "bin/java")
        spark = str(paths["spark"] / "bin/spark-class")
        commands = {
            "ldap": [java, "-Xmx128m", "-cp", f"{jar}:{paths['ldap']}", "io.qrow.fixture.Ldap",
                     str(ldap_port), str(SERVER / "users.ldif")],
            "zookeeper": [java, "-Xmx256m", "-cp",
                          f"{paths['zookeeper']}/*:{paths['zookeeper']}/lib/*:{paths['zookeeper']}/conf",
                          "org.apache.zookeeper.server.quorum.QuorumPeerMain", str(conf / "zoo.cfg")],
            "spark-master": [spark, "org.apache.spark.deploy.master.Master", "--host", "127.0.0.1",
                             "--port", str(master_port), "--webui-port", "0"],
            "spark-worker": [spark, "org.apache.spark.deploy.worker.Worker", "--host", "127.0.0.1",
                             "--port", str(worker_port), "--webui-port", "0", "--cores", "2", "--memory", "2g",
                             f"spark://127.0.0.1:{master_port}"],
            "kyuubi": [str(paths["kyuubi"] / "bin/kyuubi"), "run"],
        }
        beeline = [str(paths["kyuubi"] / "bin/beeline"), "-u", f"jdbc:hive2://127.0.0.1:{port}/default"]
        server_env = {key: env[key] for key in ("SPARK_HOME", "KYUUBI_HOME", "KYUUBI_CONF_DIR", "PATH")
                      if key in env} | {"JAVA_HOME": str(java_home)}
        fixture = cls(project, port, artifacts, [], beeline, server_env)
        announce(f"Starting native fixture {project}: LDAP, ZooKeeper, Spark, and Kyuubi.")
        try:
            for name in cls.NAMES:
                log = (artifacts / f"{name}.log").open("w")
                # A new session puts each server and its children in one process group.
                process = subprocess.Popen(commands[name], cwd=artifacts, env=env, stdout=log,
                                           stderr=subprocess.STDOUT, start_new_session=True)
                log.close()
                fixture.processes.append([name, process.pid])
                fixture.children[process.pid] = process
            wait_ready(fixture)
        except BaseException:
            fixture.stop()
            raise
        return fixture

    def beeline(self, user, password):
        return subprocess.run([*self.beeline_command, "-n", user, "-p", password, "-e", "SELECT 1"],
                              env=dict(os.environ, **self.server_env), capture_output=True, text=True,
                              timeout=150)

    def alive(self, pid):
        process = self.children.get(pid)
        if process is not None:
            return process.poll() is None
        try:
            os.kill(pid, 0)
            return True
        except ProcessLookupError:
            return False

    def check_alive(self):
        for name, pid in self.processes:
            if not self.alive(pid):
                log = self.artifacts / f"{name}.log"
                tail = log.read_text()[-4000:] if log.exists() else ""
                raise RuntimeError(f"{name} exited: {tail}")

    def healthy(self):
        try:
            self.check_alive()
            return self.beeline("qrow", USERS["qrow"]).returncode == 0
        except (OSError, RuntimeError, subprocess.TimeoutExpired):
            return False

    def evidence_count(self, token):
        root = self.artifacts / "executor-evidence"
        token = evidence_file(token)
        if token.endswith(".ended"):
            task = (root / (token.removesuffix(".ended") + ".task")).read_text().strip()
            if not re.fullmatch(r"app-[a-zA-Z0-9_-]+", task):
                raise ValueError("Invalid Spark task reference")
            token = task + ".ended"
        path = root / token
        return len(path.read_text().splitlines()) if path.exists() else 0

    def kill_engine(self):
        raise RuntimeError("Stopping an engine needs the Docker runtime; it cannot be scoped to local processes.")

    restart_server = kill_engine

    def collect(self, artifacts):
        artifacts.mkdir(parents=True, exist_ok=True)
        if artifacts.resolve() != self.artifacts.resolve():
            (artifacts / "native-fixture.txt").write_text(f"Server logs: {self.artifacts}\n")
        (artifacts / "reference.json").write_text(
            json.dumps(reference(self.runtime) | {"architecture": os.uname().machine}, indent=2) + "\n")

    def stop(self):
        # Include Spark executors and Kyuubi engines, not only their parent JVMs.
        announce(f"Stopping native fixture {self.project}.")
        for _, pid in reversed(self.processes):
            try:
                os.killpg(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + 10
        for _, pid in reversed(self.processes):
            while self.alive(pid) and time.monotonic() < deadline:
                time.sleep(0.1)
            try:
                os.killpg(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            if pid in self.children:
                self.children[pid].wait(timeout=5)

    def env(self):
        return {"QROW_E2E_PROJECT": self.project, "QROW_E2E_PORT": str(self.port),
                "QROW_E2E_NATIVE_EVIDENCE": str(self.artifacts / "executor-evidence")}

    def state(self):
        return {"runtime": self.runtime, "project": self.project, "port": self.port,
                "artifacts": str(self.artifacts), "processes": self.processes,
                "beeline": self.beeline_command, "server_env": self.server_env}

    @classmethod
    def from_state(cls, state):
        return cls(state["project"], state["port"], state["artifacts"], state["processes"],
                   state["beeline"], state["server_env"])


RUNTIMES = {"docker": DockerFixture, "native": NativeFixture}


def choose_runtime(runtime):
    if runtime != "auto":
        return runtime
    return "docker" if docker_available() else "native"


def start(runtime, artifacts):
    runtime = choose_runtime(runtime)
    announce(f"Using the {runtime} runtime.")
    return RUNTIMES[runtime].start(Path(artifacts))


def save(fixture, path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(fixture.state(), indent=2) + "\n")


def load(path=None):
    path = path or state_path()
    if not path.exists():
        return None
    state = json.loads(path.read_text())
    return RUNTIMES[state["runtime"]].from_state(state)


def reusable(runtime):
    """The fixture of `fixture up` when it answers SQL and matches the runtime."""
    fixture = load(default_state())
    if fixture is None or runtime not in ("auto", fixture.runtime) or not fixture.healthy():
        return None
    return fixture


def observe(action, token):
    fixture = load()
    if fixture is None:
        raise RuntimeError(f"No fixture state at {state_path()}. Run tests through ./qtest.")
    if action == "count":
        return str(fixture.evidence_count(token))
    if action == "kill-engine":
        fixture.kill_engine()
    elif action == "restart-server":
        fixture.restart_server()
    elif action == "ready":
        wait_ready(fixture)
    else:
        raise ValueError("Unknown observer action")
    return ""


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    up = commands.add_parser("up")
    up.add_argument("--runtime", choices=["auto", "docker", "native"], default="auto")
    commands.add_parser("down")
    commands.add_parser("status")
    watch = commands.add_parser("observe")
    watch.add_argument("action")
    watch.add_argument("token", nargs="?", default="unused")
    args = parser.parse_args(argv)
    if args.command == "observe":
        print(observe(args.action, args.token))
        return 0
    path = default_state()
    if args.command == "up":
        existing = load(path)
        if existing is not None and existing.healthy():
            announce(f"A {existing.runtime} fixture is already running on port {existing.port}.")
            return 0
        fixture = start(args.runtime, target_dir() / "qtest" / "fixture-artifacts" / uuid.uuid4().hex[:12])
        save(fixture, path)
        print(json.dumps(fixture.state() | {"state": str(path)}, indent=2))
        return 0
    fixture = load(path)
    if args.command == "status":
        if fixture is None:
            print(json.dumps({"running": False}))
            return 1
        print(json.dumps({"running": fixture.healthy(), **fixture.state()}, indent=2))
        return 0
    if fixture is None:
        announce("No fixture is running.")
        return 0
    fixture.stop()
    path.unlink()
    return 0


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    sys.exit(main())
