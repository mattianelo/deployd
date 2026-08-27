#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd -P)"
LXD_CONTAINER="deployd-appimage-build"
BUILD_PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

if [ "$#" -ne 0 ]; then
    echo "error: Fossil MCP wrapper accepts no arguments" >&2
    exit 2
fi

if ! command -v lxc >/dev/null 2>&1 || ! lxc info "$LXD_CONTAINER" >/dev/null 2>&1; then
    echo "error: LXD container $LXD_CONTAINER is unavailable" >&2
    exit 1
fi

WORKSPACE_SOURCE="$(lxc config device get "$LXD_CONTAINER" workspace source)"
if [ -z "$WORKSPACE_SOURCE" ] || [ "$(realpath "$WORKSPACE_SOURCE")" != "$REPO_ROOT" ]; then
    echo "error: $LXD_CONTAINER does not map this checkout to /workspace" >&2
    exit 1
fi

exec lxc exec "$LXD_CONTAINER" \
    --force-noninteractive \
    --cwd /workspace \
    --user 1000 \
    --group 1000 \
    --env HOME=/home/ubuntu \
    --env "PATH=$BUILD_PATH" \
    --env FOSSIL_NO_UPDATE_CHECK=1 \
    -- unshare --user --map-root-user --mount --net \
        bash /workspace/scripts/mcp/fossil-sandbox.sh
