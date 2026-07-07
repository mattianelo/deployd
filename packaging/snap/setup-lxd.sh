#!/bin/bash
# Provisions the isolated Ubuntu 24.04 container used for local Snap builds.

set -euo pipefail

export DEBIAN_FRONTEND=noninteractive

apt-get update -qq
apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl git

snap wait system seed.loaded
snap install snapcraft --classic --channel=9.x/stable

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain 1.96.1 --profile minimal
