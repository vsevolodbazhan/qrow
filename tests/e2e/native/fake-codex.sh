#!/bin/sh
# Start the deterministic app server without an account or user data.
exec python3 "$(dirname "$0")/fake-codex.py" "$@"
