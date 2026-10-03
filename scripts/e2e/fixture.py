#!/usr/bin/env python3
"""Disposable LDAP, OIDC, ZooKeeper, Spark, and Kyuubi servers for end-to-end tests.

Both runtimes give the same interface: start, wait for an authenticated SQL
response, warm the engine of the synthetic user, and stop. A fixture writes
its state to a JSON file, so another process can use it and stop it.

Kyuubi accepts the LDAP password of the test user or an access token of the
mock OIDC provider (`tests/fixture/server/TokenOrLdap.java`). Its plain binary
port has a TLS proxy in front (`TlsProxy.java`). A synthetic CA signs the
certificate of the proxy and the provider; tests trust only that CA.

Usage: fixture.py up [--runtime auto|docker|native] | down | status | observe ACTION
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/core"))
from environment import choose_runtime, target_dir  # noqa: E402

COMPOSE = ROOT / "tests/fixture/compose.yml"
SERVER = ROOT / "tests/fixture/server"
# The synthetic LDAP user of tests/fixture/server/users.ldif. Tests connect
# as this user. Readiness starts its Spark engine, so the first test does not
# wait for an engine start. The Spark worker has room for one engine.
TEST_USER = "qrow"
TEST_PASSWORD = "qrow-test-password"
READY_SECONDS = 180
# The synthetic keystore password of `tests/fixture/server/Certificates.java`.
KEYSTORE_PASSWORD = "qrow-fixture-tls"
PROJECT = re.compile(r"qrow-e2e-[a-z0-9-]+")


def announce(message):
    print(f"[fixture] {message}", file=sys.stderr, flush=True)


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


def security_dir(artifacts):
    """Certificates, the CA PEM that tests trust, and the provider JWKS."""
    return Path(artifacts) / "security"


def ca_path(artifacts):
    return (security_dir(artifacts) / "ca.pem").resolve()


def issuer(port):
    """The OIDC issuer. Docker publishes the provider on the same port, so it is the same everywhere."""
    return f"https://127.0.0.1:{port}"


def secure_env(fixture):
    return {"QROW_E2E_TLS_PORT": str(fixture.tls_port), "QROW_E2E_OIDC_ISSUER": issuer(fixture.oidc_port),
            "QROW_E2E_TLS_CA": str(ca_path(fixture.artifacts))}


def tls_context(ca):
    context = ssl.create_default_context(cafile=str(ca))
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    return context


def discovery(issuer_url, ca, timeout=5):
    """The discovery document of the provider, read over HTTPS with only the fixture CA."""
    import urllib.request
    url = issuer_url + "/.well-known/openid-configuration"
    with urllib.request.urlopen(url, timeout=timeout, context=tls_context(ca)) as response:
        document = json.load(response)
    if document.get("issuer") != issuer_url:
        raise ValueError(f"The provider reports issuer {document.get('issuer')!r}, not {issuer_url!r}")
    return document


def tls_handshake(port, ca, timeout=5):
    """A complete TLS handshake with the proxy; returns the protocol version."""
    with socket.create_connection(("127.0.0.1", port), timeout=timeout) as raw:
        with tls_context(ca).wrap_socket(raw, server_hostname="127.0.0.1") as connection:
            return connection.version()


def secure_problem(fixture):
    """Why the OIDC provider or the TLS proxy is not ready yet, or None."""
    ca = ca_path(fixture.artifacts)
    if not ca.exists():
        return f"No CA certificate at {ca} yet"
    try:
        discovery(issuer(fixture.oidc_port), ca)
        tls_handshake(fixture.tls_port, ca)
    except (OSError, ValueError) as error:
        return f"{type(error).__name__}: {error}"
    return None


def wait_ready(fixture):
    """An authenticated SQL round trip as the test user; an open port is not enough.

    The OIDC provider and the TLS proxy must also answer over TLS with the fixture CA.
    """
    started = time.monotonic()
    announce(f"Waiting for the OIDC provider and the TLS proxy (timeout: {READY_SECONDS}s).")
    while (problem := secure_problem(fixture)) is not None:
        fixture.check_alive()
        if time.monotonic() - started >= READY_SECONDS:
            raise RuntimeError(f"The OIDC provider or the TLS proxy never became ready: {problem}")
        time.sleep(1)
    user = TEST_USER
    announce(f"Waiting for authenticated SQL as {user} (timeout: {READY_SECONDS}s).")
    attempt = 0
    last_report = time.monotonic()
    while True:
        attempt += 1
        fixture.check_alive()
        result = fixture.beeline(user, TEST_PASSWORD)
        if result.returncode == 0:
            announce(f"{user} is ready after {time.monotonic() - started:.0f}s (attempt {attempt}).")
            return
        now = time.monotonic()
        if now - started >= READY_SECONDS:
            raise RuntimeError(f"Kyuubi never became ready for {user}: {result.stderr[-4000:]}")
        if now - last_report >= 15:
            announce(f"Still waiting for {user} after {now - started:.0f}s "
                     f"(attempt {attempt}, exit code {result.returncode}).")
            last_report = now
        time.sleep(2)


def evidence_dir(artifacts):
    """The executor evidence of `tests/fixture/server/Blocking.java`. Tests read it from the host."""
    return Path(artifacts) / "executor-evidence"


def reference(runtime):
    return {"kyuubi": "1.12.0", "spark": "3.5.3", "authentication": "LDAP or OIDC access token (fixture)",
            "tls": "fixture proxy", "spark_master": "standalone", "runtime": runtime}


# A state from before the TLS and OIDC servers has no ports for them. Compose
# rejects port 0 even for `down`, so such a state uses this port in the
# configuration. Cleanup does not publish it.
PLACEHOLDER_PORT = 1


class DockerFixture:
    runtime = "docker"

    def __init__(self, project, bind_port, port, artifacts, tls_port=0, oidc_port=0):
        if not PROJECT.fullmatch(project):
            raise ValueError("A Docker fixture must use a disposable qrow-e2e-* project")
        self.project, self.bind_port, self.port, self.artifacts = project, bind_port, port, Path(artifacts)
        # Docker publishes both on the same loopback port of the host.
        self.tls_port, self.oidc_port = tls_port, oidc_port

    @classmethod
    def start(cls, artifacts):
        project = "qrow-e2e-" + uuid.uuid4().hex[:12]
        bind_port, tls_port, oidc_port = free_ports(3)
        fixture = cls(project, bind_port, 0, artifacts, tls_port, oidc_port)
        announce(f"Starting Docker fixture {project}.")
        for shared in (evidence_dir(artifacts), security_dir(artifacts)):
            shared.mkdir(parents=True, exist_ok=True)
            # The servers can run as another user than the host user.
            shared.chmod(0o777)
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
        env = dict(os.environ, QROW_E2E_PROJECT=self.project, QROW_E2E_BIND_PORT=str(self.bind_port),
                   QROW_E2E_EVIDENCE=str(evidence_dir(self.artifacts).resolve()),
                   QROW_E2E_SECURITY=str(security_dir(self.artifacts).resolve()),
                   QROW_E2E_TLS_BIND_PORT=str(self.tls_port or PLACEHOLDER_PORT),
                   QROW_E2E_OIDC_PORT=str(self.oidc_port or PLACEHOLDER_PORT))
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
            return secure_problem(self) is None and self.beeline(TEST_USER, TEST_PASSWORD).returncode == 0
        except (OSError, subprocess.TimeoutExpired):
            return False

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
        for service, name in (("oidc", "oidc"), ("kyuubi-tls", "tls-proxy"), ("certificates", "certificates")):
            result = self.compose("logs", "--no-color", "--timestamps", service, capture=True, check=False,
                                  timeout=30)
            (artifacts / f"{name}.log").write_text(result.stdout + result.stderr)
        for service, source, destination in [
            ("kyuubi", "/opt/kyuubi/logs", "kyuubi-logs"),
            ("kyuubi", "/opt/kyuubi/work", "engine-work"),
            ("spark-worker", "/opt/spark/work", "spark-executor-logs"),
        ]:
            self.compose("cp", f"{service}:{source}", str(artifacts / destination), check=False, timeout=30,
                         capture=True)
        if artifacts.resolve() != self.artifacts.resolve() and evidence_dir(self.artifacts).exists():
            shutil.copytree(evidence_dir(self.artifacts), evidence_dir(artifacts), dirs_exist_ok=True)
        (artifacts / "reference.json").write_text(json.dumps(reference(self.runtime), indent=2) + "\n")

    def stop(self):
        announce(f"Removing Docker fixture {self.project}.")
        result = self.compose("down", "--volumes", "--remove-orphans", timeout=90, check=False, capture=True)
        if result.returncode:
            raise RuntimeError(f"Could not remove {self.project}: {result.stderr[-2000:]}")

    def env(self):
        return {"QROW_E2E_PROJECT": self.project, "QROW_E2E_PORT": str(self.port),
                "QROW_E2E_NATIVE_EVIDENCE": str(evidence_dir(self.artifacts).resolve()), **secure_env(self)}

    def state(self):
        return {"runtime": self.runtime, "project": self.project, "bind_port": self.bind_port,
                "port": self.port, "tls_port": self.tls_port, "oidc_port": self.oidc_port,
                "artifacts": str(self.artifacts)}

    @classmethod
    def from_state(cls, state):
        return cls(state["project"], state["bind_port"], state["port"], state["artifacts"],
                   state.get("tls_port", 0), state.get("oidc_port", 0))


def kyuubi_jar_dir(artifacts, kyuubi_home, jar):
    """The Kyuubi server jars and the fixture jar, for `KYUUBI_JAR_DIR` of `bin/kyuubi`.

    The verified download stays unchanged: the directory has links to its jars.
    """
    jars = Path(artifacts) / "kyuubi-jars"
    jars.mkdir(exist_ok=True)
    sources = sorted((Path(kyuubi_home) / "jars").glob("*.jar")) + [Path(jar)]
    for source in sources:
        link = jars / source.name
        if not link.is_symlink():
            link.symlink_to(source.resolve())
    return jars


class NativeFixture:
    """Local Java processes. Used where Docker is not available, like hosted macOS runners."""
    runtime = "native"
    NAMES = ("ldap", "oidc", "tls-proxy", "zookeeper", "spark-master", "spark-worker", "kyuubi")
    # Fixture classes for the Kyuubi server, the JDK-only servers, and Spark.
    SOURCES = ("Blocking.java", "Certificates.java", "Ldap.java", "Oidc.java", "TlsProxy.java", "TokenOrLdap.java")

    def __init__(self, project, port, artifacts, processes, beeline_command, server_env, tls_port=0, oidc_port=0):
        self.project, self.port, self.artifacts = project, port, Path(artifacts)
        self.processes = processes  # [name, pid] in start order
        self.beeline_command, self.server_env = beeline_command, server_env
        self.tls_port, self.oidc_port = tls_port, oidc_port
        self.children = {}

    @classmethod
    def start(cls, artifacts):
        import servers
        artifacts = Path(artifacts)
        artifacts.mkdir(parents=True, exist_ok=True)
        java_home = servers.preflight()
        paths = servers.downloads()
        project = "qrow-e2e-" + uuid.uuid4().hex[:12]
        evidence = evidence_dir(artifacts)
        evidence.mkdir(exist_ok=True)
        classes = artifacts / "classes"
        classes.mkdir(exist_ok=True)
        announce("Compiling the Java fixture classes.")
        subprocess.run([str(java_home / "bin/javac"), "-cp",
                        f"{paths['spark']}/jars/*:{paths['kyuubi']}/jars/*:{paths['ldap']}",
                        "-d", str(classes), *(str(SERVER / source) for source in cls.SOURCES)],
                       check=True, timeout=180)
        jar = artifacts / "qrow-fixture.jar"
        subprocess.run([str(java_home / "bin/jar"), "cf", str(jar), "-C", str(classes), "."],
                       check=True, timeout=60)
        java = str(java_home / "bin/java")
        security = security_dir(artifacts)
        announce("Creating the fixture CA and server certificate.")
        subprocess.run([java, "-cp", str(jar), "io.qrow.fixture.Certificates", str(security)],
                       check=True, timeout=120, stdout=subprocess.DEVNULL)
        jars = kyuubi_jar_dir(artifacts, paths["kyuubi"], jar)
        ldap_port, zk_port, master_port, worker_port, port, tls_port, oidc_port = free_ports(7)
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
                   KYUUBI_JAR_DIR=str(jars),
                   KYUUBI_JAVA_OPTS=f"-Xmx512m -Dqrow.oidc.jwks={security / 'jwks.json'} "
                                    f"-Dqrow.oidc.issuer={issuer(oidc_port)}",
                   SPARK_LOCAL_IP="127.0.0.1", SPARK_DAEMON_MEMORY="256m",
                   SPARK_LOG_DIR=str(artifacts / "spark-logs"), SPARK_WORKER_DIR=str(artifacts / "spark-work"),
                   QROW_E2E_NATIVE_EVIDENCE=str(evidence))
        spark = str(paths["spark"] / "bin/spark-class")
        keystore = str(security / "server.p12")
        commands = {
            "ldap": [java, "-Xmx128m", "-cp", f"{jar}:{paths['ldap']}", "io.qrow.fixture.Ldap",
                     str(ldap_port), str(SERVER / "users.ldif")],
            "oidc": [java, "-Xmx96m", "-cp", str(jar), "io.qrow.fixture.Oidc", str(oidc_port), keystore,
                     KEYSTORE_PASSWORD, str(security / "jwks.json"), "127.0.0.1"],
            "tls-proxy": [java, "-Xmx64m", "-cp", str(jar), "io.qrow.fixture.TlsProxy", str(tls_port),
                          "127.0.0.1", str(port), keystore, KEYSTORE_PASSWORD, "127.0.0.1"],
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
        fixture = cls(project, port, artifacts, [], beeline, server_env, tls_port, oidc_port)
        announce(f"Starting native fixture {project}: LDAP, OIDC, TLS proxy, ZooKeeper, Spark, and Kyuubi.")
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
            return secure_problem(self) is None and self.beeline(TEST_USER, TEST_PASSWORD).returncode == 0
        except (OSError, RuntimeError, subprocess.TimeoutExpired):
            return False

    def kill_engine(self):
        raise RuntimeError("Stopping an engine needs the Docker runtime; it cannot be scoped to local processes.")

    restart_server = kill_engine

    def collect(self, artifacts):
        artifacts.mkdir(parents=True, exist_ok=True)
        if artifacts.resolve() != self.artifacts.resolve():
            (artifacts / "native-fixture.txt").write_text(f"Server logs: {self.artifacts}\n")
            for name in ("oidc", "tls-proxy"):
                log = self.artifacts / f"{name}.log"
                if log.exists():
                    shutil.copyfile(log, artifacts / f"{name}.log")
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
                "QROW_E2E_NATIVE_EVIDENCE": str(evidence_dir(self.artifacts)), **secure_env(self)}

    def state(self):
        return {"runtime": self.runtime, "project": self.project, "port": self.port,
                "tls_port": self.tls_port, "oidc_port": self.oidc_port,
                "artifacts": str(self.artifacts), "processes": self.processes,
                "beeline": self.beeline_command, "server_env": self.server_env}

    @classmethod
    def from_state(cls, state):
        return cls(state["project"], state["port"], state["artifacts"], state["processes"],
                   state["beeline"], state["server_env"], state.get("tls_port", 0), state.get("oidc_port", 0))


RUNTIMES = {"docker": DockerFixture, "native": NativeFixture}


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


def observe(action):
    fixture = load()
    if fixture is None:
        raise RuntimeError(f"No fixture state at {state_path()}. Run tests through ./qtest.")
    if action == "kill-engine":
        fixture.kill_engine()
    elif action == "restart-server":
        fixture.restart_server()
    elif action == "ready":
        wait_ready(fixture)
    else:
        raise ValueError("Unknown observer action")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    up = commands.add_parser("up")
    up.add_argument("--runtime", choices=["auto", "docker", "native"], default="auto")
    commands.add_parser("down")
    commands.add_parser("status")
    watch = commands.add_parser("observe")
    watch.add_argument("action", choices=["kill-engine", "restart-server", "ready"])
    args = parser.parse_args(argv)
    if args.command == "observe":
        observe(args.action)
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
