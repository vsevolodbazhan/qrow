#!/usr/bin/env python3
"""Set the release version and archive checksums in the AUR package files.

Read SHA256SUMS of a published stable release. Use `--sums` with a local copy,
or let the script download it with the GitHub CLI.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[2]
PACKAGES = ROOT / "packaging/aur"
ARCHITECTURES = ("x86_64", "aarch64")
# pacman versions cannot hold a hyphen, so nightly versions are not accepted.
STABLE_VERSION = re.compile(r"\d+\.\d+\.\d+")


def checksums(text: str, version: str) -> dict[str, str]:
    found = {}
    for line in text.splitlines():
        digest, _, name = line.partition(" ")
        name = name.strip().lstrip("*").removeprefix("./")
        for architecture in ARCHITECTURES:
            if name == f"Qrow-{version}-linux-{architecture}.tar.gz":
                if not re.fullmatch(r"[0-9a-f]{64}", digest):
                    raise ValueError(f"Invalid checksum for {name}")
                found[architecture] = digest
    missing = [architecture for architecture in ARCHITECTURES if architecture not in found]
    if missing:
        raise ValueError(f"SHA256SUMS has no Linux archive for: {', '.join(missing)}")
    return found


def replace(text: str, field: str, value: str) -> str:
    updated, count = re.subn(rf"^{field}=.*$", f"{field}={value}", text, flags=re.MULTILINE)
    if count != 1:
        raise ValueError(f"PKGBUILD must set {field} once")
    return updated


def update(directory: Path, version: str, sums: dict[str, str]) -> None:
    for build in (directory / "qrow/PKGBUILD", directory / "qrow-bin/PKGBUILD"):
        text = replace(build.read_text(), "pkgver", version)
        text = replace(text, "pkgrel", "1")
        if build.parent.name == "qrow-bin":
            for architecture, digest in sums.items():
                text = replace(text, f"sha256sums_{architecture}", f"('{digest}')")
        build.write_text(text)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", help="stable release version, for example 0.4.0")
    parser.add_argument("--sums", type=Path, help="local SHA256SUMS of the release")
    parser.add_argument("--packages", type=Path, default=PACKAGES, help=argparse.SUPPRESS)
    args = parser.parse_args()
    try:
        if not STABLE_VERSION.fullmatch(args.version):
            raise ValueError(f"AUR packages need a stable version, got: {args.version!r}")
        if args.sums:
            text = args.sums.read_text()
        else:
            text = subprocess.run(
                ["gh", "release", "download", f"v{args.version}", "--repo", "vsevolodbazhan/qrow",
                 "--pattern", "SHA256SUMS", "--output", "-"],
                check=True, capture_output=True, text=True).stdout
        update(args.packages, args.version, checksums(text, args.version))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))


if __name__ == "__main__":
    main()
