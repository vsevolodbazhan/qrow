#!/usr/bin/env python3
"""Use a job-local SSH configuration; never alter the runner user's SSH files."""
import os
from pathlib import Path
import re
import shutil
import sys

root = Path(os.environ["RUNNER_TEMP"]) / "qrow-e2e-ssh"
if "--cleanup" in sys.argv:
    shutil.rmtree(root, ignore_errors=True)
    sys.exit(0)
host, user = os.environ["E2E_SSH_HOST"], os.environ["E2E_SSH_USER"]
if not re.fullmatch(r"[a-zA-Z0-9.-]+", host) or not re.fullmatch(r"[a-zA-Z0-9_-]+", user):
    raise ValueError("Configure QROW_E2E_SSH_HOST and QROW_E2E_SSH_USER for a dedicated Linux Docker host")
key, known = os.environ["E2E_SSH_KEY"], os.environ["E2E_KNOWN_HOSTS"]
if not key.strip() or not known.strip():
    raise ValueError("Missing QROW_E2E_SSH_KEY or pinned QROW_E2E_KNOWN_HOSTS")
root.mkdir(mode=0o700)
for name, content in [("key", key), ("known_hosts", known)]:
    path = root / name
    path.write_text(content + "\n")
    path.chmod(0o600)
config = root / "config"
config.write_text(f'''Host qrow-e2e-docker
  HostName {host}
  User {user}
  IdentityFile "{root / 'key'}"
  UserKnownHostsFile "{root / 'known_hosts'}"
  StrictHostKeyChecking yes
  IdentitiesOnly yes
  BatchMode yes
  ConnectTimeout 15
''')
# Docker invokes ssh through PATH. A private wrapper shares the config with port forwarding.
wrapper = root / "ssh"
wrapper.write_text('#!/bin/sh\nexec /usr/bin/ssh -F ' + "'" + str(config).replace("'", "'\\''") + "'" + ' "$@"\n')
wrapper.chmod(0o700)
with open(os.environ["GITHUB_PATH"], "a") as output:
    output.write(str(root) + "\n")
