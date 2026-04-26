#!/usr/bin/env bash
# Run cargo commands inside the deployd build environment (mirrors build-appimage.sh flow).
# Usage: ./check.sh [check|clippy|test|...] [extra cargo flags]
#
# Requires the deployd-appimage-build LXD container and deployd-build-env Docker image.
# If the Docker image doesn't exist yet: ./packaging/appimage/build-appimage.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
DOCKERFILE="$REPO_ROOT/packaging/appimage/Dockerfile"
LXD_CONTAINER="deployd-appimage-build"
CMD="${1:-check}"
FEATURES="loot,libarchive-fallback"

# ── LXD path (host) ──────────────────────────────────────────────────────────
if [ "${DEPLOYD_NO_LXD:-0}" != "1" ] \
    && command -v lxc &>/dev/null \
    && lxc info &>/dev/null 2>&1
then
    lxc start "$LXD_CONTAINER" 2>/dev/null || true
    lxc exec --force-noninteractive "$LXD_CONTAINER" -- \
        env DEPLOYD_NO_LXD=1 \
        bash /workspace/check.sh "$@"
    exit $?
fi

# ── Docker path (inside LXD) ─────────────────────────────────────────────────
DOCKERFILE_HASH=$(sha256sum "$DOCKERFILE" | cut -c1-12)
IMAGE_TAG="deployd-build-env:${DOCKERFILE_HASH}"

if ! docker image inspect "$IMAGE_TAG" &>/dev/null 2>&1; then
    IMAGE_TAG="deployd-build-env:latest"
fi

CARGO_CMD="cargo $CMD --features $FEATURES ${*:2}"
if [ "$CMD" = "clippy" ]; then
    CARGO_CMD="rustup component add clippy 2>/dev/null; $CARGO_CMD"
fi

docker run --rm \
    -v "$REPO_ROOT:/workspace:z" \
    --workdir /workspace \
    "$IMAGE_TAG" \
    sh -c "$CARGO_CMD"
