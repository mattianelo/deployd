#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
source "$SCRIPT_DIR/mcp-versions.sh"

INSTALL_ROOT="$HOME/.local/lib/deployd-mcp"
SOURCE_ROOT="/build/mcp/sources"
TARGET_ROOT="/build/mcp/target"
BINARY="$INSTALL_ROOT/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION"
MARKER="${BINARY}.lock-sha256"

if [ "$(id -u)" = "0" ]; then
    echo "error: install-rust-analyzer-mcp.sh must not run as root" >&2
    exit 1
fi
if [ "${HOME:-}" != "/home/ubuntu" ]; then
    echo "error: install-rust-analyzer-mcp.sh requires HOME=/home/ubuntu" >&2
    exit 1
fi

if [ -x "$BINARY" ] \
    && [ "$(cat "$MARKER" 2>/dev/null || true)" = "$RUST_ANALYZER_MCP_LOCK_SHA256" ] \
    && "$BINARY" --version 2>/dev/null \
        | grep -qx "rust-analyzer-mcp $RUST_ANALYZER_MCP_VERSION"
then
    exit 0
fi

mkdir -p "$INSTALL_ROOT" "$SOURCE_ROOT" "$TARGET_ROOT"
CRATE="$SOURCE_ROOT/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION.crate"
DOWNLOAD="$CRATE.download"
CRATE_SOURCE="$SOURCE_ROOT/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION"

curl --fail --location --proto '=https' --tlsv1.2 \
    --user-agent 'deployd-mcp-provisioning/1.0' \
    --output "$DOWNLOAD" \
    "https://crates.io/api/v1/crates/rust-analyzer-mcp/$RUST_ANALYZER_MCP_VERSION/download"
printf '%s  %s\n' "$RUST_ANALYZER_MCP_SHA256" "$DOWNLOAD" \
    | sha256sum --check --status
mv "$DOWNLOAD" "$CRATE"

case "$CRATE_SOURCE" in
    /build/mcp/sources/rust-analyzer-mcp-*) rm -rf "$CRATE_SOURCE" ;;
    *)
        echo "error: unsafe rust-analyzer MCP source path" >&2
        exit 1
        ;;
esac
mkdir -p "$CRATE_SOURCE"
tar -xzf "$CRATE" --strip-components=1 -C "$CRATE_SOURCE"
cargo update \
    --manifest-path "$CRATE_SOURCE/Cargo.toml" \
    --package bytes \
    --precise "$RUST_ANALYZER_MCP_BYTES_VERSION"
printf '%s  %s\n' "$RUST_ANALYZER_MCP_LOCK_SHA256" "$CRATE_SOURCE/Cargo.lock" \
    | sha256sum --check --status
cargo build \
    --locked \
    --release \
    --manifest-path "$CRATE_SOURCE/Cargo.toml" \
    --target-dir "$TARGET_ROOT/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION"
install -m 0755 \
    "$TARGET_ROOT/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION/release/rust-analyzer-mcp" \
    "$BINARY"
printf '%s\n' "$RUST_ANALYZER_MCP_LOCK_SHA256" >"$MARKER"
"$BINARY" --version | grep -qx "rust-analyzer-mcp $RUST_ANALYZER_MCP_VERSION"
