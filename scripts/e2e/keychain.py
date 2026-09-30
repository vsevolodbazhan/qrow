#!/usr/bin/env python3
"""Remove only synthetic Keychain entries created in this run's isolated workspace."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import uuid

# The desktop driver adds one connection with this name.
FIXTURE_PROFILE_NAME = "Qrow E2E"


def fixture_profile_ids(profiles):
    identifiers = []
    for profile in profiles:
        if (
            profile["name"] != FIXTURE_PROFILE_NAME
            or profile["username"] != "qrow"
            or profile["host"] != "127.0.0.1"
        ):
            raise ValueError("Unexpected non-fixture profile")
        identifiers.append(str(uuid.UUID(profile["id"])))
    return identifiers


def isolated_run(artifacts, target):
    """The desktop directory of one qtest run: <target>/qtest/runs/<run>/desktop."""
    run = artifacts.parent
    return (artifacts.name == "desktop" and run.parent == target / "qtest" / "runs"
            and re.fullmatch(r"\d{8}-\d{6}-\d+", run.name) is not None)


def main():
    if shutil.which("security") is None:
        raise RuntimeError("Missing required command: security")
    artifacts = Path(os.environ["QROW_E2E_ARTIFACTS"]).resolve()
    target = Path(os.environ.get("CARGO_TARGET_DIR", Path(__file__).resolve().parents[2] / "target")).resolve()
    if not isolated_run(artifacts, target):
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
