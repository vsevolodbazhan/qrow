#!/bin/sh
# Native renderer checks with demo data; no database or Keychain access.
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands swiftc
qrow_require_macos
qrow_require_xcode_tools
qrow_preflight_finish || exit 1
: "${QROW_E2E_ARTIFACTS:?Missing isolated test artifact directory}"
qrow_tools="${CARGO_TARGET_DIR:-target}/e2e-tools"
qrow_driver="$qrow_tools/native-driver"
mkdir -p "$qrow_tools"
if [ ! -x "$qrow_driver" ] || [ -n "$(find tests/desktop/Driver.swift -newer "$qrow_driver" -print)" ]; then
    swiftc -warnings-as-errors tests/desktop/Driver.swift -o "$qrow_driver"
fi
export QROW_E2E_BUNDLE="${QROW_E2E_BUNDLE:-$QROW_E2E_ARTIFACTS/package/Qrow.app}"
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
