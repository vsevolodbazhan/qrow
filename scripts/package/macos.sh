#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands cargo rustc uv codesign ditto
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
case "$QROW_BUILD_PROFILE" in
    release) cargo build --locked --release --bin qrow ;;
    debug) cargo build --locked --bin qrow ;;
    *) echo "Use release or debug for QROW_BUILD_PROFILE." >&2; exit 1 ;;
esac
QROW_DIST_DIR="${QROW_DIST_DIR:-dist}"
QROW_BUNDLE="$QROW_DIST_DIR/Qrow.app"
mkdir -p "$QROW_BUNDLE/Contents/MacOS" "$QROW_BUNDLE/Contents/Resources"
QROW_ICON_SOURCE="assets/app-icons/macos/qrow.png"
uv run --locked python scripts/package/icon.py "$QROW_ICON_SOURCE" "$QROW_BUNDLE/Contents/Resources/Qrow.icns"
# Replace the executable atomically, including when an older build is still running.
cp "target/$QROW_BUILD_PROFILE/qrow" "$QROW_BUNDLE/Contents/MacOS/qrow.new"
mv -f "$QROW_BUNDLE/Contents/MacOS/qrow.new" "$QROW_BUNDLE/Contents/MacOS/qrow"
cp NOTICE "$QROW_BUNDLE/Contents/Resources/NOTICE"
cp LICENSE "$QROW_BUNDLE/Contents/Resources/LICENSE"
uv run --locked python scripts/package/notices.py "$QROW_BUNDLE/Contents/Resources/THIRD_PARTY_NOTICES.txt"
cat > "$QROW_BUNDLE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Qrow</string>
<key>CFBundleDisplayName</key><string>Qrow</string>
<key>CFBundleIdentifier</key><string>io.qrow.app</string>
<key>CFBundleExecutable</key><string>qrow</string>
<key>CFBundleIconFile</key><string>Qrow.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict></plist>
PLIST
codesign --force --deep --sign - "$QROW_BUNDLE"
codesign --verify --deep --strict "$QROW_BUNDLE"
ditto -c -k --keepParent "$QROW_BUNDLE" "$QROW_DIST_DIR/Qrow-macos.zip"
echo "Built $QROW_BUNDLE and $QROW_DIST_DIR/Qrow-macos.zip for $(uname -m)."
