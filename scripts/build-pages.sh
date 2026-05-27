#!/bin/sh
# Build the static GitLab Pages artifact without host-side dependencies.
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="${DEPLOYD_PAGES_OUT:-$REPO_ROOT/out}"

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR/assets/screenshots" "$OUT_DIR/assets/icons"

cp "$REPO_ROOT/pages/index.html" "$OUT_DIR/index.html"
cp "$REPO_ROOT"/data/screenshots/*.png "$OUT_DIR/assets/screenshots/"
cp "$REPO_ROOT/data/icons/hicolor/scalable/apps/deployd.svg" "$OUT_DIR/assets/icons/"

echo "Deployd Pages artifact written to $OUT_DIR"
