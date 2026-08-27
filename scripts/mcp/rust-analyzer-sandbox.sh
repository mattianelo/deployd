#!/usr/bin/env bash
set -euo pipefail

source /workspace/packaging/appimage/mcp-versions.sh

STATE_ROOT="/build/mcp/rust-analyzer"
mkdir -p "$STATE_ROOT"
SESSION_ROOT="$(mktemp -d "$STATE_ROOT/session.XXXXXX")"
UPPER_DIR="$SESSION_ROOT/upper"
WORK_DIR="$SESSION_ROOT/work"
BINARY="/home/ubuntu/.local/lib/deployd-mcp/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION"

cleanup() {
    if [ "$(findmnt --noheadings --output FSTYPE --target /workspace)" = "overlay" ]; then
        umount /workspace || umount --lazy /workspace
    fi
    case "$SESSION_ROOT" in
        /build/mcp/rust-analyzer/session.*) rm -rf "$SESSION_ROOT" ;;
    esac
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir -p "$UPPER_DIR" "$WORK_DIR" "$CARGO_TARGET_DIR"
mount --make-rprivate /
mount -t overlay overlay \
    -o "lowerdir=/workspace,upperdir=$UPPER_DIR,workdir=$WORK_DIR" \
    /workspace

"$BINARY" \
    --features loot,libarchive-fallback \
    --config cargo.targetDir=/build/mcp/rust-analyzer/target \
    -- /workspace
