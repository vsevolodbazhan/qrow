#!/usr/bin/env python3
"""Remove only synthetic Keychain entries created in this run's isolated workspace."""
import json
import os
from pathlib import Path
import subprocess
import uuid

artifacts = Path(os.environ["QROW_E2E_ARTIFACTS"]).resolve()
root = Path(__file__).resolve().parent.parent / "target/e2e"
if artifacts.parent != root or not artifacts.name.startswith("qrow-e2e-"):
    raise ValueError("Refusing to clean credentials outside an isolated E2E run")
workspace = artifacts / "workspace/workspace.json"
if workspace.exists():
    for profile in json.loads(workspace.read_text())["profiles"]:
        if profile["name"] != "Qrow E2E" or profile["username"] != "qrow" or profile["host"] != "127.0.0.1":
            raise ValueError("Unexpected non-fixture profile")
        identifier = str(uuid.UUID(profile["id"]))
        result = subprocess.run(["security", "delete-generic-password", "-s", "io.qrow.connection",
                                 "-a", identifier], check=False, capture_output=True)
        if result.returncode not in (0, 44):
            raise RuntimeError(result.stderr.decode())
