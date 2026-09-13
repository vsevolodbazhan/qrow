#!/bin/sh
# Ordinary checks stay offline. Explicit acceptance modes use disposable servers and fixture credentials.
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
    backend|core)
        cargo fmt --all -- --check
        cargo clippy --locked --no-default-features --all-targets -- -D warnings
        cargo test --locked --no-default-features
        RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-default-features --no-deps
        ;;
    macos|native)
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
    backend-e2e|backend-integration)
        python3 scripts/e2e.py backend
        ;;
    ui-e2e|native-ui)
        shift
        python3 scripts/e2e.py native-ui "$@"
        ;;
    all)
        "$0" scripts
        "$0" backend
        "$0" macos
        "$0" dependencies
        "$0" performance
        "$0" coverage
        ;;
    *) echo "Usage: $0 [all|hook|scripts|backend|macos|dependencies|coverage|performance|backend-e2e|ui-e2e]" >&2; exit 2 ;;
esac
