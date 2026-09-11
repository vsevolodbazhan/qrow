#!/bin/sh
# The same commands run locally and in CI. No live database or Keychain access.
set -eu
cd "$(dirname "$0")/.."
mode="${1:-all}"
case "$mode" in
    hook)
        cargo fmt --all -- --check
        cargo clippy --locked --no-default-features --all-targets -- -D warnings
        cargo test --locked --no-default-features
        python3 scripts/check-policy.py
        ;;
    scripts)
        shellcheck scripts/*.sh .githooks/*
        actionlint .github/workflows/*.yml
        python3 -m unittest discover -s scripts/tests
        python3 scripts/check-policy.py
        ;;
    core)
        cargo fmt --all -- --check
        cargo clippy --locked --no-default-features --all-targets -- -D warnings
        cargo test --locked --no-default-features
        RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-default-features --no-deps
        ;;
    native)
        cargo clippy --locked --all-targets -- -D warnings
        cargo test --locked
        ;;
    dependencies)
        python3 scripts/check-policy.py
        cargo machete
        cargo deny --locked check
        ;;
    coverage)
        coverage_dir="${CARGO_TARGET_DIR:-target}/coverage"
        mkdir -p "$coverage_dir"
        cargo llvm-cov --locked --no-default-features --lib --tests \
            --ignore-filename-regex 'src/connector/t_c_l_i_service.rs|tests/|src/bin/' \
            --fail-under-lines 80 --lcov --output-path "$coverage_dir/core.lcov"
        ;;
    performance)
        cargo bench --locked --no-default-features --bench sql
        ;;
    all)
        "$0" scripts
        "$0" core
        "$0" native
        "$0" dependencies
        "$0" performance
        "$0" coverage
        ;;
    *) echo "Usage: $0 [all|hook|scripts|core|native|dependencies|coverage|performance]" >&2; exit 2 ;;
esac
