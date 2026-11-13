#!/bin/sh
# Use a private bus and keyring; never access the user's desktop secrets.
set -eu
if [ "${1:-}" = --session ]; then
    export QROW_ISOLATED_SECRET_TEST=1
    printf '%s\n' synthetic-keyring-password | gnome-keyring-daemon --unlock --components=secrets >/dev/null
    # The daemon claims its bus name after it returns. A request before that
    # makes D-Bus start a second daemon, which has no unlocked keyring.
    qrow_tries=0
    until dbus-send --session --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.NameHasOwner string:org.freedesktop.secrets 2>/dev/null | grep -q 'boolean true'; do
        qrow_tries=$((qrow_tries + 1))
        if [ "$qrow_tries" -ge 100 ]; then
            echo "GNOME Keyring did not register on the private D-Bus session" >&2
            exit 1
        fi
        sleep 0.1
    done
    qrow_profile=default
    if [ -n "${CI:-}" ]; then qrow_profile=ci; fi
    exec "${QROW_PYTHON:-python3}" scripts/qtest/builds.py cargo nextest run --locked --no-default-features --profile "$qrow_profile" --lib --run-ignored only -E 'test(isolated_secret_service_round_trip)'
fi
qrow_secret_dir=$(mktemp -d)
trap 'rm -rf "$qrow_secret_dir"' EXIT HUP INT TERM
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
# Start the bus in the isolated environment, so that the services that it
# starts also use the temporary home and runtime folders.
export HOME="$qrow_secret_dir"
export XDG_DATA_HOME="$HOME/data" XDG_CONFIG_HOME="$HOME/config"
export XDG_RUNTIME_DIR="$HOME/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR"
dbus-run-session -- sh "$0" --session
