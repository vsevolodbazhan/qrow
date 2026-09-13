#!/bin/sh
# Run against a disposable stack created by scripts/e2e/run.py, in a logged-in macOS session.
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands python3 swiftc
qrow_require_python_311
qrow_require_macos
qrow_require_xcode_tools
qrow_preflight_finish || exit 1
case "${1:-}" in
    --preflight) ;;
    --prepare) : "${QROW_E2E_ARTIFACTS:?Missing isolated E2E artifact directory}" ;;
    *)
        : "${QROW_E2E_ARTIFACTS:?Missing isolated E2E artifact directory}"
        : "${QROW_E2E_PORT:?Missing isolated fixture port}"
        ;;
esac
mkdir -p target/e2e-tools
if [ ! -x target/e2e-tools/native-driver ] || [ -n "$(find tests/e2e/native/Driver.swift -newer target/e2e-tools/native-driver -print)" ]; then
    swiftc -warnings-as-errors tests/e2e/native/Driver.swift -o target/e2e-tools/native-driver
fi
if ! target/e2e-tools/native-driver --preflight > target/e2e-tools/preflight.log 2>&1; then
    cat target/e2e-tools/preflight.log >&2
    exit 1
fi
test "${1:-}" != --preflight || exit 0
export QROW_DATA_DIR="$QROW_E2E_ARTIFACTS/workspace"
export QROW_DIST_DIR="$QROW_E2E_ARTIFACTS/package"
export QROW_E2E_BUNDLE="$QROW_DIST_DIR/Qrow.app"
mkdir -p "$QROW_DATA_DIR"
cleanup() {
    python3 scripts/e2e/keychain.py
}
trap cleanup 0
trap 'exit 130' INT
trap 'exit 143' TERM
if [ "${1:-}" != --prepared ]; then
    sh scripts/package/macos.sh
    python3 scripts/core/size.py
fi
test "${1:-}" != --prepare || exit 0
target/e2e-tools/native-driver
