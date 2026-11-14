#!/bin/sh
# Native renderer checks with demo data; no database or Keychain access.
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands swiftc ditto uv
qrow_require_macos
qrow_require_xcode_tools
qrow_preflight_finish || exit 1
: "${QROW_E2E_ARTIFACTS:?Missing isolated test artifact directory}"
qrow_tools="${CARGO_TARGET_DIR:-target}/e2e-tools"
qrow_driver="$qrow_tools/native-driver"
mkdir -p "$qrow_tools"
if [ "${QROW_NATIVE_PREPARED:-false}" = true ]; then
    test -x "$qrow_driver" || { echo "Missing prepared native driver." >&2; exit 1; }
elif [ ! -x "$qrow_driver" ] || [ -n "$(find tests/desktop/Driver.swift -newer "$qrow_driver" -print)" ]; then
    swiftc -warnings-as-errors tests/desktop/Driver.swift -o "$qrow_driver"
fi
export QROW_E2E_BUNDLE="${QROW_E2E_BUNDLE:-$QROW_E2E_ARTIFACTS/package/Qrow.app}"
export QROW_NO_UPDATES=1
if [ "${QROW_NATIVE_PREPARED:-false}" = true ]; then
    : "${QROW_MACOS_PACKAGE:?Missing prepared macOS package}"
    test -f "$QROW_MACOS_PACKAGE" || { echo "Missing prepared macOS package." >&2; exit 1; }
    test ! -e "$QROW_E2E_BUNDLE" || { echo "Refusing to overwrite the test bundle." >&2; exit 1; }
    mkdir -p "$QROW_E2E_ARTIFACTS/package"
    ditto -x -k "$QROW_MACOS_PACKAGE" "$QROW_E2E_ARTIFACTS/package"
elif [ ! -d "$QROW_E2E_BUNDLE" ]; then
    QROW_DIST_DIR="$QROW_E2E_ARTIFACTS/package" QROW_BUILD_PROFILE=release QROW_PACKAGE_PREBUILT=false \
        sh scripts/package/macos.sh
    QROW_DIST_DIR="$QROW_E2E_ARTIFACTS/package" uv run --locked python scripts/core/size.py
fi
if [ ! -f "$QROW_E2E_BUNDLE/Contents/Info.plist" ] || [ ! -x "$QROW_E2E_BUNDLE/Contents/MacOS/qrow" ]; then
    echo "Invalid renderer test application bundle." >&2
    exit 1
fi
if [ "${1:-}" = --prepare ]; then exit 0; fi
qrow_renderer="${QROW_E2E_RENDERER:-software}"
case "$qrow_renderer" in
    software) unset QROW_RENDERER; export QROW_SOFTWARE_RENDERER=1; qrow_renderer_log='Qrow: using CPU rendering' ;;
    opengl) unset QROW_SOFTWARE_RENDERER; export QROW_RENDERER=opengl; qrow_renderer_log='Qrow: using OpenGL rendering' ;;
    *) echo "Unknown test renderer: $qrow_renderer" >&2; exit 1 ;;
esac
for scenario in editor-highlight-only results-text-only software-input-only renderer-lifecycle-only renderer-blank-only; do
    qrow_scenario_dir="$QROW_E2E_ARTIFACTS/$scenario"
    mkdir -p "$qrow_scenario_dir/workspace"
    QROW_E2E_ARTIFACTS="$qrow_scenario_dir" QROW_DATA_DIR="$qrow_scenario_dir/workspace" \
        "$qrow_driver" "--$scenario"
    grep -q "$qrow_renderer_log" "$qrow_scenario_dir/qrow.log"
    if [ "$qrow_renderer" = opengl ]; then
        grep -q 'hardware accelerated' "$qrow_scenario_dir/qrow.log"
        grep -q 'core profile; instanced' "$qrow_scenario_dir/qrow.log"
        if grep -q 'OpenGL drawing failed' "$qrow_scenario_dir/qrow.log"; then
            echo 'OpenGL frame failed; see the application log.' >&2
            exit 1
        fi
    fi
done
