#!/bin/bash
# Builds the Snap in a dedicated Ubuntu 24.04 LXD container.
# Usage: bash packaging/snap/build-snap.sh [--development] [--rebuild] [--clean]

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LXD_CONTAINER="deployd-snap-build"
SETUP_SCRIPT="packaging/snap/setup-lxd.sh"
SNAP_BUILD_DIR="/build/deployd-snap"
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -1)"
HOST_OUTPUT_DIR="$REPO_ROOT/out/snap"

if [ -z "$VERSION" ]; then
    echo "ERROR: failed to read the package version from Cargo.toml." >&2
    exit 1
fi

case "$SNAP_BUILD_DIR" in
    /build/*) ;;
    *)
        echo "ERROR: Snap scratch directory must remain below /build." >&2
        exit 1
        ;;
esac

REBUILD=0
CLEAN=0
DEVELOPMENT=0
for arg in "$@"; do
    case "$arg" in
        --development) DEVELOPMENT=1 ;;
        --rebuild) REBUILD=1 ;;
        --clean) CLEAN=1 ;;
        *) echo "Unknown option: $arg"; exit 1 ;;
    esac
done

if [ "$DEVELOPMENT" = "1" ]; then
    SNAP_NAME="deployd-dev"
    SNAP_RECIPE="snap/snapcraft-dev.yaml"
else
    SNAP_NAME="deployd"
    SNAP_RECIPE="snap/snapcraft.yaml"
fi
SNAP_ARTIFACT="${SNAP_NAME}_${VERSION}_amd64.snap"

if ! command -v lxc &>/dev/null || ! lxc info &>/dev/null 2>&1; then
    echo "ERROR: LXD is not available to the current user." >&2
    exit 1
fi

if [ "$REBUILD" = "1" ] && lxc info "$LXD_CONTAINER" &>/dev/null 2>&1; then
    echo "==> Destroying LXD container $LXD_CONTAINER (--rebuild)"
    lxc delete --force "$LXD_CONTAINER"
fi

if ! lxc info "$LXD_CONTAINER" &>/dev/null 2>&1; then
    echo "==> Creating LXD container $LXD_CONTAINER"
    lxc launch ubuntu:24.04 "$LXD_CONTAINER"
    lxc config device add "$LXD_CONTAINER" workspace disk \
        source="$REPO_ROOT" path=/workspace shift=true

    echo "==> Provisioning $LXD_CONTAINER"
    lxc exec "$LXD_CONTAINER" -- bash "/workspace/$SETUP_SCRIPT"
else
    lxc start "$LXD_CONTAINER" 2>/dev/null || true
fi

echo "==> Copying source into container-owned Snap build storage"
lxc exec "$LXD_CONTAINER" -- python3 /workspace/scripts/package_source.py \
    /workspace "$SNAP_BUILD_DIR"

if [ "$DEVELOPMENT" = "1" ]; then
    echo "==> Selecting isolated development Snap recipe"
    lxc exec "$LXD_CONTAINER" -- install -m 0644 \
        "$SNAP_BUILD_DIR/$SNAP_RECIPE" "$SNAP_BUILD_DIR/snap/snapcraft.yaml"
fi

echo "==> Building Snap inside $LXD_CONTAINER"
lxc exec "$LXD_CONTAINER" --cwd "$SNAP_BUILD_DIR" -- \
    env PATH="/root/.cargo/bin:/snap/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    snapcraft pack --destructive-mode

mkdir -p "$HOST_OUTPUT_DIR"
lxc file pull "$LXD_CONTAINER/$SNAP_BUILD_DIR/$SNAP_ARTIFACT" \
    "$HOST_OUTPUT_DIR/$SNAP_ARTIFACT"
chmod 0644 "$HOST_OUTPUT_DIR/$SNAP_ARTIFACT"

EXPECTED_OWNER="$(id -u):$(id -g)"
ACTUAL_OWNER="$(stat -c '%u:%g' "$HOST_OUTPUT_DIR/$SNAP_ARTIFACT")"
if [ "$ACTUAL_OWNER" != "$EXPECTED_OWNER" ]; then
    echo "ERROR: exported Snap has unexpected host ownership $ACTUAL_OWNER." >&2
    exit 1
fi

if [ "$CLEAN" = "1" ]; then
    echo "==> Cleaning up LXD container $LXD_CONTAINER (--clean)"
    lxc delete --force "$LXD_CONTAINER"
fi

echo
echo "Snap build complete: $HOST_OUTPUT_DIR/$SNAP_ARTIFACT"
