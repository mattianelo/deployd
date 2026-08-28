#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"

if [ "$(id -u)" = "0" ]; then
    echo "error: provision-mcp.sh must not run as root" >&2
    exit 1
fi
if [ "${HOME:-}" != "/home/ubuntu" ]; then
    echo "error: provision-mcp.sh requires HOME=/home/ubuntu" >&2
    exit 1
fi

bash "$SCRIPT_DIR/install-rust-analyzer-mcp.sh"
bash "$SCRIPT_DIR/install-fossil.sh"

cargo fetch --locked --manifest-path /workspace/Cargo.toml
RUST_SYSROOT="$(rustc --print sysroot)"
RUSTC_BOOTSTRAP=1 cargo fetch \
    --locked \
    --manifest-path "$RUST_SYSROOT/lib/rustlib/src/rust/library/Cargo.toml"
