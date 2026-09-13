#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands actionlint python3 shellcheck
qrow_require_python_311
qrow_preflight_finish || exit 1
find scripts -type f -name '*.sh' -exec shellcheck {} +
shellcheck .githooks/*
actionlint .github/workflows/*.yml
python3 -m unittest discover -s scripts/tests
python3 scripts/core/policy.py
