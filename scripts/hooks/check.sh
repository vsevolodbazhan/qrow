#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."

changed() {
    printf '%s\n' "$QROW_CHANGED_FILES" | grep -Eq "$1"
}

backend=false
dependencies=false
scripts=false
if [ -z "${QROW_CHANGED_FILES+x}" ]; then
    backend=true
    dependencies=true
else
    if changed '^(Cargo\.toml|Cargo\.lock|rust-toolchain\.toml|src/|tests/)'; then backend=true; fi
    if changed '^(Cargo\.toml|Cargo\.lock|deny\.toml|dependency-reviews\.toml|scripts/core/(dependencies|install)\.sh|scripts/core/policy\.py)$'; then dependencies=true; fi
    if changed '^scripts/core/preflight\.sh$'; then
        backend=true
        dependencies=true
    fi
    if changed '^(\.githooks/|\.github/workflows/|pyproject\.toml$|uv\.lock$|scripts/)'; then scripts=true; fi
fi

if [ "$backend" = true ]; then
    . scripts/core/preflight.sh
    qrow_require_commands cargo
    qrow_preflight_finish || exit 1
    cargo fmt --all -- --check
    cargo clippy --locked --no-default-features --all-targets -- -D warnings
    cargo test --locked --no-default-features
fi
if [ "$dependencies" = true ]; then
    . scripts/core/preflight.sh
    qrow_require_commands uv
    qrow_preflight_finish || exit 1
    uv run --locked python scripts/core/policy.py
fi
if [ "$scripts" = true ]; then
    sh scripts/core/scripts.sh
fi
if [ "$backend" = false ] && [ "$dependencies" = false ] && [ "$scripts" = false ]; then
    echo 'No staged files require pre-commit checks.'
fi
