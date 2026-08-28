#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
source "$SCRIPT_DIR/mcp-versions.sh"

INSTALL_ROOT="$HOME/.local/lib/deployd-mcp"
SOURCE_ROOT="/build/mcp/sources"
BINARY="$INSTALL_ROOT/fossil-mcp-$FOSSIL_MCP_VERSION"

if [ "$(id -u)" = "0" ]; then
    echo "error: install-fossil.sh must not run as root" >&2
    exit 1
fi
if [ "${HOME:-}" != "/home/ubuntu" ]; then
    echo "error: install-fossil.sh requires HOME=/home/ubuntu" >&2
    exit 1
fi

if [ -x "$BINARY" ] \
    && "$BINARY" --version 2>/dev/null | grep -qx "fossil-mcp $FOSSIL_MCP_VERSION"
then
    exit 0
fi

mkdir -p "$INSTALL_ROOT" "$SOURCE_ROOT"
ARCHIVE="$SOURCE_ROOT/fossil-mcp-linux-x86_64-musl-$FOSSIL_MCP_VERSION.tar.gz"
DOWNLOAD="$ARCHIVE.download"
EXTRACT_ROOT="$SOURCE_ROOT/fossil-mcp-$FOSSIL_MCP_VERSION-extract"

curl --fail --location --proto '=https' --tlsv1.2 \
    --user-agent 'deployd-mcp-provisioning/1.0' \
    --output "$DOWNLOAD" \
    "https://github.com/yfedoseev/fossil-mcp/releases/download/v$FOSSIL_MCP_VERSION/fossil-mcp-linux-x86_64-musl-$FOSSIL_MCP_VERSION.tar.gz"
printf '%s  %s\n' "$FOSSIL_MCP_SHA256" "$DOWNLOAD" \
    | sha256sum --check --status
mv "$DOWNLOAD" "$ARCHIVE"

case "$EXTRACT_ROOT" in
    /build/mcp/sources/fossil-mcp-*-extract) rm -rf "$EXTRACT_ROOT" ;;
    *)
        echo "error: unsafe Fossil extraction path" >&2
        exit 1
        ;;
esac
mkdir -p "$EXTRACT_ROOT"
tar -xzf "$ARCHIVE" -C "$EXTRACT_ROOT"
install -m 0755 "$EXTRACT_ROOT/fossil-mcp" "$BINARY"
"$BINARY" --version | grep -qx "fossil-mcp $FOSSIL_MCP_VERSION"
