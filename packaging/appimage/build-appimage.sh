#!/bin/bash
# Host-side wrapper — runs local AppImage builds inside LXD.
# Usage: bash packaging/appimage/build-appimage.sh [--rebuild] [--clean] [--debug] [--setup-only]
#
# Flags:
#   --rebuild  Destroy and recreate the LXD container / rebuild the Docker image.
#   --clean    Delete the LXD container after a successful build.
#   --debug    Compile in debug mode (faster build, larger binary, no optimisations).
#   --setup-only  Provision the build environment without building an AppImage.
#
# Prerequisites:
#   sudo snap install lxd
#   sudo lxd init --minimal
#   sudo adduser "$USER" lxd && newgrp lxd
#
#   On first run the container is created and provisioned automatically via
#   setup-lxd.sh (apt packages + Rust + AppImage tools).  Subsequent builds
#   reuse the container without re-downloading anything.  Use --rebuild to
#   recreate the container from scratch.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INNER_SCRIPT="packaging/appimage/build-appimage-inner.sh"
SETUP_SCRIPT="packaging/appimage/setup-lxd.sh"
USER_SETUP_SCRIPT="packaging/appimage/setup-user.sh"
LXD_CONTAINER="deployd-appimage-build"
BUILD_UID="1000"
BUILD_GID="1000"
BUILD_HOME="/home/ubuntu"
BUILD_PATH="$BUILD_HOME/.cargo/bin:/opt/appimage-tools:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
RUST_VERSION="$(sed -n 's/^channel = "\([^"]*\)"/\1/p' "$REPO_ROOT/rust-toolchain.toml" | head -1)"

if [ -z "$RUST_VERSION" ]; then
    echo "ERROR: failed to read the pinned Rust version." >&2
    exit 1
fi

REBUILD=0
CLEAN=0
DEBUG=0
SETUP_ONLY=0
for arg in "$@"; do
    case "$arg" in
        --rebuild) REBUILD=1 ;;
        --clean)   CLEAN=1 ;;
        --debug)   DEBUG=1 ;;
        --setup-only) SETUP_ONLY=1 ;;
        *) echo "Unknown option: $arg"; exit 1 ;;
    esac
done

_lxd_available() {
    [ "${DEPLOYD_NO_LXD:-0}" != "1" ] \
        && command -v lxc &>/dev/null \
        && lxc info &>/dev/null 2>&1
}

if ! _lxd_available; then
    echo "ERROR: LXD is not available to the current user." >&2
    exit 1
fi

if [ "$REBUILD" = "1" ] && lxc info "$LXD_CONTAINER" &>/dev/null 2>&1; then
    echo "==> Destroying LXD container $LXD_CONTAINER (--rebuild)"
    lxc delete --force "$LXD_CONTAINER"
fi

if ! lxc info "$LXD_CONTAINER" &>/dev/null 2>&1; then
    echo "==> Creating LXD container $LXD_CONTAINER"
    lxc launch ubuntu:24.04 "$LXD_CONTAINER" \
        -c security.syscalls.intercept.mknod=true \
        -c security.syscalls.intercept.setxattr=true

    lxc config device add "$LXD_CONTAINER" workspace disk \
        source="$REPO_ROOT" path=/workspace shift=true

    echo "==> Provisioning system dependencies (first-time setup)"
    lxc exec "$LXD_CONTAINER" -- bash "/workspace/$SETUP_SCRIPT"
else
    lxc start "$LXD_CONTAINER" 2>/dev/null || true
fi

lxc exec "$LXD_CONTAINER" -- install -d -o "$BUILD_UID" -g "$BUILD_GID" /build
lxc exec "$LXD_CONTAINER" -- chown -R "$BUILD_UID:$BUILD_GID" /build

if ! lxc exec "$LXD_CONTAINER" \
    --user "$BUILD_UID" \
    --group "$BUILD_GID" \
    --env "HOME=$BUILD_HOME" \
    --env "PATH=$BUILD_PATH" \
    --env "EXPECTED_RUST_VERSION=$RUST_VERSION" \
    -- sh -c 'rustc --version | grep -q "^rustc $EXPECTED_RUST_VERSION " && rust-analyzer --version >/dev/null && command -v cargo-nextest >/dev/null && command -v cargo-audit >/dev/null'
then
    echo "==> Provisioning Rust tools for the non-root build user"
    lxc exec "$LXD_CONTAINER" \
        --cwd /workspace \
        --user "$BUILD_UID" \
        --group "$BUILD_GID" \
        --env "HOME=$BUILD_HOME" \
        --env "PATH=$BUILD_PATH" \
        -- bash "/workspace/$USER_SETUP_SCRIPT"
fi

if [ "$SETUP_ONLY" = "1" ]; then
    echo "Build environment ready: $LXD_CONTAINER"
    exit 0
fi

INNER_FLAGS=()
[ "$DEBUG" = "1" ] && INNER_FLAGS+=(--debug)

echo "==> Running build inside LXD container $LXD_CONTAINER"
lxc exec "$LXD_CONTAINER" \
    --cwd /workspace \
    --user "$BUILD_UID" \
    --group "$BUILD_GID" \
    --env "HOME=$BUILD_HOME" \
    --env "PATH=$BUILD_PATH" \
    --env APPIMAGE_EXTRACT_AND_RUN=1 \
    --env CARGO_TARGET_DIR=/build/target \
    -- bash "/workspace/$INNER_SCRIPT" "${INNER_FLAGS[@]}"

if [ "$CLEAN" = "1" ]; then
    echo "==> Cleaning up LXD container $LXD_CONTAINER (--clean)"
    lxc delete --force "$LXD_CONTAINER"
fi

echo ""
echo "Build complete: $REPO_ROOT/Deployd-x86_64.AppImage"
