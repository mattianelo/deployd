#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
source "$SCRIPT_DIR/mcp-versions.sh"

INSTALL_ROOT="$HOME/.local/lib/deployd-mcp"
SOURCE_ROOT="/build/mcp/sources"
TARGET_ROOT="/build/mcp/target"

if [ "$(id -u)" = "0" ]; then
    echo "error: provision-mcp.sh must not run as root" >&2
    exit 1
fi

if [ "${HOME:-}" != "/home/ubuntu" ]; then
    echo "error: provision-mcp.sh requires HOME=/home/ubuntu" >&2
    exit 1
fi

download_verified() {
    local url="$1"
    local sha256="$2"
    local destination="$3"
    local temporary="${destination}.download"

    curl --fail --location --proto '=https' --tlsv1.2 \
        --user-agent 'deployd-mcp-provisioning/1.0' \
        --output "$temporary" "$url"
    printf '%s  %s\n' "$sha256" "$temporary" | sha256sum --check --status
    mv "$temporary" "$destination"
}

mkdir -p "$INSTALL_ROOT" "$SOURCE_ROOT" "$TARGET_ROOT"

RUST_ANALYZER_MCP_BIN="$INSTALL_ROOT/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION"
RUST_ANALYZER_MCP_MARKER="${RUST_ANALYZER_MCP_BIN}.lock-sha256"
if [ ! -x "$RUST_ANALYZER_MCP_BIN" ] \
    || [ "$(cat "$RUST_ANALYZER_MCP_MARKER" 2>/dev/null || true)" != "$RUST_ANALYZER_MCP_LOCK_SHA256" ] \
    || ! "$RUST_ANALYZER_MCP_BIN" --version 2>/dev/null \
        | grep -qx "rust-analyzer-mcp $RUST_ANALYZER_MCP_VERSION"
then
    RUST_ANALYZER_MCP_CRATE="$SOURCE_ROOT/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION.crate"
    RUST_ANALYZER_MCP_SOURCE="$SOURCE_ROOT/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION"

    download_verified \
        "https://crates.io/api/v1/crates/rust-analyzer-mcp/$RUST_ANALYZER_MCP_VERSION/download" \
        "$RUST_ANALYZER_MCP_SHA256" \
        "$RUST_ANALYZER_MCP_CRATE"
    rm -rf "$RUST_ANALYZER_MCP_SOURCE"
    mkdir -p "$RUST_ANALYZER_MCP_SOURCE"
    tar -xzf "$RUST_ANALYZER_MCP_CRATE" \
        --strip-components=1 \
        -C "$RUST_ANALYZER_MCP_SOURCE"
    cargo update \
        --manifest-path "$RUST_ANALYZER_MCP_SOURCE/Cargo.toml" \
        --package bytes \
        --precise "$RUST_ANALYZER_MCP_BYTES_VERSION"
    printf '%s  %s\n' \
        "$RUST_ANALYZER_MCP_LOCK_SHA256" \
        "$RUST_ANALYZER_MCP_SOURCE/Cargo.lock" \
        | sha256sum --check --status
    cargo build \
        --locked \
        --release \
        --manifest-path "$RUST_ANALYZER_MCP_SOURCE/Cargo.toml" \
        --target-dir "$TARGET_ROOT/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION"
    install -m 0755 \
        "$TARGET_ROOT/rust-analyzer-mcp-$RUST_ANALYZER_MCP_VERSION/release/rust-analyzer-mcp" \
        "$RUST_ANALYZER_MCP_BIN"
    printf '%s\n' "$RUST_ANALYZER_MCP_LOCK_SHA256" >"$RUST_ANALYZER_MCP_MARKER"
fi

FOSSIL_MCP_BIN="$INSTALL_ROOT/fossil-mcp-$FOSSIL_MCP_VERSION"
if [ ! -x "$FOSSIL_MCP_BIN" ] \
    || ! "$FOSSIL_MCP_BIN" --version 2>/dev/null \
        | grep -qx "fossil-mcp $FOSSIL_MCP_VERSION"
then
    FOSSIL_MCP_ARCHIVE="$SOURCE_ROOT/fossil-mcp-linux-x86_64-musl-$FOSSIL_MCP_VERSION.tar.gz"
    FOSSIL_MCP_EXTRACT="$SOURCE_ROOT/fossil-mcp-$FOSSIL_MCP_VERSION-extract"

    download_verified \
        "https://github.com/yfedoseev/fossil-mcp/releases/download/v$FOSSIL_MCP_VERSION/fossil-mcp-linux-x86_64-musl-$FOSSIL_MCP_VERSION.tar.gz" \
        "$FOSSIL_MCP_SHA256" \
        "$FOSSIL_MCP_ARCHIVE"
    rm -rf "$FOSSIL_MCP_EXTRACT"
    mkdir -p "$FOSSIL_MCP_EXTRACT"
    tar -xzf "$FOSSIL_MCP_ARCHIVE" -C "$FOSSIL_MCP_EXTRACT"
    install -m 0755 "$FOSSIL_MCP_EXTRACT/fossil-mcp" "$FOSSIL_MCP_BIN"
fi

cargo fetch --locked --manifest-path /workspace/Cargo.toml
RUST_SYSROOT="$(rustc --print sysroot)"
RUSTC_BOOTSTRAP=1 cargo fetch \
    --locked \
    --manifest-path "$RUST_SYSROOT/lib/rustlib/src/rust/library/Cargo.toml"
