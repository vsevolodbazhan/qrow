#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands actionlint uv shellcheck
qrow_preflight_finish || exit 1
find scripts -type f -name '*.sh' -exec shellcheck {} +
shellcheck .githooks/*
actionlint .github/workflows/*.yml
uv run --locked ruff check scripts
uv run --locked python -m unittest discover -s scripts/tests
