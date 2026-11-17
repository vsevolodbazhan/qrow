#!/bin/sh
# Build and install the AUR package in DIR in an Arch Linux container, check
# it with namcap, and write DIR/.SRCINFO. DIR holds the PKGBUILD and can hold
# source files that makepkg then does not download. Needs Docker and network
# access.
set -eu
if [ $# -ne 1 ] || [ ! -f "$1/PKGBUILD" ]; then
    echo 'Usage: aur-build.sh DIR (DIR holds the PKGBUILD)' >&2
    exit 2
fi
qrow_dir=$(cd "$1" && pwd)
# The official image is x86-64 only. The aarch64 packages differ only in their sources.
docker run --rm --platform linux/amd64 --volume "$qrow_dir:/package" archlinux:base-devel sh -euc '
# The pacman download sandbox fails in some containers, for example under emulation.
sed -i "s/^\[options\]\$/[options]\nDisableSandbox/" /etc/pacman.conf
pacman -Syu --noconfirm --needed namcap >/dev/null
useradd --create-home builder
echo "builder ALL=(ALL) NOPASSWD: ALL" > /etc/sudoers.d/builder
cp -R /package /home/builder/package
rm -f /home/builder/package/.SRCINFO
chown -R builder: /home/builder/package
cd /home/builder/package
su builder -c "makepkg --syncdeps --noconfirm --cleanbuild"
su builder -c "makepkg --printsrcinfo" > .SRCINFO
pacman -U --noconfirm ./*.pkg.tar.zst
pacman -Qkk "$(sed -n "s/^pkgname = //p" .SRCINFO)"
# namcap reports errors with " E: ". It also lists the libraries that each dependency gives.
namcap --info PKGBUILD ./*.pkg.tar.zst | tee namcap.log
if grep -F " E: " namcap.log; then
    echo "namcap found errors" >&2
    exit 1
fi
if ldd /usr/bin/qrow | grep -F "not found"; then
    echo "The package does not depend on all the libraries of Qrow" >&2
    exit 1
fi
cp .SRCINFO /package/.SRCINFO
'
printf 'Wrote %s\n' "$qrow_dir/.SRCINFO"
