#!/bin/bash
# Host-side wrapper — builds the Docker image if needed, then runs the inner script.
# Usage: bash packaging/appimage/build-appimage.sh [--rebuild]
#
# Prerequisites (snap Docker):
#   sudo snap install docker
#   sudo addgroup --system docker && sudo adduser "$USER" docker
#   sudo snap disable docker && sudo snap enable docker
#   newgrp docker

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DOCKERFILE="$REPO_ROOT/packaging/appimage/Dockerfile"
IMAGE_NAME="deployd-build-env"
INNER_SCRIPT="packaging/appimage/build-appimage-inner.sh"

REBUILD=0
for arg in "$@"; do
    case "$arg" in
        --rebuild) REBUILD=1 ;;
        *) echo "Unknown option: $arg"; exit 1 ;;
    esac
done

if [ "${DEPLOYD_NO_DOCKER:-0}" = "1" ]; then
    exec bash "$REPO_ROOT/$INNER_SCRIPT"
fi

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
    docker build --tag "$IMAGE_TAG" --tag "${IMAGE_NAME}:latest" --file "$DOCKERFILE" "$REPO_ROOT"
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
