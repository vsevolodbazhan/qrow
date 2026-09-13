#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands cargo cargo-machete cargo-deny python3
qrow_require_python_311
qrow_preflight_finish || exit 1
python3 scripts/core/policy.py
cargo machete
cargo deny --locked check
