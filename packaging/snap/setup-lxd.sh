#!/bin/bash
# Provisions the isolated Ubuntu 24.04 container used for local Snap builds.

set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
RUST_VERSION="$(sed -n 's/^channel = "\([^"]*\)"/\1/p' /workspace/rust-toolchain.toml | head -1)"

if [ -z "$RUST_VERSION" ]; then
    echo "ERROR: failed to read the pinned Rust version." >&2
    exit 1
fi

apt-get update -qq
apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl git

snap wait system seed.loaded
snap install snapcraft --classic --channel=9.x/stable

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain "$RUST_VERSION" --profile minimal

mkdir -p /build
