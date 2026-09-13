#!/bin/sh
# Run against a disposable stack created by scripts/e2e.py, in a logged-in macOS session.
set -eu
cd "$(dirname "$0")/.."
test "$(uname -s)" = Darwin || { echo 'Native UI tests require macOS.' >&2; exit 1; }
mkdir -p target/e2e-tools
if [ ! -x target/e2e-tools/native-driver ] || [ tests/e2e/native/Driver.swift -nt target/e2e-tools/native-driver ]; then
    swiftc -warnings-as-errors tests/e2e/native/Driver.swift -o target/e2e-tools/native-driver
fi
if ! target/e2e-tools/native-driver --preflight > target/e2e-tools/preflight.log 2>&1; then
    cat target/e2e-tools/preflight.log >&2
    exit 1
fi
test "${1:-}" != --preflight || exit 0
: "${QROW_E2E_ARTIFACTS:?Run python3 scripts/e2e.py native-ui}"
: "${QROW_E2E_PORT:?Missing isolated fixture port}"
export QROW_DATA_DIR="$QROW_E2E_ARTIFACTS/workspace"
export QROW_DIST_DIR="$QROW_E2E_ARTIFACTS/package"
export QROW_E2E_BUNDLE="$QROW_DIST_DIR/Qrow.app"
mkdir -p "$QROW_DATA_DIR"
cleanup() {
    python3 scripts/native-e2e-cleanup.py
}
trap cleanup 0
trap 'exit 130' INT
trap 'exit 143' TERM
sh scripts/package-macos.sh
python3 scripts/check-size.py
target/e2e-tools/native-driver
