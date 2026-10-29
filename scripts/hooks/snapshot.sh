#!/bin/sh
# Check committed/staged content without touching the user's worktree or index.
set -eu
# shellcheck disable=SC1091 # The hook can run from any working directory.
. "$(dirname "$0")/../core/preflight.sh"
qrow_require_commands git mktemp tar
qrow_preflight_finish || exit 1
repository="$(git rev-parse --show-toplevel)"
revision="${1:?Expected a Git tree or commit}"
shift
scratch="$(mktemp -d "${TMPDIR:-/tmp}/qrow-check.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
git -C "$repository" archive --format=tar --output="$scratch/snapshot.tar" "$revision"
mkdir "$scratch/worktree"
tar -xf "$scratch/snapshot.tar" -C "$scratch/worktree"
# Share dependency builds between snapshots and between the worktrees of one
# repository: the build directory is in the main checkout. Never replace the
# running application.
common="$(git -C "$repository" rev-parse --path-format=absolute --git-common-dir)"
case "$common" in
    */.git) checkout="${common%/.git}" ;;
    *) checkout="$repository" ;;
esac
export CARGO_TARGET_DIR="${QROW_HOOK_TARGET_DIR:-$checkout/target/hook-checks}"
cd "$scratch/worktree"
# The remaining arguments are qtest arguments, for example `hook pre-push`.
sh ./qtest "$@"
