#!/usr/bin/env python3
"""Real Java servers on macOS ARM, isolated from the application and user data."""
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import signal
import shutil
import socket
import subprocess
import tarfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests/e2e/fixture"


def preflight():
    if os.uname().sysname != "Darwin":
        raise RuntimeError("Native E2E tests require macOS")
    missing = [command for command in ("curl",) if shutil.which(command) is None]
    if missing:
        raise RuntimeError("Missing required command: " + ", ".join(missing))
    java_home = os.environ.get("JAVA_HOME")
    if not java_home:
        raise RuntimeError("JAVA_HOME must point to a JDK for native E2E tests")
    required_tools = ("java", "javac", "jar")
    missing = [tool for tool in required_tools if not (Path(java_home) / "bin" / tool).is_file()]
    if missing:
        raise RuntimeError("JAVA_HOME is missing required JDK tools: " + ", ".join(missing))
    return Path(java_home)


def distribution(item):
    cache = ROOT / "target/e2e-downloads"
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / item["url"].rsplit("/", 1)[1]
    if not archive.exists():
        partial = archive.with_suffix(".partial")
        subprocess.run(["curl", "--fail", "--silent", "--show-error", "--location", "--retry", "3", "--max-time", "600",
                        "--output", str(partial), item["url"]], check=True, timeout=650)
        partial.rename(archive)
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha512").hexdigest()
    if digest != item["sha512"]:
        raise ValueError(f"Checksum mismatch: {archive}")
    destination = cache / item["directory"]
    if archive.suffix != ".jar" and not destination.exists():
        with tarfile.open(archive) as source:
            source.extractall(cache, filter="data")
    return destination


def downloads():
    manifest = json.loads((ROOT / "tests/e2e/native-downloads.json").read_text())
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        paths = list(executor.map(distribution, manifest.values()))
    return dict(zip(manifest, paths))


class Servers:
    def __init__(self, artifacts):
        self.artifacts = artifacts
        self.processes = []
        self.logs = []

    def start(self, name, args, env):
        output = (self.artifacts / f"{name}.log").open("w")
        self.logs.append(output)
        process = subprocess.Popen(args, cwd=self.artifacts, env=env, stdout=output,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        self.processes.append((name, process))

    def check(self):
        for name, process in self.processes:
            if process.poll() is not None:
                raise RuntimeError(f"{name} exited: {(self.artifacts / f'{name}.log').read_text()[-4000:]}")

    def stop(self):
        # Include Spark executors and Kyuubi engines, not only their parent JVMs.
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


def run():
    def terminate(_signal, _frame):
        raise KeyboardInterrupt("Native fixture termination requested")

    previous = signal.signal(signal.SIGTERM, terminate)
    try:
        run_fixture()
    finally:
        signal.signal(signal.SIGTERM, previous)


def run_fixture():
    import run as runner
    java_home = preflight()
    runner.run(["sh", "scripts/e2e/driver.sh", "--preflight"])
    java = str(java_home / "bin/java")
    paths = downloads()
    artifacts = ROOT / "target/e2e" / ("qrow-e2e-" + uuid.uuid4().hex[:12])
    artifacts.mkdir(parents=True)
    os.environ["QROW_E2E_ARTIFACTS"] = str(artifacts)
    evidence = artifacts / "executor-evidence"
    evidence.mkdir()
    os.environ["QROW_E2E_NATIVE_EVIDENCE"] = str(evidence)
    print(f"Artifacts: {artifacts}", flush=True)
    # Build before starting the JVMs to keep compiler and server memory separate.
    runner.bounded_command(["sh", "scripts/e2e/driver.sh", "--prepare"], 1200, artifacts / "package.log")
    classes = artifacts / "classes"
    classes.mkdir()
    runner.run([str(java_home / "bin/javac"), "-cp", f"{paths['spark']}/jars/*:{paths['ldap']}",
             "-d", str(classes), str(FIXTURE / "Blocking.java"), str(FIXTURE / "Ldap.java")])
    jar = artifacts / "qrow-fixture.jar"
    runner.run([str(java_home / "bin/jar"), "cf", str(jar), "-C", str(classes), "."])
    listeners = [socket.socket() for _ in range(5)]
    try:
        for listener in listeners:
            listener.bind(("127.0.0.1", 0))
        ldap_port, zk_port, master_port, worker_port, port = [s.getsockname()[1] for s in listeners]
    finally:
        for listener in listeners:
            listener.close()
    os.environ["QROW_E2E_PORT"] = str(port)
    conf = artifacts / "conf"
    conf.mkdir()
    config = (FIXTURE / "kyuubi-defaults.conf").read_text()
    for old, new in {
        "0.0.0.0": "127.0.0.1", "10009": str(port), "ldap://ldap:389": f"ldap://127.0.0.1:{ldap_port}",
        "zookeeper:2181": f"127.0.0.1:{zk_port}", "spark://spark-master:7077": f"spark://127.0.0.1:{master_port}",
        "spark.driver.host kyuubi": "spark.driver.host 127.0.0.1", "/opt/spark/jars/qrow-fixture.jar": str(jar),
    }.items():
        config = config.replace(old, new)
    config += "\nkyuubi.frontend.protocols THRIFT_BINARY\nspark.kyuubi.frontend.thrift.binary.bind.host 127.0.0.1\nspark.kyuubi.frontend.thrift.binary.bind.port 0\n"
    config += f"\nspark.driver.extraClassPath {jar}\nspark.driver.extraJavaOptions -Dqrow.evidence.dir={evidence}\nspark.executor.extraJavaOptions -Dqrow.evidence.dir={evidence}\n"
    (conf / "kyuubi-defaults.conf").write_text(config)
    (conf / "zoo.cfg").write_text(f"tickTime=2000\ndataDir={artifacts}/zookeeper\nclientPort={zk_port}\nclientPortAddress=127.0.0.1\nadmin.enableServer=false\n")
    env = dict(os.environ, SPARK_HOME=str(paths["spark"]), KYUUBI_HOME=str(paths["kyuubi"]),
               KYUUBI_CONF_DIR=str(conf), KYUUBI_LOG_DIR=str(artifacts / "kyuubi-logs"),
               KYUUBI_PID_DIR=str(artifacts / "pid"), KYUUBI_WORK_DIR_ROOT=str(artifacts / "engine-work"),
               KYUUBI_JAVA_OPTS="-Xmx512m", SPARK_LOCAL_IP="127.0.0.1", SPARK_DAEMON_MEMORY="256m",
               SPARK_LOG_DIR=str(artifacts / "spark-logs"), SPARK_WORKER_DIR=str(artifacts / "spark-work"))
    servers = Servers(artifacts)
    try:
        servers.start("ldap", [java, "-Xmx128m", "-cp", f"{jar}:{paths['ldap']}", "io.qrow.fixture.Ldap", str(ldap_port), str(FIXTURE / "users.ldif")], env)
        servers.start("zookeeper", [java, "-Xmx256m", "-cp", f"{paths['zookeeper']}/*:{paths['zookeeper']}/lib/*:{paths['zookeeper']}/conf", "org.apache.zookeeper.server.quorum.QuorumPeerMain", str(conf / "zoo.cfg")], env)
        spark = str(paths["spark"] / "bin/spark-class")
        servers.start("spark-master", [spark, "org.apache.spark.deploy.master.Master", "--host", "127.0.0.1", "--port", str(master_port), "--webui-port", "0"], env)
        servers.start("spark-worker", [spark, "org.apache.spark.deploy.worker.Worker", "--host", "127.0.0.1", "--port", str(worker_port), "--webui-port", "0", "--cores", "2", "--memory", "2g", f"spark://127.0.0.1:{master_port}"], env)
        servers.start("kyuubi", [str(paths["kyuubi"] / "bin/kyuubi"), "run"], env)
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            servers.check()
            result = subprocess.run([str(paths["kyuubi"] / "bin/beeline"), "-u", f"jdbc:hive2://127.0.0.1:{port}/default", "-n", "qrow", "-p", "qrow-test-password", "-e", "SELECT 1"],
                                    env=env, capture_output=True, text=True, timeout=150)
            (artifacts / "readiness.log").write_text(result.stdout + result.stderr)
            if result.returncode == 0:
                break
            time.sleep(2)
        else:
            raise RuntimeError("Native fixture never became ready; see readiness.log")
        servers.check()
        (artifacts / "reference.json").write_text(json.dumps({"kyuubi": "1.12.0", "spark": "3.5.3", "authentication": "LDAP", "spark_master": "standalone", "fixture": "native-jvm", "architecture": os.uname().machine}, indent=2) + "\n")
        runner.bounded_command(["sh", "scripts/e2e/driver.sh", "--prepared"], 1200, artifacts / "native-ui.log")
        servers.check()
    except BaseException as error:
        (artifacts / "failure.txt").write_text(str(error) + "\n")
        raise
    finally:
        servers.stop()
        runner.run(["python3", "scripts/e2e/keychain.py"])


if __name__ == "__main__":
    run()
