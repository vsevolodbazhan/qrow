#!/usr/bin/env python3
"""Write the AUR PKGBUILD of a release.

`--package bin` copies packaging/aur/bin/PKGBUILD with the package of the
release channel, the release version, and the archive checksums from the
SHA256SUMS of the release: qrow-bin for a stable release, qrow-nightly-bin for
a nightly release. `--package source` copies packaging/aur/source/PKGBUILD,
the qrow package, with the version, the release commit, and the checksum of the
source archive. Only stable releases have a source package.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[2]
TEMPLATES = {
    "bin": ROOT / "packaging/aur/bin/PKGBUILD",
    "source": ROOT / "packaging/aur/source/PKGBUILD",
}
ARCHITECTURES = ("x86_64", "aarch64")
STABLE_VERSION = re.compile(r"\d+\.\d+\.\d+")
NIGHTLY_VERSION = re.compile(r"\d+\.\d+\.\d+-nightly\.\d{8}\.\d+")
SHA256 = re.compile(r"[0-9a-f]{64}")
COMMIT = re.compile(r"[0-9a-f]{40}")
# The name and description of the binary package of each channel.
BINARY_PACKAGES = {
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
                if not SHA256.fullmatch(digest):
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


def render_binary(template: str, version: str, sums: dict[str, str]) -> str:
    name, description = BINARY_PACKAGES[channel(version)]
    text = replace(template, "pkgname", name)
    text = replace(text, "pkgdesc", f"'{description}'")
    text = replace(text, "_version", version)
    text = replace(text, "pkgver", pkgver(version))
    text = replace(text, "pkgrel", "1")
    for architecture in ARCHITECTURES:
        text = replace(text, f"sha256sums_{architecture}", f"('{sums[architecture]}')")
    return text


def render_source(template: str, version: str, commit: str, digest: str) -> str:
    if channel(version) != "stable":
        raise ValueError(f"Only stable releases have a source package, got: {version!r}")
    if not COMMIT.fullmatch(commit):
        raise ValueError(f"The release commit must be a full SHA, got: {commit!r}")
    if not SHA256.fullmatch(digest):
        raise ValueError(f"Invalid checksum of the source archive: {digest!r}")
    text = replace(template, "pkgver", version)
    text = replace(text, "pkgrel", "1")
    text = replace(text, "_commit", commit)
    return replace(text, "sha256sums", f"('{digest}')")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("version", help="release version, for example 0.6.0 or 0.6.0-nightly.20261116.63")
    parser.add_argument("--package", choices=TEMPLATES, default="bin", help="package kind (default: bin)")
    parser.add_argument("--sums", type=Path, help="SHA256SUMS of the release (bin)")
    parser.add_argument("--commit", help="full SHA of the release commit (source)")
    parser.add_argument("--source-sha256", help="checksum of the GitHub source archive of the tag (source)")
    parser.add_argument("--output", type=Path, required=True, help="folder for the PKGBUILD")
    args = parser.parse_args()
    try:
        template = TEMPLATES[args.package].read_text()
        if args.package == "bin":
            if args.sums is None:
                raise ValueError("--package bin needs --sums")
            text = render_binary(template, args.version, checksums(args.sums.read_text(), args.version))
        else:
            if args.commit is None or args.source_sha256 is None:
                raise ValueError("--package source needs --commit and --source-sha256")
            text = render_source(template, args.version, args.commit, args.source_sha256)
        args.output.mkdir(parents=True, exist_ok=True)
        (args.output / "PKGBUILD").write_text(text)
    except (OSError, ValueError) as error:
        sys.exit(str(error))


if __name__ == "__main__":
    main()
