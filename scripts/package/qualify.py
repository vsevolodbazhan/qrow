"""Inspect and launch an extracted candidate, then record its content digest."""
import argparse
import hashlib
import os
import platform
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "qtest"))
from contract import atomic_json, source_sha  # noqa: E402
from targets import host_target  # noqa: E402
from process import spawn_options, stop_tree  # noqa: E402


def linux_archive(archive, destination, machine):
    required = {"bin/qrow", "share/applications/qrow.desktop", "share/icons/hicolor/256x256/apps/qrow.png",
                "share/licenses/qrow/LICENSE", "share/licenses/qrow/NOTICE", "share/licenses/qrow/THIRD_PARTY_NOTICES.txt"}
    with tarfile.open(archive) as bundle:
        members = bundle.getmembers()
        for member in members:
            path = Path(member.name)
            if path.is_absolute() or ".." in path.parts or not (member.isfile() or member.isdir()):
                raise ValueError(f"Unsafe archive member: {member.name}")
        if not required <= {member.name.removeprefix("./") for member in members}:
            raise ValueError("Missing package files")
        bundle.extractall(destination, filter="data")
    executable = destination / "bin/qrow"
    header = executable.read_bytes()[:20]
    expected = {"x86_64": 62, "aarch64": 183}[machine]
    if len(header) != 20 or header[:6] != b"\x7fELF\x02\x01" or int.from_bytes(header[18:20], "little") != expected:
        raise ValueError("Package ELF architecture mismatch")
    if not os.access(executable, os.X_OK):
        raise ValueError("Package executable has no execute permission")
    libraries = subprocess.run(["ldd", executable], check=True, capture_output=True, text=True)
    if "not found" in libraries.stdout:
        raise ValueError("Missing runtime libraries: " + libraries.stdout)
    return executable


def smoke(executable, system, evidence):
    """Wait for a native window, without a blocking read on application output."""
    with tempfile.TemporaryDirectory(prefix="qrow-package-smoke-") as workspace:
        env = dict(os.environ, QROW_DATA_DIR=workspace)
        command = [str(executable), "--demo"]
        observer = None
        if system == "macos":
            observer = Path(workspace) / "window-ready"
            source = Path(__file__).resolve().parents[2] / "tests/desktop/window_ready.swift"
            subprocess.run(["swiftc", source, "-o", observer], check=True, timeout=60)
        if system == "linux":
            env.pop("WAYLAND_DISPLAY", None)
            env["XDG_SESSION_TYPE"] = "x11"
        with evidence.open("wb") as log:
            process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT, **spawn_options())
            try:
                deadline = time.monotonic() + 60
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError(f"Packaged application exited before readiness: {process.returncode}")
                    if system == "linux":
                        window = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(process.pid)],
                                                capture_output=True, timeout=5)
                        ready = window.returncode == 0 and bool(window.stdout.strip())
                    else:
                        window = subprocess.run([observer, str(process.pid)], capture_output=True, timeout=5)
                        ready = window.returncode == 0
                    if ready:
                        return
                    time.sleep(0.1)
                raise TimeoutError("Packaged application did not open a native window")
            finally:
                stop_tree(process)


def qualify_dmg(image, app, evidence):
    def tree(directory):
        return {str(path.relative_to(directory)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in directory.rglob("*") if path.is_file()}

    with tempfile.TemporaryDirectory(prefix="qrow-dmg-check-") as directory:
        mount = Path(directory) / "image"
        mount.mkdir()
        subprocess.run(["hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, image],
                       check=True, capture_output=True, timeout=60)
        try:
            mounted_app = mount / "Qrow.app"
            if tree(mounted_app) != tree(app) or not (mounted_app / "Contents/MacOS/qrow").exists():
                raise ValueError("Final DMG does not contain the qualified app")
            subprocess.run(["codesign", "--verify", "--deep", "--strict", mounted_app], check=True)
            smoke(mounted_app / "Contents/MacOS/qrow", "macos", evidence)
        finally:
            detach = subprocess.run(["hdiutil", "detach", mount], capture_output=True, timeout=30)
            if detach.returncode:
                subprocess.run(["hdiutil", "detach", "-force", mount], check=True, timeout=30)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--inside-display", action="store_true")
    parser.add_argument("--dmg", type=Path)
    args = parser.parse_args()
    target = host_target()
    if target is None or not target.supported:
        raise ValueError("No supported native packaging target")
    if args.dmg:
        if target.system != "macos":
            raise ValueError("DMG qualification requires macOS")
        args.receipt.parent.mkdir(parents=True, exist_ok=True)
        qualify_dmg(args.dmg, args.directory / "Qrow.app", args.receipt.with_suffix(".log"))
        atomic_json(args.receipt, {"schema": 1, "target": target.name,
                                  "source_sha": source_sha(Path.cwd()), "archive": args.dmg.name,
                                  "sha256": hashlib.sha256(args.dmg.read_bytes()).hexdigest(),
                                  "qualification": "native-dmg-smoke"})
        return 0
    if target.system == "linux" and not args.inside_display:
        command = ["xvfb-run", "-a", sys.executable, __file__, str(args.directory),
                   "--receipt", str(args.receipt), "--inside-display"]
        return subprocess.call(command)
    extension = "*.tar.gz" if target.system == "linux" else "*.zip"
    archives = sorted(args.directory.glob(extension))
    if len(archives) != 1:
        raise ValueError(f"Expected one candidate archive; found {len(archives)}")
    archive = archives[0]
    args.receipt.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="qrow-qualify-") as directory:
        destination = Path(directory)
        if target.system == "linux":
            executable = linux_archive(archive, destination, target.machine)
        else:
            subprocess.run(["ditto", "-x", "-k", archive, destination], check=True)
            app = destination / "Qrow.app"
            subprocess.run(["codesign", "--verify", "--deep", "--strict", app], check=True)
            executable = app / "Contents/MacOS/qrow"
            architecture = subprocess.check_output(["lipo", "-archs", executable], text=True).strip()
            if architecture != {"aarch64": "arm64", "x86_64": "x86_64"}[target.machine]:
                raise ValueError("Mach-O architecture mismatch")
        smoke(executable, target.system, args.receipt.with_suffix(".log"))
    atomic_json(args.receipt, {"schema": 1, "target": target.name, "source_sha": source_sha(Path.cwd()),
                              "archive": archive.name, "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
                              "host": platform.platform(), "runner_image": os.environ.get("ImageVersion"),
                              "release_version": os.environ.get("QROW_RELEASE_VERSION"),
                              "qualification": "native-package-smoke"})
    return 0


if __name__ == "__main__":
    sys.exit(main())
