#!/bin/sh
# Check committed/staged content without touching the user's worktree or index.
set -eu
repository="$(git rev-parse --show-toplevel)"
revision="${1:?Expected a Git tree or commit}"
mode="${2:-hook}"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/qrow-check.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
git -C "$repository" archive --format=tar --output="$scratch/snapshot.tar" "$revision"
mkdir "$scratch/worktree"
tar -xf "$scratch/snapshot.tar" -C "$scratch/worktree"
# Share dependency builds between snapshots; never replace the running application.
export CARGO_TARGET_DIR="$repository/target/hook-checks"
cd "$scratch/worktree"
sh scripts/check.sh "$mode"
