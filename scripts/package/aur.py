#!/usr/bin/env python3
"""Write the AUR PKGBUILD of a release.

Copy packaging/aur/PKGBUILD to the output folder with the package of the
release channel, the release version, and the archive checksums from the
SHA256SUMS of the release. Stable releases go to qrow-bin, nightly releases to
qrow-nightly-bin.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[2]
TEMPLATE = ROOT / "packaging/aur/PKGBUILD"
ARCHITECTURES = ("x86_64", "aarch64")
STABLE_VERSION = re.compile(r"\d+\.\d+\.\d+")
NIGHTLY_VERSION = re.compile(r"\d+\.\d+\.\d+-nightly\.\d{8}\.\d+")
# The package name and description of each channel.
PACKAGES = {
    "stable": ("qrow-bin", "Desktop SQL workbench (release build)"),
    "nightly": ("qrow-nightly-bin", "Desktop SQL workbench (nightly build)"),
}


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


def channel(version: str) -> str:
    if STABLE_VERSION.fullmatch(version):
        return "stable"
    if NIGHTLY_VERSION.fullmatch(version):
        return "nightly"
    raise ValueError(f"Not a stable or nightly release version: {version!r}")


def pkgver(version: str) -> str:
    # 0.6.0-nightly.20261116.63 becomes 0.6.0.nightly.20261116.63. pacman
    # compares the parts in order, so later nightly builds have higher versions.
    return version.replace("-", ".")


def replace(text: str, field: str, value: str) -> str:
    updated, count = re.subn(rf"^{field}=.*$", lambda _: f"{field}={value}", text, flags=re.MULTILINE)
    if count != 1:
        raise ValueError(f"PKGBUILD must set {field} once")
    return updated


def render(template: str, version: str, sums: dict[str, str]) -> str:
    name, description = PACKAGES[channel(version)]
    text = replace(template, "pkgname", name)
    text = replace(text, "pkgdesc", f"'{description}'")
    text = replace(text, "_version", version)
    text = replace(text, "pkgver", pkgver(version))
    text = replace(text, "pkgrel", "1")
    for architecture in ARCHITECTURES:
        text = replace(text, f"sha256sums_{architecture}", f"('{sums[architecture]}')")
    return text


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", help="release version, for example 0.6.0 or 0.6.0-nightly.20261116.63")
    parser.add_argument("--sums", type=Path, required=True, help="SHA256SUMS of the release")
    parser.add_argument("--output", type=Path, required=True, help="folder for the PKGBUILD")
    args = parser.parse_args()
    try:
        channel(args.version)
        text = render(TEMPLATE.read_text(), args.version,
                      checksums(args.sums.read_text(), args.version))
        args.output.mkdir(parents=True, exist_ok=True)
        (args.output / "PKGBUILD").write_text(text)
    except (OSError, ValueError) as error:
        sys.exit(str(error))


if __name__ == "__main__":
    main()
