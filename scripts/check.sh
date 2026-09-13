#!/bin/sh
# Offline checks by default; E2E modes start disposable real servers explicitly.
set -eu
cd "$(dirname "$0")/.."
mode="${1:-all}"
if [ "$#" -gt 0 ]; then shift; fi
case "$mode" in
    core/backend|core/macos|core/dependencies|core/scripts|core/coverage|core/performance|e2e/backend|e2e/macos)
        exec sh "scripts/$mode.sh" "$@"
        ;;
    hook)
        exec sh scripts/hooks/check.sh
        ;;
    all)
        for check in core/scripts core/dependencies core/backend core/macos core/performance core/coverage; do
            sh "scripts/$check.sh"
        done
        ;;
    *)
        echo 'Usage: scripts/check.sh [all|hook|core/{backend,macos,dependencies,scripts,coverage,performance}|e2e/{backend,macos}]' >&2
        exit 2
        ;;
esac
