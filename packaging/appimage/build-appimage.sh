#!/bin/bash
# Build script for Deployd AppImage
# Target: Ubuntu 24.04 LTS (Noble) or newer, x86_64
#
# Usage (from repo root):
#   bash packaging/appimage/build-appimage.sh
#
# Build requirements:
#   sudo apt install build-essential libgtk-4-dev libadwaita-1-dev \
#     libsqlite3-dev libssl-dev libarchive-dev libglib2.0-bin \
#     libunrar-dev patchelf
#   (plus Rust toolchain: https://rustup.rs)
#
# Runtime requirements on the target machine (Ubuntu 24.04+):
#   sudo apt install libgtk-4-1 libadwaita-1-0 libunrar5

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TOOLS_DIR="$REPO_ROOT/.appimage-tools"
APPDIR="$REPO_ROOT/AppDir"
APP_ID="deployd"

VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*= *"\(.*\)"/\1/')
OUTPUT="$REPO_ROOT/Deployd-x86_64.AppImage"

echo "==> Building Deployd $VERSION AppImage"

# ---------------------------------------------------------------------------
# Download appimagetool
# ---------------------------------------------------------------------------
mkdir -p "$TOOLS_DIR"

APPIMAGETOOL="$TOOLS_DIR/appimagetool-x86_64.AppImage"
if [ ! -f "$APPIMAGETOOL" ]; then
    echo "    Downloading appimagetool..."
    wget -q --show-progress \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" \
        -O "$APPIMAGETOOL"
    chmod +x "$APPIMAGETOOL"
fi

# appimagetool is itself an AppImage; avoid FUSE by extracting it to a temp dir.
export APPIMAGE_EXTRACT_AND_RUN=1

# ---------------------------------------------------------------------------
# Build the Rust binary
# ---------------------------------------------------------------------------
echo "==> Compiling (release, features: loot libarchive-fallback)"
cd "$REPO_ROOT"
cargo build --release --features loot,libarchive-fallback

# ---------------------------------------------------------------------------
# Assemble AppDir
# ---------------------------------------------------------------------------
# We build the AppDir ourselves — no linuxdeploy — so that nothing touches
# our custom AppRun or overwrites the deployd binary.
echo "==> Assembling AppDir"
rm -rf "$APPDIR"
mkdir -p \
    "$APPDIR/usr/bin" \
    "$APPDIR/usr/lib" \
    "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/scalable/apps" \
    "$APPDIR/usr/share/metainfo" \
    "$APPDIR/usr/share/glib-2.0/schemas"

# Binary
cp "target/release/deployd" "$APPDIR/usr/bin/"

# Data files
cp "data/$APP_ID.desktop"                               "$APPDIR/usr/share/applications/"
cp "data/icons/hicolor/scalable/apps/$APP_ID.svg"       "$APPDIR/usr/share/icons/hicolor/scalable/apps/"
cp "data/$APP_ID.metainfo.xml"                          "$APPDIR/usr/share/metainfo/"

# ---------------------------------------------------------------------------
# Bundle non-standard shared libraries
# ---------------------------------------------------------------------------
# GTK4, libadwaita, GLib, and other GNOME stack libraries are provided by
# Ubuntu 24.04+ and do NOT need bundling. We only bundle libraries that are
# not reliably pre-installed (libunrar, libarchive if not system-default, etc.).
#
# Strategy: copy every .so that ldd reports and that is NOT in the list of
# well-known Ubuntu system libraries, then fix the rpath on the binary.
echo "==> Bundling non-system libraries"

# Libraries provided by the target OS — do not bundle these.
SYSTEM_LIBS_PATTERN="libgtk|libgdk|libadwaita|libgio|libglib|libgobject|libgmodule|libgthread|libpango|libcairo|libatk|libharfbuzz|libfontconfig|libfreetype|libpixman|libpng|libX|libxcb|libwayland|libdrm|libxkb|libdbus|libsystemd|libc\\.so|libm\\.so|libdl\\.so|libpthread|librt\\.so|libresolv|libz\\.so|libstdc|libgcc|liblzma|libzstd|libssl|libcrypto|libsqlite|libuuid|libffi|libpcre|libexpat|libgraphene|libnss|libgdk_pixbuf|librsvg|libxml2|libepoxy|libGL|libEGL|libjpeg|libwebp|libtiff|libopenjp"

ldd "target/release/deployd" | awk '/=>/ { print $3 }' | while read -r lib; do
    [ -f "$lib" ] || continue
    basename=$(basename "$lib")
    if ! echo "$basename" | grep -qE "$SYSTEM_LIBS_PATTERN"; then
        echo "    Bundling $basename"
        cp "$lib" "$APPDIR/usr/lib/"
    fi
done

# Point the binary to the bundled libs directory.
patchelf --set-rpath '$ORIGIN/../lib' "$APPDIR/usr/bin/deployd"

# ---------------------------------------------------------------------------
# GSettings schemas
# ---------------------------------------------------------------------------
# GTK4 and libadwaita register GSettings schemas. The compiled binary database
# must be present so the app finds settings at runtime via GSETTINGS_SCHEMA_DIR.
echo "==> Compiling GSettings schemas"
SCHEMA_DIR="$APPDIR/usr/share/glib-2.0/schemas"
cp /usr/share/glib-2.0/schemas/*.xml "$SCHEMA_DIR/" 2>/dev/null || true
glib-compile-schemas "$SCHEMA_DIR"

# ---------------------------------------------------------------------------
# AppRun and AppDir root symlinks
# ---------------------------------------------------------------------------
echo "==> Installing AppRun"
cp "packaging/appimage/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"

# appimagetool requires the .desktop file and primary icon at the AppDir root.
ln -sf "usr/share/applications/$APP_ID.desktop"             "$APPDIR/$APP_ID.desktop"
ln -sf "usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"  "$APPDIR/$APP_ID.svg"

# ---------------------------------------------------------------------------
# Package
# ---------------------------------------------------------------------------
echo "==> Packaging -> $OUTPUT"
VERSION="$VERSION" ARCH=x86_64 "$APPIMAGETOOL" \
    --comp zstd \
    "$APPDIR" \
    "$OUTPUT"

echo ""
echo "Done: $OUTPUT"
echo ""
echo "Quick tests:"
echo "  $OUTPUT                          # first launch registers the app"
echo "  ls ~/.local/share/applications/$APP_ID.desktop"
echo "  xdg-open 'nxm://skyrimspecialedition/mods/2347/files/12345?key=abc&expires=999'"
