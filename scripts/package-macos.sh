#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
test "$(uname -s)" = Darwin || { echo "Packaging requires macOS." >&2; exit 1; }
QROW_BUILD_PROFILE="${QROW_BUILD_PROFILE:-release}"
case "$QROW_BUILD_PROFILE" in
    release) cargo build --locked --release --bin qrow ;;
    debug) cargo build --locked --bin qrow ;;
    *) echo "Use release or debug for QROW_BUILD_PROFILE." >&2; exit 1 ;;
esac
QROW_BUNDLE="dist/Qrow.app"
mkdir -p "$QROW_BUNDLE/Contents/MacOS" "$QROW_BUNDLE/Contents/Resources"
QROW_ICON_SOURCE="assets/app-icons/macos/qrow.png"
test -f "$QROW_ICON_SOURCE" || { echo "Missing application icon: $QROW_ICON_SOURCE" >&2; exit 1; }
QROW_ICON_TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/qrow-icon.XXXXXX")"
QROW_ICONSET="$QROW_ICON_TEMP_DIR/Qrow.iconset"
trap 'rm -rf "$QROW_ICON_TEMP_DIR"' EXIT HUP INT TERM
mkdir "$QROW_ICONSET"
swift scripts/inset-icon.swift "$QROW_ICON_SOURCE" "$QROW_ICON_TEMP_DIR/Qrow.png"

make_icon_size() {
    sips -z "$1" "$1" "$QROW_ICON_TEMP_DIR/Qrow.png" --out "$QROW_ICONSET/$2" >/dev/null
}

make_icon_size 16 icon_16x16.png
make_icon_size 32 icon_16x16@2x.png
make_icon_size 32 icon_32x32.png
make_icon_size 64 icon_32x32@2x.png
make_icon_size 128 icon_128x128.png
make_icon_size 256 icon_128x128@2x.png
make_icon_size 256 icon_256x256.png
make_icon_size 512 icon_256x256@2x.png
make_icon_size 512 icon_512x512.png
make_icon_size 1024 icon_512x512@2x.png
iconutil -c icns "$QROW_ICONSET" -o "$QROW_BUNDLE/Contents/Resources/Qrow.icns"
# Replace the executable atomically, including when an older build is still running.
cp "target/$QROW_BUILD_PROFILE/qrow" "$QROW_BUNDLE/Contents/MacOS/qrow.new"
mv -f "$QROW_BUNDLE/Contents/MacOS/qrow.new" "$QROW_BUNDLE/Contents/MacOS/qrow"
cp NOTICE "$QROW_BUNDLE/Contents/Resources/NOTICE"
cp LICENSE "$QROW_BUNDLE/Contents/Resources/LICENSE"
python3 scripts/third-party-notices.py "$QROW_BUNDLE/Contents/Resources/THIRD_PARTY_NOTICES.txt"
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
ditto -c -k --keepParent "$QROW_BUNDLE" dist/Qrow-macos.zip
echo "Built $QROW_BUNDLE and dist/Qrow-macos.zip for $(uname -m)."
