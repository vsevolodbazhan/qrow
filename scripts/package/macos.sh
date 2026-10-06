#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands cargo rustc uv codesign ditto hdiutil lipo
qrow_require_macos
qrow_require_xcode_tools
for file in Cargo.lock LICENSE NOTICE pyproject.toml uv.lock assets/app-icons/macos/qrow.png; do
    if [ ! -f "$file" ]; then
        echo "Missing required file: $file" >&2
        qrow_preflight_failed=1
    fi
done
qrow_preflight_finish || exit 1

QROW_BUILD_PROFILE="${QROW_BUILD_PROFILE:-release}"
export QROW_BUILD_PROFILE
QROW_MACOS_MINIMUM="$(uv run --locked python scripts/package/macos_platform.py minimum)"
export MACOSX_DEPLOYMENT_TARGET="$QROW_MACOS_MINIMUM"
qrow_executable="$(uv run --locked python scripts/package/macos_platform.py executable)"
qrow_architecture="$(uv run --locked python scripts/package/macos_platform.py architecture)"
case "$QROW_BUILD_PROFILE" in
    release) cargo build --locked --release --bin qrow ;;
    debug) cargo build --locked --bin qrow ;;
    *) echo "Use release or debug for QROW_BUILD_PROFILE." >&2; exit 1 ;;
esac
QROW_DIST_DIR="${QROW_DIST_DIR:-dist}"
if [ "$(lipo -archs "$qrow_executable")" != "$qrow_architecture" ]; then
    echo "Cargo executable does not match the package architecture: $qrow_architecture" >&2
    exit 1
fi
QROW_BUNDLE="$QROW_DIST_DIR/Qrow.app"
mkdir -p "$QROW_BUNDLE/Contents/MacOS" "$QROW_BUNDLE/Contents/Resources"
QROW_ICON_SOURCE="assets/app-icons/macos/qrow.png"
uv run --locked python scripts/package/icon.py "$QROW_ICON_SOURCE" "$QROW_BUNDLE/Contents/Resources/Qrow.icns"
# Replace the executable atomically, including when an older build is still running.
cp "$qrow_executable" "$QROW_BUNDLE/Contents/MacOS/qrow.new"
mv -f "$QROW_BUNDLE/Contents/MacOS/qrow.new" "$QROW_BUNDLE/Contents/MacOS/qrow"
cp NOTICE "$QROW_BUNDLE/Contents/Resources/NOTICE"
cp LICENSE "$QROW_BUNDLE/Contents/Resources/LICENSE"
uv run --locked python scripts/package/notices.py "$QROW_BUNDLE/Contents/Resources/THIRD_PARTY_NOTICES.txt"
# Cargo.toml is the only place that holds the version.
QROW_VERSION="$(cargo metadata --locked --no-deps --format-version 1 |
    uv run --locked python scripts/package/version.py)"
cat > "$QROW_BUNDLE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Qrow</string>
<key>CFBundleDisplayName</key><string>Qrow</string>
<key>CFBundleIdentifier</key><string>io.qrow.app</string>
<key>CFBundleExecutable</key><string>qrow</string>
<key>CFBundleIconFile</key><string>Qrow.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$QROW_VERSION</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>$QROW_MACOS_MINIMUM</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict></plist>
PLIST
codesign --force --deep --sign - "$QROW_BUNDLE"
codesign --verify --deep --strict "$QROW_BUNDLE"
ditto -c -k --keepParent "$QROW_BUNDLE" "$QROW_DIST_DIR/Qrow-macos.zip"
QROW_DMG="$QROW_DIST_DIR/Qrow-$QROW_VERSION-$qrow_architecture.dmg"
uv run --locked python scripts/package/dmg.py build \
    --app "$QROW_BUNDLE" \
    --output "$QROW_DMG" \
    --volume-name "Qrow $QROW_VERSION"
echo "Built $QROW_BUNDLE, $QROW_DIST_DIR/Qrow-macos.zip, and $QROW_DMG for $qrow_architecture (macOS $QROW_MACOS_MINIMUM+)."
