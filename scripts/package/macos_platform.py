#!/usr/bin/env python3
"""Read the macOS package baseline and locate its Cargo executable."""
import os
from pathlib import Path
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
TARGETS = {"aarch64-apple-darwin": "arm64", "x86_64-apple-darwin": "x86_64"}


def minimum(config, override=None):
    baseline = config["env"]["MACOSX_DEPLOYMENT_TARGET"]
    selected = override or baseline
    try:
        versions = [tuple(int(part) for part in value.split(".")) for value in (baseline, selected)]
    except (AttributeError, ValueError) as error:
        raise ValueError("Use a numeric MACOSX_DEPLOYMENT_TARGET") from error
    if any(not 1 <= len(version) <= 3 or any(part < 0 for part in version) for version in versions):
        raise ValueError("Use up to three numeric macOS version components")
    padded = [version + (0,) * (3 - len(version)) for version in versions]
    if padded[1] < padded[0]:
        raise ValueError(f"MACOSX_DEPLOYMENT_TARGET must be at least {baseline}")
    return selected


def executable(target_dir, profile, target=None):
    if profile not in ("release", "debug"):
        raise ValueError("Use release or debug for QROW_BUILD_PROFILE")
    if target is not None and target not in TARGETS:
        raise ValueError("Use aarch64-apple-darwin or x86_64-apple-darwin for CARGO_BUILD_TARGET")
    directory = Path(target_dir)
    if target:
        directory /= target
    return directory / profile / "qrow"


def host():
    version = subprocess.check_output(["rustc", "-vV"], text=True)
    return next(line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: "))


def main():
    config = tomllib.loads((ROOT / ".cargo/config.toml").read_text())
    command = sys.argv[1]
    if command == "minimum":
        print(minimum(config, os.environ.get("MACOSX_DEPLOYMENT_TARGET")))
    elif command == "executable":
        print(executable(os.environ.get("CARGO_TARGET_DIR", "target"),
                         os.environ.get("QROW_BUILD_PROFILE", "release"), os.environ.get("CARGO_BUILD_TARGET")))
    elif command == "architecture":
        target = os.environ.get("CARGO_BUILD_TARGET") or host()
        if target not in TARGETS:
            raise ValueError(f"Unsupported macOS package target: {target}")
        print(TARGETS[target])
    else:
        raise ValueError(f"Unknown package platform command: {command}")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, ValueError, StopIteration) as error:
        sys.exit(str(error))
