#!/bin/sh
# Push the PKGBUILD and .SRCINFO in DIR to the AUR package that .SRCINFO names.
# AUR_SSH_PRIVATE_KEY holds the SSH key of the AUR account. RELEASE_VERSION
# goes into the commit message. AUR_REMOTE replaces the AUR URL in tests.
set -eu
if [ $# -ne 1 ] || [ ! -f "$1/PKGBUILD" ] || [ ! -f "$1/.SRCINFO" ]; then
    echo 'Usage: aur-push.sh DIR (DIR holds the PKGBUILD and .SRCINFO)' >&2
    exit 2
fi
[ -n "${RELEASE_VERSION:-}" ] || { echo "Set RELEASE_VERSION" >&2; exit 2; }
if [ -z "${AUR_SSH_PRIVATE_KEY:-}" ]; then
    echo "The AUR_SSH_PRIVATE_KEY secret is missing." >&2
    exit 1
fi
qrow_dir=$(cd "$1" && pwd)
qrow_package=$(sed -n 's/^pkgbase = //p' "$qrow_dir/.SRCINFO")
case "$qrow_package" in
    qrow|qrow-bin|qrow-nightly-bin) ;;
    *)
        echo "Unexpected AUR package: $qrow_package" >&2
        exit 1
        ;;
esac
qrow_work=$(mktemp -d)
trap 'rm -rf "$qrow_work"' EXIT HUP INT TERM
(umask 077 && printf '%s\n' "$AUR_SSH_PRIVATE_KEY" > "$qrow_work/key")
# The host key that aur.archlinux.org publishes: SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4.
echo 'aur.archlinux.org ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEuBKrPzbawxA/k2g6NcyV5jmqwJ2s+zpgZGZ7tpLIcN' \
    > "$qrow_work/known_hosts"
GIT_SSH_COMMAND="ssh -i $qrow_work/key -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$qrow_work/known_hosts"
export GIT_SSH_COMMAND

qrow_repository="$qrow_work/repository"
git clone "${AUR_REMOTE:-ssh://aur@aur.archlinux.org/$qrow_package.git}" "$qrow_repository"
# The AUR keeps the history of a package name, so add a commit on top of it.
find "$qrow_repository" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
cp "$qrow_dir/PKGBUILD" "$qrow_dir/.SRCINFO" "$qrow_repository/"
cd "$qrow_repository"
git add --all
if git diff --cached --quiet; then
    echo "$qrow_package already has Qrow $RELEASE_VERSION."
    exit 0
fi
git -c user.name="github-actions[bot]" \
    -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
    commit --message "Update to $RELEASE_VERSION"
git push origin HEAD:master
