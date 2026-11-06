#!/bin/sh
# Build a Linux archive that can be installed under a user prefix.
set -eu
cd "$(dirname "$0")/../.."
case "$(uname -m)" in
    x86_64|aarch64) qrow_arch=$(uname -m) ;;
    *) echo 'Unsupported Linux architecture' >&2; exit 1 ;;
esac
[ "$(uname -s)" = Linux ] || { echo 'Linux is required' >&2; exit 1; }
[ -z "${CARGO_BUILD_TARGET:-}" ] || { echo 'Unset CARGO_BUILD_TARGET for a native package' >&2; exit 1; }
qrow_version=${QROW_RELEASE_VERSION:-$(cargo metadata --locked --no-deps --format-version 1 | uv run --locked python scripts/package/version.py)}
case "$qrow_version" in *[!a-zA-Z0-9.+-]*|'') echo 'Invalid release version' >&2; exit 1 ;; esac
cargo build --locked --release --bin qrow
qrow_package=$(mktemp -d)
trap 'rm -rf "$qrow_package"' EXIT HUP INT TERM
mkdir -p "$qrow_package/bin" "$qrow_package/share/applications" "$qrow_package/share/icons/hicolor/1024x1024/apps" "$qrow_package/share/licenses/qrow"
cp "${CARGO_TARGET_DIR:-target}/release/qrow" "$qrow_package/bin/qrow"
cp assets/app-icons/macos/qrow.png "$qrow_package/share/icons/hicolor/1024x1024/apps/qrow.png"
cp LICENSE NOTICE "$qrow_package/share/licenses/qrow/"
uv run --locked python scripts/package/notices.py "$qrow_package/share/licenses/qrow/THIRD_PARTY_NOTICES.txt"
cat > "$qrow_package/share/applications/qrow.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Qrow
Comment=SQL workbench
Exec=qrow
Icon=qrow
Terminal=false
Categories=Development;Database;
DESKTOP
qrow_output=${QROW_DIST_DIR:-target/package}
mkdir -p "$qrow_output"
tar -C "$qrow_package" -czf "$qrow_output/Qrow-$qrow_version-linux-$qrow_arch.tar.gz" .
printf 'Created %s\n' "$qrow_output/Qrow-$qrow_version-linux-$qrow_arch.tar.gz"
