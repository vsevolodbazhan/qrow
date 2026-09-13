#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands cargo
qrow_preflight_finish || exit 1
cargo bench --locked --no-default-features --bench sql
