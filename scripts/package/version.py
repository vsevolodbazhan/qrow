#!/usr/bin/env python3
"""Print the packaged version from `cargo metadata` output on standard input."""

from __future__ import annotations

import json
import re
import sys


PACKAGE = "qrow"
# CFBundleShortVersionString holds one to three integers separated by periods.
RELEASE_VERSION = re.compile(r"\d+(\.\d+){0,2}$")


def version(metadata: str, package: str = PACKAGE) -> str:
    packages = json.loads(metadata).get("packages", [])
    found = next((entry for entry in packages if entry.get("name") == package), None)
    if found is None:
        raise ValueError(f"Cargo metadata has no package: {package}")
    value = found.get("version", "")
    if not RELEASE_VERSION.fullmatch(value):
        raise ValueError(f"macOS accepts up to three numbers in a version, got: {value!r}")
    return value


def main() -> None:
    try:
        print(version(sys.stdin.read()))
    except ValueError as error:
        sys.exit(str(error))


if __name__ == "__main__":
    main()
