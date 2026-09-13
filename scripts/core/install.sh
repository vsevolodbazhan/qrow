#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands cargo rustup
qrow_preflight_finish || exit 1
# Pin tool versions as well as the project's Rust toolchain.
if ! cargo deny --version 2>/dev/null | grep -qx 'cargo-deny 0.20.2'; then
    cargo install --locked cargo-deny --version 0.20.2
fi
if ! cargo machete --version 2>/dev/null | grep -qx '0.9.2'; then
    cargo install --locked cargo-machete --version 0.9.2
fi
if ! cargo llvm-cov --version 2>/dev/null | grep -qx 'cargo-llvm-cov 0.9.1'; then
    cargo install --locked cargo-llvm-cov --version 0.9.1
fi
rustup component add llvm-tools-preview
