#!/usr/bin/env bash
# Fetch snap build dependencies that cannot be committed to git.
# Run this before 'snapcraft pack' (or 'snapcraft pack --use-lxd') for local snap builds.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

UMU_TAG=$(curl -fsSL https://api.github.com/repos/Open-Wine-Components/umu-launcher/releases/latest \
  | grep '"tag_name"' | head -1 | cut -d'"' -f4)

echo "Fetching UMU Launcher ${UMU_TAG}..."
mkdir -p "$REPO_ROOT/snap/local"
wget -q \
  "https://github.com/Open-Wine-Components/umu-launcher/releases/download/${UMU_TAG}/umu-launcher-${UMU_TAG}-zipapp.tar" \
  -O "$REPO_ROOT/snap/local/umu-launcher-zipapp.tar"

echo "Done. snap/local/umu-launcher-zipapp.tar is ready."
