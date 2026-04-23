#!/bin/bash
# Host-side wrapper — runs inside LXD (preferred) or directly via Docker.
# Usage: bash packaging/appimage/build-appimage.sh [--rebuild] [--clean]
#
# Prerequisites (LXD — preferred):
#   sudo snap install lxd
#   sudo lxd init --minimal
#   sudo adduser "$USER" lxd && newgrp lxd
#
# Prerequisites (Docker fallback, if LXD not available):
#   sudo snap install docker
#   sudo addgroup --system docker && sudo adduser "$USER" docker
#   sudo snap disable docker && sudo snap enable docker
#   newgrp docker
#
# Environment variables:
#   DEPLOYD_NO_LXD=1    skip LXD, run Docker directly (set automatically inside LXD)
#   DEPLOYD_NO_DOCKER=1 skip Docker, run inner script directly (CI / Docker-in-Docker)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DOCKERFILE="$REPO_ROOT/packaging/appimage/Dockerfile"
IMAGE_NAME="deployd-build-env"
INNER_SCRIPT="packaging/appimage/build-appimage-inner.sh"
LXD_CONTAINER="deployd-appimage-build"

REBUILD=0
CLEAN=0
for arg in "$@"; do
    case "$arg" in
        --rebuild) REBUILD=1 ;;
        --clean)   CLEAN=1 ;;
        *) echo "Unknown option: $arg"; exit 1 ;;
    esac
done

if [ "${DEPLOYD_NO_DOCKER:-0}" = "1" ]; then
    exec bash "$REPO_ROOT/$INNER_SCRIPT"
fi

# ── LXD path ────────────────────────────────────────────────────────────────
_lxd_available() {
    [ "${DEPLOYD_NO_LXD:-0}" != "1" ] \
        && command -v lxc &>/dev/null \
        && lxc info &>/dev/null 2>&1
}

if _lxd_available; then
    if [ "$REBUILD" = "1" ] && lxc info "$LXD_CONTAINER" &>/dev/null 2>&1; then
        echo "==> Destroying LXD container $LXD_CONTAINER (--rebuild)"
        lxc delete --force "$LXD_CONTAINER"
    fi

    if ! lxc info "$LXD_CONTAINER" &>/dev/null 2>&1; then
        echo "==> Creating LXD container $LXD_CONTAINER"
        lxc launch ubuntu:24.04 "$LXD_CONTAINER" \
            -c security.nesting=true \
            -c security.syscalls.intercept.mknod=true \
            -c security.syscalls.intercept.setxattr=true

        lxc config device add "$LXD_CONTAINER" workspace disk \
            source="$REPO_ROOT" path=/workspace shift=true

        echo "==> Installing Docker inside $LXD_CONTAINER"
        lxc exec "$LXD_CONTAINER" -- bash -euo pipefail -c '
            apt-get update -qq
            apt-get install -y docker.io
        '

        echo "==> Waiting for Docker daemon inside $LXD_CONTAINER"
        lxc exec "$LXD_CONTAINER" -- bash -c '
            for i in $(seq 1 30); do
                docker info &>/dev/null && exit 0
                sleep 2
            done
            echo "ERROR: Docker daemon not ready after 60s" >&2; exit 1
        '
    else
        lxc start "$LXD_CONTAINER" 2>/dev/null || true
    fi

    REBUILD_FLAG=""
    [ "$REBUILD" = "1" ] && REBUILD_FLAG="--rebuild"

    echo "==> Running build inside LXD container $LXD_CONTAINER"
    # shellcheck disable=SC2086
    lxc exec "$LXD_CONTAINER" -- \
        env DEPLOYD_NO_LXD=1 \
        bash /workspace/packaging/appimage/build-appimage.sh $REBUILD_FLAG

    if [ "$CLEAN" = "1" ]; then
        echo "==> Cleaning up LXD container $LXD_CONTAINER (--clean)"
        lxc delete --force "$LXD_CONTAINER"
    fi

    echo ""
    echo "Build complete: $REPO_ROOT/Deployd-x86_64.AppImage"
    exit 0
fi

# ── Docker path ──────────────────────────────────────────────────────────────
if ! command -v docker &>/dev/null; then
    echo "ERROR: 'docker' not found." >&2
    echo "  sudo snap install docker" >&2
    echo "  sudo addgroup --system docker && sudo adduser \"\$USER\" docker" >&2
    echo "  sudo snap disable docker && sudo snap enable docker" >&2
    exit 1
fi

if ! docker info &>/dev/null; then
    echo "ERROR: Docker daemon not accessible. Add your user to the docker group:" >&2
    echo "  sudo adduser \"\$USER\" docker && sudo snap disable docker && sudo snap enable docker" >&2
    exit 1
fi

DOCKERFILE_HASH=$(sha256sum "$DOCKERFILE" | cut -c1-12)
IMAGE_TAG="${IMAGE_NAME}:${DOCKERFILE_HASH}"

if [ "$REBUILD" = "1" ] || ! docker image inspect "$IMAGE_TAG" &>/dev/null; then
    echo "==> Building Docker image $IMAGE_TAG"
    docker build --tag "$IMAGE_TAG" --tag "${IMAGE_NAME}:latest" --file "$DOCKERFILE" "$(dirname "$DOCKERFILE")"
else
    echo "==> Reusing cached Docker image $IMAGE_TAG"
fi

VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*= *"\(.*\)"/\1/')
echo "==> Running build inside container (Deployd $VERSION)"

docker run --rm \
    -v "$REPO_ROOT:/workspace:z" \
    -e VERSION="$VERSION" \
    --workdir /workspace \
    "$IMAGE_TAG" \
    bash "$INNER_SCRIPT"

echo ""
echo "Build complete: $REPO_ROOT/Deployd-x86_64.AppImage"
