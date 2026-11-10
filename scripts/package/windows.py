"""Build the unsigned native Windows release archive."""
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile
import zipfile


def main():
    if (sys.platform != "win32" or platform.machine().lower() not in ("amd64", "x86_64", "arm64", "aarch64")
            or os.environ.get("CARGO_BUILD_TARGET")):
        raise ValueError("A native Windows build is required; unset CARGO_BUILD_TARGET")
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], text=True))
    package = next(item for item in metadata["packages"] if item["name"] == "qrow")
    version = os.environ.get("QROW_RELEASE_VERSION") or package["version"]
    if not re.fullmatch(r"[a-zA-Z0-9.+-]+", version):
        raise ValueError("Invalid release version")
    subprocess.run(["cargo", "build", "--locked", "--release", "--bin", "qrow"], check=True)
    output = Path("target/package")
    output.mkdir(parents=True, exist_ok=True)
    architecture = "aarch64" if platform.machine().lower() in ("arm64", "aarch64") else "x86_64"
    archive = output / f"Qrow-{version}-windows-{architecture}.zip"
    with tempfile.TemporaryDirectory(prefix="qrow-notices-") as directory:
        notices = Path(directory) / "THIRD_PARTY_NOTICES.txt"
        subprocess.run([sys.executable, "scripts/package/notices.py", str(notices)], check=True)
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            bundle.write(Path(metadata["target_directory"]) / "release/qrow.exe", "qrow.exe")
            for name in ("LICENSE", "NOTICE"):
                bundle.write(name, name)
            bundle.write(notices, notices.name)
    print(archive)


if __name__ == "__main__":
    main()
