#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands cargo
qrow_require_macos
qrow_require_xcode_tools
qrow_preflight_finish || exit 1
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
