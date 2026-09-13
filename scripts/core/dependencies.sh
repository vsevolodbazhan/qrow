#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
python3 scripts/core/policy.py
cargo machete
cargo deny --locked check
