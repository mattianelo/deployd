#!/bin/bash
# Builds the Snap in a dedicated Ubuntu 24.04 LXD container.
# Usage: bash packaging/snap/build-snap.sh [--rebuild] [--clean]

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LXD_CONTAINER="deployd-snap-build"
SETUP_SCRIPT="packaging/snap/setup-lxd.sh"

REBUILD=0
CLEAN=0
for arg in "$@"; do
    case "$arg" in
        --rebuild) REBUILD=1 ;;
        --clean) CLEAN=1 ;;
        *) echo "Unknown option: $arg"; exit 1 ;;
    esac
done

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

echo "==> Building Snap inside $LXD_CONTAINER"
lxc exec "$LXD_CONTAINER" --cwd /workspace -- \
    env PATH="/root/.cargo/bin:/snap/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    snapcraft pack --destructive-mode

if [ "$CLEAN" = "1" ]; then
    echo "==> Cleaning up LXD container $LXD_CONTAINER (--clean)"
    lxc delete --force "$LXD_CONTAINER"
fi

echo
echo "Snap build complete."
