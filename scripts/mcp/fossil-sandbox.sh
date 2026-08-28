#!/usr/bin/env bash
set -euo pipefail

source /workspace/packaging/appimage/mcp-versions.sh

STATE_ROOT="/build/mcp/fossil"
mkdir -p "$STATE_ROOT"
UPPER_DIR="$STATE_ROOT/upper"
WORK_DIR="$(mktemp -d "$STATE_ROOT/work.XXXXXX")"
CONFIG_SOURCE="/workspace/fossil.toml"
BINARY="/home/ubuntu/.local/lib/deployd-mcp/fossil-mcp-$FOSSIL_MCP_VERSION"

cleanup() {
    if [ "$(findmnt --noheadings --output FSTYPE --target /workspace)" = "overlay" ]; then
        umount /workspace || umount --lazy /workspace
    fi
    case "$WORK_DIR" in
        /build/mcp/fossil/work.*) rm -rf "$WORK_DIR" ;;
    esac
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir -p "$UPPER_DIR"
exec 9>"$STATE_ROOT/server.lock"
if ! flock --nonblock 9; then
    echo "error: another Fossil MCP server is already using the analysis cache" >&2
    exit 1
fi

if [ -f "$CONFIG_SOURCE" ]; then
    cp "$CONFIG_SOURCE" "$UPPER_DIR/fossil.toml"
else
    rm -f "$UPPER_DIR/fossil.toml"
fi

mount --make-rprivate /
mount -t overlay overlay \
    -o "lowerdir=/workspace,upperdir=$UPPER_DIR,workdir=$WORK_DIR" \
    /workspace

"$BINARY" mcp
