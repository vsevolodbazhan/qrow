#!/usr/bin/env python3
"""Check Linux archives before CI publishes them."""
import argparse
from pathlib import Path
import subprocess
import tarfile
import tempfile

REQUIRED = (
    "bin/qrow", "share/applications/qrow.desktop",
    "share/icons/hicolor/256x256/apps/qrow.png", "share/licenses/qrow/LICENSE",
    "share/licenses/qrow/NOTICE", "share/licenses/qrow/THIRD_PARTY_NOTICES.txt",
)


def check(archive):
    """Extract one archive and validate its installed files and dependencies."""
    with tempfile.TemporaryDirectory(prefix="qrow-linux-package-") as temporary:
        root = Path(temporary)
        with tarfile.open(archive) as bundle:
            bundle.extractall(root, filter="data")
        for name in REQUIRED:
            path = root / name
            if not path.is_file() or path.stat().st_size == 0:
                raise ValueError(f"Missing or empty package file: {name}")
        binary = root / "bin/qrow"
        if not binary.stat().st_mode & 0o111:
            raise ValueError("Package executable has no execute permission")
        result = subprocess.run(["ldd", str(binary)], check=True, capture_output=True, text=True)
        if "not found" in result.stdout:
            raise ValueError(f"Unresolved shared library:\n{result.stdout}")
        subprocess.run(["desktop-file-validate", str(root / "share/applications/qrow.desktop")], check=True)
        print(f"Validated {archive}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    options = parser.parse_args()
    archives = sorted(options.directory.glob("Qrow-*-linux-*.tar.gz"))
    if not archives:
        parser.error(f"No Linux archives in {options.directory}")
    for archive in archives:
        check(archive)


if __name__ == "__main__":
    main()
