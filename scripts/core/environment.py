"""What the scripts need from the computer: the target directory, Docker, and a JDK.

qtest, the fixture, the Keychain cleanup, and the app probe import this file.
They add scripts/core to sys.path first, because they run as plain scripts.
"""
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
JDK_TOOLS = ("java", "javac", "jar")


def target_dir():
    """The Cargo target directory. Hook snapshots set CARGO_TARGET_DIR."""
    return Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target")


def docker_status():
    """The Docker state: "running" when its daemon answers, else "stopped" or "missing"."""
    if shutil.which("docker") is None:
        return "missing"
    try:
        answered = subprocess.run(["docker", "info"], cwd=ROOT, capture_output=True, timeout=20).returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        answered = False
    return "running" if answered else "stopped"


def choose_runtime(runtime):
    """The fixture runtime for `auto`: Docker when its daemon answers, else local Java processes."""
    if runtime != "auto":
        return runtime
    return "docker" if docker_status() == "running" else "native"


def jdk_problem():
    """None when JAVA_HOME holds a JDK, else the problem."""
    java_home = os.environ.get("JAVA_HOME")
    if not java_home:
        return "JAVA_HOME must point to a JDK for native E2E tests"
    missing = [tool for tool in JDK_TOOLS if not (Path(java_home) / "bin" / tool).is_file()]
    if missing:
        return "JAVA_HOME is missing required JDK tools: " + ", ".join(missing)
    return None
