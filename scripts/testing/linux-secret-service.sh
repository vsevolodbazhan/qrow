#!/bin/sh
# Use a private bus and keyring; never access the user's desktop secrets.
set -eu
if [ "${1:-}" = --session ]; then
    export HOME="$QROW_SECRET_TEST_DIR"
    export XDG_DATA_HOME="$HOME/data" XDG_CONFIG_HOME="$HOME/config"
    export XDG_RUNTIME_DIR="$HOME/runtime"
    mkdir -m 700 "$XDG_RUNTIME_DIR"
    export QROW_ISOLATED_SECRET_TEST=1
    printf '%s\n' synthetic-keyring-password | gnome-keyring-daemon --unlock --components=secrets >/dev/null
    exec cargo nextest run --locked --no-default-features --lib --run-ignored only -E 'test(isolated_secret_service_round_trip)'
fi
qrow_secret_dir=$(mktemp -d)
trap 'rm -rf "$qrow_secret_dir"' EXIT HUP INT TERM
export QROW_SECRET_TEST_DIR="$qrow_secret_dir"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
dbus-run-session -- sh "$0" --session
