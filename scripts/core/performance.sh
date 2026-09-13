#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
cargo bench --locked --no-default-features --bench sql
