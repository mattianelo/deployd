#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
"$REPO_ROOT/scripts/ci-environment.sh"

WORKSPACE="$REPO_ROOT/.ci-artifacts/mcp-workspace"
RESOLVED_WORKSPACE="$(realpath -m "$WORKSPACE")"
case "$RESOLVED_WORKSPACE" in
    "$REPO_ROOT"/*) ;;
    *)
        echo "error: isolated MCP workspace must stay inside the checkout" >&2
        exit 1
        ;;
esac
if [ -e "$WORKSPACE" ]; then
    echo "error: isolated MCP workspace already exists" >&2
    exit 1
fi
mkdir -p "$RESOLVED_WORKSPACE"
git -C "$REPO_ROOT" archive HEAD | tar -x -C "$WORKSPACE"

DEPLOYD_MCP_WORKSPACE="$WORKSPACE" \
DEPLOYD_MCP_RUST_ANALYZER_WRAPPER="$REPO_ROOT/scripts/ci-rust-analyzer-mcp.sh" \
DEPLOYD_MCP_FOSSIL_WRAPPER="$REPO_ROOT/scripts/ci-fossil-mcp.sh" \
    python3 "$REPO_ROOT/scripts/test_mcp_smoke.py"

"$REPO_ROOT/scripts/ci-ownership.sh" "$WORKSPACE"
"$REPO_ROOT/scripts/ci-ownership-smoke.sh"
