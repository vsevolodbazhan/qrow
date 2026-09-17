#!/usr/bin/env python3
"""Remove only synthetic Keychain entries created in this run's isolated workspace."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import uuid

FIXTURE_PROFILE_NAMES = {"Qrow E2E", "Qrow E2E copy", "Qrow E2E live"}


def fixture_profile_ids(profiles):
    identifiers = []
    for profile in profiles:
        if (
            profile["name"] not in FIXTURE_PROFILE_NAMES
            or profile["username"] != "qrow"
            or profile["host"] != "127.0.0.1"
        ):
            raise ValueError("Unexpected non-fixture profile")
        identifiers.append(str(uuid.UUID(profile["id"])))
    return identifiers


def main():
    if shutil.which("security") is None:
        raise RuntimeError("Missing required command: security")
    artifacts = Path(os.environ["QROW_E2E_ARTIFACTS"]).resolve()
    root = Path(__file__).resolve().parents[2] / "target/e2e"
    if artifacts.parent != root or not artifacts.name.startswith("qrow-e2e-"):
        raise ValueError("Refusing to clean credentials outside an isolated E2E run")
    workspace = artifacts / "workspace/workspace.json"
    if workspace.exists():
        profiles = json.loads(workspace.read_text())["profiles"]
        # Validate every profile before deleting any credential.
        for identifier in fixture_profile_ids(profiles):
            result = subprocess.run(
                ["security", "delete-generic-password", "-s", "io.qrow.connection", "-a", identifier],
                check=False,
                capture_output=True,
            )
            if result.returncode not in (0, 44):
                raise RuntimeError(result.stderr.decode())


if __name__ == "__main__":
    main()
