#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
source "$REPO_ROOT/packaging/appimage/mcp-versions.sh"
"$REPO_ROOT/scripts/ci-environment.sh"

WORKSPACE="${DEPLOYD_MCP_WORKSPACE:-}"
BINARY="$HOME/.local/lib/deployd-mcp/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION"

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
    echo "error: pinned rust-analyzer MCP binary is unavailable" >&2
    exit 1
}

exec "$BINARY" \
    --features loot,libarchive-fallback \
    --config "cargo.targetDir=${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is required}" \
    -- "$WORKSPACE"
