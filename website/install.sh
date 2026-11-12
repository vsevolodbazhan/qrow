#!/bin/sh
# Install the latest stable Qrow release on Linux:
#
#   curl -fsSL https://qrow.dev/install.sh | sh
#
# The script downloads the release archive for this processor, verifies the
# signature of the release checksums with minisign or OpenSSL 3, verifies the
# checksum of the archive, and installs it into ~/.local. Qrow then updates
# itself. If neither minisign nor OpenSSL 3 is installed, the script asks
# before it continues with the checksum only.
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
        curl --proto '=https,file' --tlsv1.2 --retry 3 -fsSL -o "$2" "$1"
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

# True when OpenSSL can verify a minisign signature: Ed25519 over the
# BLAKE2b-512 hash of the file. OpenSSL 1.1 and LibreSSL cannot.
openssl_ready() {
    command -v openssl >/dev/null 2>&1 || return 1
    command -v base64 >/dev/null 2>&1 || return 1
    openssl pkeyutl -help 2>&1 | grep -q -- '-rawin' || return 1
    printf '' | openssl dgst -blake2b512 >/dev/null 2>&1
}

# Verifies the minisign signature $2 of the file $1 with OpenSSL, as
# `minisign -V` does: the signature of the file, then the global signature of
# the signature and the trusted comment. $3 is a folder for temporary files.
openssl_verify() {
    printf '%s' "$QROW_PUBLIC_KEY" | base64 -d > "$3/key" 2>/dev/null || return 1
    sed -n 2p "$2" | base64 -d > "$3/signature" 2>/dev/null || return 1
    sed -n 4p "$2" | base64 -d > "$3/global" 2>/dev/null || return 1
    # The key is "Ed", the key ID (8 bytes), and the key (32 bytes). The
    # signature is "ED" (prehashed), the key ID, and the signature (64 bytes).
    [ "$(wc -c < "$3/key")" -eq 42 ] && [ "$(wc -c < "$3/signature")" -eq 74 ] && [ "$(wc -c < "$3/global")" -eq 64 ] || return 1
    [ "$(head -c 2 "$3/signature")" = ED ] || return 1
    [ "$(head -c 10 "$3/key" | tail -c 8 | od -An -tx1)" = "$(head -c 10 "$3/signature" | tail -c 8 | od -An -tx1)" ] || return 1
    trusted=$(sed -n 3p "$2")
    case "$trusted" in 'trusted comment: '*) ;; *) return 1 ;; esac

    # The DER header of an Ed25519 public key, then the key.
    printf '\060\052\060\005\006\003\053\145\160\003\041\000' > "$3/key.der"
    tail -c 32 "$3/key" >> "$3/key.der"
    tail -c 64 "$3/signature" > "$3/signature.raw"
    openssl dgst -blake2b512 -binary "$1" > "$3/hash" || return 1
    openssl pkeyutl -verify -pubin -keyform DER -inkey "$3/key.der" -rawin \
        -in "$3/hash" -sigfile "$3/signature.raw" >/dev/null 2>&1 || return 1
    cp "$3/signature.raw" "$3/global.message"
    printf '%s' "${trusted#trusted comment: }" >> "$3/global.message"
    openssl pkeyutl -verify -pubin -keyform DER -inkey "$3/key.der" -rawin \
        -in "$3/global.message" -sigfile "$3/global" >/dev/null 2>&1
}

# Asks whether to continue without a signature check. The script itself can
# come from standard input, so the question goes to the terminal. Without a
# terminal, the default answer applies.
confirm_without_signature() {
    cat >&2 <<'MESSAGE'
qrow: neither minisign nor OpenSSL 3 is installed, so the script cannot verify
the signature of the release.

Without the signature check, the script compares the archive only with the
checksum from the same GitHub release. This finds a damaged download. It does
not find an archive that someone replaced together with its checksum, for
example after they got access to the GitHub repository. After the install,
Qrow verifies the signature of each update itself.

To verify this install, install minisign or OpenSSL 3, then run the script again.
MESSAGE
    if (: </dev/tty) 2>/dev/null; then
        printf 'Continue without the signature check? [Y/n] ' >/dev/tty
        read -r answer </dev/tty || answer=n
        case "$answer" in
            '' | [Yy] | [Yy][Ee][Ss]) ;;
            *) fail 'install stopped' ;;
        esac
    else
        printf 'qrow: there is no terminal to ask. Continuing without the signature check.\n' >&2
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
    signature=$work/SHA256SUMS.minisig
    if command -v minisign >/dev/null 2>&1; then
        fetch "$releases/SHA256SUMS.minisig" "$signature" || fail 'could not download the release signature'
        minisign -V -q -m "$work/SHA256SUMS" -P "$QROW_PUBLIC_KEY" || fail 'the release signature is not valid'
    elif openssl_ready; then
        fetch "$releases/SHA256SUMS.minisig" "$signature" || fail 'could not download the release signature'
        mkdir "$work/openssl"
        openssl_verify "$work/SHA256SUMS" "$signature" "$work/openssl" || fail 'the release signature is not valid'
    else
        confirm_without_signature
        signature=
    fi

    # A line is "CHECKSUM  ./Qrow-VERSION-linux-ARCH.tar.gz".
    line=$(grep -E "^[0-9a-f]{64}  (\./)?Qrow-[A-Za-z0-9.+-]+-linux-$arch\.tar\.gz\$" "$work/SHA256SUMS" | head -n 1)
    [ -n "$line" ] || fail "the latest release has no archive for $arch"
    expected=${line%% *}
    name=${line##* }
    name=${name#./}
    version=${name#Qrow-}
    version=${version%-linux-"$arch".tar.gz}
    # The signed trusted comment names the release, so the signature of
    # another release does not apply.
    if [ -n "$signature" ] && [ "$(sed -n 3p "$signature")" != "trusted comment: Qrow release v$version" ]; then
        fail "the release signature is not for Qrow $version"
    fi

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
