#!/usr/bin/env bash
# Provision Rust tools as the non-root build user after root installs system packages.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
RUST_VERSION="$(sed -n 's/^channel = "\([^"]*\)"/\1/p' "$REPO_ROOT/rust-toolchain.toml" | head -1)"

if [ -z "$RUST_VERSION" ]; then
    echo "error: failed to read the pinned Rust version" >&2
    exit 1
fi

if [ "$(id -u)" = "0" ]; then
    echo "error: setup-user.sh must not run as root" >&2
    exit 1
fi

if [ "${HOME:-}" != "/home/ubuntu" ]; then
    echo "error: setup-user.sh requires HOME=/home/ubuntu" >&2
    exit 1
fi

export PATH="$HOME/.cargo/bin:$PATH"

if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain "$RUST_VERSION" --profile minimal
fi

rustup toolchain install "$RUST_VERSION" --profile minimal
rustup default "$RUST_VERSION"
rustup component add --toolchain "$RUST_VERSION" rustfmt clippy rust-analyzer

if ! command -v cargo-audit >/dev/null 2>&1; then
    cargo install cargo-audit --version 0.22.2 --locked
fi

if ! command -v cargo-nextest >/dev/null 2>&1; then
    cargo install cargo-nextest --version 0.9.143 --locked
fi
