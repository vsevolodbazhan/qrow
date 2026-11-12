#!/bin/sh
# Install the latest stable Qrow release on Linux:
#
#   curl -fsSL https://qrow.dev/install.sh | sh
#
# The script downloads the release archive for this processor, verifies its
# checksum (and the signature of the checksums when minisign is installed),
# and installs it into ~/.local. Qrow then updates itself.
#
# QROW_PREFIX sets another install folder. QROW_DOWNLOAD_URL sets another
# release folder, for example for tests.
set -eu

QROW_PUBLIC_KEY=RWR2pcgdcxt7jUi0QyCxd/bGK6kkC1Ggku3NNiniI/1PqQDEBbBkbzlG
QROW_RELEASES=https://github.com/vsevolodbazhan/qrow/releases/latest/download

fail() {
    printf 'qrow: %s\n' "$1" >&2
    exit 1
}

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl --proto '=https,file' --tlsv1.2 -fsSL -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        fail 'curl or wget is required'
    fi
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        fail 'sha256sum or shasum is required'
    fi
}

# The whole script is one function, so a partial download runs nothing.
main() {
    [ "$(uname -s)" = Linux ] || fail 'this script installs Qrow on Linux. See https://qrow.dev/#install for other systems.'
    case "$(uname -m)" in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
        *) fail "there is no Qrow build for $(uname -m)" ;;
    esac
    command -v tar >/dev/null 2>&1 || fail 'tar is required'

    releases=${QROW_DOWNLOAD_URL:-$QROW_RELEASES}
    prefix=${QROW_PREFIX:-$HOME/.local}
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT HUP INT TERM

    fetch "$releases/SHA256SUMS" "$work/SHA256SUMS" || fail 'could not download the release checksums'
    if command -v minisign >/dev/null 2>&1; then
        fetch "$releases/SHA256SUMS.minisig" "$work/SHA256SUMS.minisig" || fail 'could not download the release signature'
        minisign -V -q -m "$work/SHA256SUMS" -P "$QROW_PUBLIC_KEY" || fail 'the release signature is not valid'
    fi

    # A line is "CHECKSUM  ./Qrow-VERSION-linux-ARCH.tar.gz".
    line=$(grep -E "^[0-9a-f]{64}  (\./)?Qrow-[A-Za-z0-9.+-]+-linux-$arch\.tar\.gz\$" "$work/SHA256SUMS" | head -n 1)
    [ -n "$line" ] || fail "the latest release has no archive for $arch"
    expected=${line%% *}
    name=${line##* }
    name=${name#./}
    version=${name#Qrow-}
    version=${version%-linux-"$arch".tar.gz}

    printf 'Downloading Qrow %s for %s\n' "$version" "$arch"
    fetch "$releases/$name" "$work/$name" || fail "could not download $name"
    [ "$(sha256 "$work/$name")" = "$expected" ] || fail "the checksum of $name does not match"

    # Stage the files in the prefix, then rename each into place, the
    # executable last. A rename keeps a running Qrow working until it quits.
    mkdir -p "$prefix" || fail "could not create $prefix"
    stage=$(mktemp -d "$prefix/.qrow-install-XXXXXX") || fail "could not write to $prefix"
    trap 'rm -rf "$work" "$stage"' EXIT HUP INT TERM
    tar -xzf "$work/$name" -C "$stage" || fail "could not extract $name"
    [ -f "$stage/bin/qrow" ] || fail "$name has no bin/qrow"
    (cd "$stage" && find share -type f) | while IFS= read -r file; do
        mkdir -p "$prefix/$(dirname "$file")"
        mv -f "$stage/$file" "$prefix/$file"
    done
    mkdir -p "$prefix/bin"
    mv -f "$stage/bin/qrow" "$prefix/bin/qrow"

    # Show the launcher and icon in the application menu at once.
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database -q "$prefix/share/applications" 2>/dev/null || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true
    fi

    printf 'Installed Qrow %s in %s\n' "$version" "$prefix"
    case ":$PATH:" in
        *":$prefix/bin:"*) printf 'Run qrow, or open Qrow from the application menu.\n' ;;
        *) printf 'Add %s/bin to your PATH to run qrow, or open Qrow from the application menu.\n' "$prefix" ;;
    esac
}

main "$@"
