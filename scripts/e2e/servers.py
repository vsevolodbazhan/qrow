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
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests/e2e/fixture"
DOWNLOAD_REPORT_INTERVAL = 15
DOWNLOAD_TIMEOUT_SECONDS = 120 * 60
DOWNLOAD_WATCHDOG_SECONDS = DOWNLOAD_TIMEOUT_SECONDS + 60
OUTPUT_LOCK = threading.Lock()


def announce(message):
    with OUTPUT_LOCK:
        print(f"[native-e2e] {message}", flush=True)


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
    java_home = os.environ.get("JAVA_HOME")
    if not java_home:
        raise RuntimeError("JAVA_HOME must point to a JDK for native E2E tests")
    required_tools = ("java", "javac", "jar")
    missing = [tool for tool in required_tools if not (Path(java_home) / "bin" / tool).is_file()]
    if missing:
        raise RuntimeError("JAVA_HOME is missing required JDK tools: " + ", ".join(missing))
    return Path(java_home)


def distribution(item, label=None):
    label = label or item["directory"]
    total = item.get("bytes")
    cache = ROOT / "target/e2e-downloads"
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / item["url"].rsplit("/", 1)[1]
    if not archive.exists():
        partial = archive.with_suffix(".partial")
        if total and partial.exists() and partial.stat().st_size == total:
            announce(f"{label}: completed partial download found ({human_size(total)}).")
            partial.rename(archive)
        else:
            if total and partial.exists() and partial.stat().st_size > total:
                announce(f"{label}: discarding oversized partial download ({human_size(partial.stat().st_size)}).")
                partial.unlink()
            initial_received = partial.stat().st_size if partial.exists() else 0
            if initial_received:
                announce(f"{label}: resuming download at {human_size(initial_received)} / {human_size(total)}.")
            command = ["curl", "--fail", "--silent", "--show-error", "--location", "--continue-at", "-",
                       "--max-time", str(DOWNLOAD_TIMEOUT_SECONDS),
                       "--output", str(partial), item["url"]]
            total_text = f" ({human_size(total)} total)" if total else ""
            announce(f"{label}: downloading {item['url']}{total_text}.")
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
            partial.rename(archive)
            announce(f"{label}: download complete: {human_size(archive.stat().st_size)} "
                     f"in {elapsed_seconds(started)}.")
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
    manifest = json.loads((ROOT / "tests/e2e/native-downloads.json").read_text())
    cache = ROOT / "target/e2e-downloads"
    marker = cache / ".complete"
    if marker.exists():
        marker.unlink()
    announce(f"Checking {len(manifest)} native fixture dependencies in {cache}.")
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        paths = list(executor.map(distribution, manifest.values(), manifest.keys()))
    marker.write_text("All native fixture dependencies were verified.\n")
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
    announce(f"Native fixture preflight passed (JAVA_HOME={java_home}).")
    announce("Checking native driver automation permissions.")
    runner.run(["sh", "scripts/e2e/driver.sh", "--preflight"])
    announce("Native driver preflight passed.")
    java = str(java_home / "bin/java")
    paths = downloads()
    artifacts = ROOT / "target/e2e" / ("qrow-e2e-" + uuid.uuid4().hex[:12])
    artifacts.mkdir(parents=True)
    os.environ["QROW_E2E_ARTIFACTS"] = str(artifacts)
    evidence = artifacts / "executor-evidence"
    evidence.mkdir()
    os.environ["QROW_E2E_NATIVE_EVIDENCE"] = str(evidence)
    announce(f"Artifacts: {artifacts}")
    # Build before starting the JVMs to keep compiler and server memory separate.
    announce("Preparing the Qrow application package for the native driver.")
    runner.bounded_command(["sh", "scripts/e2e/driver.sh", "--prepare"], 1200, artifacts / "package.log")
    announce("Qrow application package prepared.")
    classes = artifacts / "classes"
    classes.mkdir()
    announce("Compiling Java fixture classes.")
    runner.run([str(java_home / "bin/javac"), "-cp", f"{paths['spark']}/jars/*:{paths['ldap']}",
             "-d", str(classes), str(FIXTURE / "Blocking.java"), str(FIXTURE / "Ldap.java")])
    jar = artifacts / "qrow-fixture.jar"
    announce("Packaging the Java fixture JAR.")
    runner.run([str(java_home / "bin/jar"), "cf", str(jar), "-C", str(classes), "."])
    announce(f"Java fixture JAR ready: {jar}.")
    listeners = [socket.socket() for _ in range(5)]
    try:
        for listener in listeners:
            listener.bind(("127.0.0.1", 0))
        ldap_port, zk_port, master_port, worker_port, port = [s.getsockname()[1] for s in listeners]
    finally:
        for listener in listeners:
            listener.close()
    announce(f"Allocated fixture ports: LDAP={ldap_port}, ZooKeeper={zk_port}, "
             f"Spark master={master_port}, Spark worker={worker_port}, Kyuubi={port}.")
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
        announce("Starting LDAP, ZooKeeper, Spark master, Spark worker, and Kyuubi.")
        servers.start("ldap", [java, "-Xmx128m", "-cp", f"{jar}:{paths['ldap']}", "io.qrow.fixture.Ldap", str(ldap_port), str(FIXTURE / "users.ldif")], env)
        servers.start("zookeeper", [java, "-Xmx256m", "-cp", f"{paths['zookeeper']}/*:{paths['zookeeper']}/lib/*:{paths['zookeeper']}/conf", "org.apache.zookeeper.server.quorum.QuorumPeerMain", str(conf / "zoo.cfg")], env)
        spark = str(paths["spark"] / "bin/spark-class")
        servers.start("spark-master", [spark, "org.apache.spark.deploy.master.Master", "--host", "127.0.0.1", "--port", str(master_port), "--webui-port", "0"], env)
        servers.start("spark-worker", [spark, "org.apache.spark.deploy.worker.Worker", "--host", "127.0.0.1", "--port", str(worker_port), "--webui-port", "0", "--cores", "2", "--memory", "2g", f"spark://127.0.0.1:{master_port}"], env)
        servers.start("kyuubi", [str(paths["kyuubi"] / "bin/kyuubi"), "run"], env)
        deadline = time.monotonic() + 180
        started = deadline - 180
        attempt = 0
        last_report = started
        fixture_ready = False
        announce("Waiting for authenticated Kyuubi SQL readiness (timeout: 180s).")
        while True:
            now = time.monotonic()
            if now >= deadline:
                break
            attempt += 1
            servers.check()
            result = subprocess.run([str(paths["kyuubi"] / "bin/beeline"), "-u", f"jdbc:hive2://127.0.0.1:{port}/default", "-n", "qrow", "-p", "qrow-test-password", "-e", "SELECT 1"],
                                    env=env, capture_output=True, text=True, timeout=150)
            (artifacts / "readiness.log").write_text(result.stdout + result.stderr)
            if result.returncode == 0:
                announce(f"Kyuubi is ready after {time.monotonic() - started:.0f}s (attempt {attempt}).")
                fixture_ready = True
                break
            if now - last_report >= 15:
                announce(f"Still waiting for Kyuubi readiness after {now - started:.0f}s "
                         f"(attempt {attempt}, last exit code {result.returncode}).")
                last_report = now
            time.sleep(2)
        if not fixture_ready:
            raise RuntimeError("Native fixture never became ready; see readiness.log")
        servers.check()
        announce("Native fixture is ready; starting the native UI driver.")
        (artifacts / "reference.json").write_text(json.dumps({"kyuubi": "1.12.0", "spark": "3.5.3", "authentication": "LDAP", "spark_master": "standalone", "fixture": "native-jvm", "architecture": os.uname().machine}, indent=2) + "\n")
        runner.bounded_command(["sh", "scripts/e2e/driver.sh", "--prepared"], 1200, artifacts / "native-ui.log")
        announce("Native UI driver completed successfully.")
        servers.check()
    except BaseException as error:
        announce(f"Native fixture failed: {error}")
        (artifacts / "failure.txt").write_text(str(error) + "\n")
        raise
    finally:
        announce("Cleaning up the native fixture.")
        servers.stop()
        runner.run(["python3", "scripts/e2e/keychain.py"])
        announce("Native fixture cleanup complete.")


if __name__ == "__main__":
    run()
