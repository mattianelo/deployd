#!/usr/bin/env bash
# Run approved Cargo commands inside the Deployd LXD build environment.
set -euo pipefail

trap '' PIPE

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
LXD_CONTAINER="deployd-appimage-build"
BUILD_UID="1000"
BUILD_GID="1000"
BUILD_HOME="/home/ubuntu"
BUILD_PATH="$BUILD_HOME/.cargo/bin:/opt/appimage-tools:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
CMD="${1:-check}"
shift "$(( $# > 0 ? 1 : 0 ))"

DEPLOYD_CI_FRESHNESS="${DEPLOYD_CI_FRESHNESS:-0}" \
    "$REPO_ROOT/scripts/rust-command.sh" validate "$CMD" "$@"

if [ "${DEPLOYD_BUILD_CONTAINER:-0}" != "1" ]; then
    if ! command -v lxc &>/dev/null || ! lxc info &>/dev/null 2>&1; then
        echo "error: LXD is required for ./check.sh" >&2
        echo "hint: ensure 'lxc info' works and the '$LXD_CONTAINER' container exists" >&2
        exit 1
    fi

    lxc start "$LXD_CONTAINER" 2>/dev/null || true
    exec lxc exec --force-noninteractive "$LXD_CONTAINER" \
        --cwd /workspace \
        --user "$BUILD_UID" \
        --group "$BUILD_GID" \
        --env "HOME=$BUILD_HOME" \
        --env "PATH=$BUILD_PATH" \
        --env DEPLOYD_BUILD_CONTAINER=1 \
        --env "DEPLOYD_DEPENDENCY_MAINTENANCE=${DEPLOYD_DEPENDENCY_MAINTENANCE:-0}" \
        --env "DEPLOYD_CI_FRESHNESS=${DEPLOYD_CI_FRESHNESS:-0}" \
        --env "DEPLOYD_EXPERIMENTAL=${DEPLOYD_EXPERIMENTAL:-0}" \
        --env APPIMAGE_EXTRACT_AND_RUN=1 \
        --env CARGO_TARGET_DIR=/build/target \
        -- bash /workspace/check.sh "$CMD" "$@"
fi

if [ "$(id -u)" != "$BUILD_UID" ] || [ "$(id -g)" != "$BUILD_GID" ]; then
    echo "error: ./check.sh must run as UID $BUILD_UID and GID $BUILD_GID inside LXD" >&2
    exit 1
fi

if [ "$REPO_ROOT" != "/workspace" ]; then
    echo "error: container workspace must be mounted at /workspace" >&2
    exit 1
fi

if [ ! -x "$BUILD_HOME/.cargo/bin/cargo" ]; then
    echo "error: the non-root Rust toolchain is not provisioned" >&2
    echo "hint: run 'bash packaging/appimage/build-appimage.sh --setup-only'" >&2
    exit 1
fi

exec "$REPO_ROOT/scripts/rust-command.sh" run "$CMD" "$@"
