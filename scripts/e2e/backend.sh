#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands python3
qrow_require_python_311
qrow_preflight_finish || exit 1
exec python3 scripts/e2e/run.py backend "$@"
