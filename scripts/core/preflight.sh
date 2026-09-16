#!/bin/sh
# Shared prerequisite checks for repository scripts. Source after changing to the repository root.

qrow_preflight_failed=0

qrow_require_commands() {
    for qrow_command in "$@"; do
        if ! command -v "$qrow_command" >/dev/null 2>&1; then
            echo "Missing required command: $qrow_command" >&2
            qrow_preflight_failed=1
        fi
    done
}

qrow_require_macos() {
    if [ "$(uname -s)" != Darwin ]; then
        echo "This script requires macOS." >&2
        qrow_preflight_failed=1
    fi
}

qrow_require_xcode_tools() {
    qrow_require_commands xcode-select
    if command -v xcode-select >/dev/null 2>&1 && ! xcode-select -p >/dev/null 2>&1; then
        echo "Xcode command-line tools are not configured; run xcode-select --install." >&2
        qrow_preflight_failed=1
    fi
}

qrow_preflight_finish() {
    if [ "$qrow_preflight_failed" -ne 0 ]; then
        echo "Install or configure the missing prerequisites, then run this script again." >&2
        return 1
    fi
}
