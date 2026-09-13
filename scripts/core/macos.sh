#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
