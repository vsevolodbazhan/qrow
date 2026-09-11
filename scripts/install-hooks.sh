#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
existing="$(git config --get core.hooksPath || true)"
if [ -n "$existing" ] && [ "$existing" != '.githooks' ]; then
    echo "Existing hooksPath is $existing; refusing to replace another hook setup." >&2
    exit 1
fi
git config --local core.hooksPath .githooks
printf '%s\n' 'Enabled .githooks for this repository.'
