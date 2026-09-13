#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands cargo
qrow_preflight_finish || exit 1
cargo fmt --all -- --check
cargo clippy --locked --no-default-features --all-targets -- -D warnings
cargo test --locked --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-default-features --no-deps
