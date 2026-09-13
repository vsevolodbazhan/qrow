#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
find scripts -type f -name '*.sh' -exec shellcheck {} +
shellcheck .githooks/*
actionlint .github/workflows/*.yml
python3 -m unittest discover -s scripts/tests
python3 scripts/core/policy.py
