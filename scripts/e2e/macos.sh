#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands uv
qrow_preflight_finish || exit 1
exec uv run --locked python scripts/e2e/run.py macos "$@"
