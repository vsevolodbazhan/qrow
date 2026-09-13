#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
coverage_dir="${CARGO_TARGET_DIR:-target}/coverage"
mkdir -p "$coverage_dir"
cargo llvm-cov --locked --no-default-features --lib --tests \
    --ignore-filename-regex 'src/connector/t_c_l_i_service.rs|tests/|src/bin/' \
    --fail-under-lines 80 --lcov --output-path "$coverage_dir/core.lcov"
