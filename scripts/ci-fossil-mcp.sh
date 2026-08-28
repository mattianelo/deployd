#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
source "$REPO_ROOT/packaging/appimage/mcp-versions.sh"
"$REPO_ROOT/scripts/ci-environment.sh"

WORKSPACE="${DEPLOYD_MCP_WORKSPACE:-}"
BINARY="$HOME/.local/lib/deployd-mcp/fossil-mcp-$FOSSIL_MCP_VERSION"

[ -n "$WORKSPACE" ] || {
    echo "error: DEPLOYD_MCP_WORKSPACE is required" >&2
    exit 1
}
WORKSPACE="$(realpath "$WORKSPACE")"
case "$WORKSPACE" in
    "$REPO_ROOT"/.ci-artifacts/*) ;;
    *)
        echo "error: MCP workspace must be an isolated CI artifact directory" >&2
        exit 1
        ;;
esac
[ -x "$BINARY" ] || {
    echo "error: pinned Fossil binary is unavailable" >&2
    exit 1
}

cd "$WORKSPACE"
export FOSSIL_NO_UPDATE_CHECK=1
exec "$BINARY" mcp
