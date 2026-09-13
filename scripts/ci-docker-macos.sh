#!/bin/sh
# Disposable Docker VM on the GitHub-hosted Intel Mac; never use a local profile.
set -eu
test "${GITHUB_ACTIONS:-}" = true && test "${RUNNER_OS:-}" = macOS && test "${RUNNER_ARCH:-}" = X64 || {
    echo 'This script requires a GitHub-hosted Intel macOS runner.' >&2
    exit 1
}
: "${RUNNER_TEMP:?Missing runner temporary directory}"
export COLIMA_HOME="$RUNNER_TEMP/qrow-colima"
export DOCKER_CONFIG="$RUNNER_TEMP/qrow-docker"
export DOCKER_HOST="unix://$COLIMA_HOME/qrow-e2e/docker.sock"
mkdir -p target/e2e-tools
case "${1:-}" in
    start)
        export HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_INSTALL_UPGRADE=1
        brew install colima docker docker-compose docker-buildx
        mkdir -p "$DOCKER_CONFIG/cli-plugins"
        ln -sf "$(brew --prefix docker-compose)/bin/docker-compose" "$DOCKER_CONFIG/cli-plugins/docker-compose"
        ln -sf "$(brew --prefix docker-buildx)/bin/docker-buildx" "$DOCKER_CONFIG/cli-plugins/docker-buildx"
        colima --profile qrow-e2e start --vm-type vz --mount-type virtiofs \
            --cpu 2 --memory 6 --disk 30 --runtime docker --activate=false
        printf 'COLIMA_HOME=%s\nDOCKER_CONFIG=%s\nDOCKER_HOST=%s\n' \
            "$COLIMA_HOME" "$DOCKER_CONFIG" "$DOCKER_HOST" >> "$GITHUB_ENV"
        colima version
        docker version
        docker compose version
        docker buildx version
        ;;
    stop)
        if command -v colima >/dev/null 2>&1; then
            colima --profile qrow-e2e status > target/e2e-tools/docker.log 2>&1 || true
            docker info >> target/e2e-tools/docker.log 2>&1 || true
            colima --profile qrow-e2e delete --force
        fi
        ;;
    *) echo 'Usage: ci-docker-macos.sh [start|stop]' >&2; exit 2 ;;
esac
