#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands cargo cargo-machete cargo-deny uv
qrow_preflight_finish || exit 1
uv run --locked python scripts/core/policy.py
cargo machete
cargo deny --locked check
