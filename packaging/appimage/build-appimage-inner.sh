#!/bin/bash
# Runs INSIDE the deployd-build-env Docker container.
# On CI (Docker-in-Docker) this is called directly via DEPLOYD_NO_DOCKER=1.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# Intermediate artifacts go to a container-internal path so they are never
# written to the bind-mounted workspace.  Only the final AppImage reaches
# /workspace, avoiding root-owned files that would block Snap builds.
mkdir -p /build
export CARGO_TARGET_DIR=/build/target
APPDIR=/build/AppDir
APP_ID="deployd"
DESKTOP_ID="io.mattianelo.deployd"
OUTPUT="$REPO_ROOT/Deployd-x86_64.AppImage"

DEBUG=0
for arg in "$@"; do
    case "$arg" in
        --debug) DEBUG=1 ;;
    esac
done

cd "$REPO_ROOT"

VERSION=${VERSION:-$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')}

echo "==> Building Deployd $VERSION AppImage (inner)"

# 1. Compile
FEATURES="loot,libarchive-fallback"

if [ "$DEBUG" = "1" ]; then
    echo "==> Compiling (debug, features: $FEATURES)"
    cargo build --features "$FEATURES"
    BINARY="$CARGO_TARGET_DIR/debug/$APP_ID"
else
    echo "==> Compiling (release, features: $FEATURES)"
    cargo build --release --features "$FEATURES"
    BINARY="$CARGO_TARGET_DIR/release/$APP_ID"
fi

# 2. Assemble AppDir
echo "==> Assembling AppDir with linuxdeploy"
rm -rf "$APPDIR"
linuxdeploy \
    --appdir "$APPDIR" \
    --executable "$BINARY" \
    --desktop-file "data/$DESKTOP_ID.desktop" \
    --icon-file "data/icons/hicolor/scalable/apps/$APP_ID.svg" \
    --plugin gtk

# 2a. Strip gvfs GIO modules bundled by linuxdeploy-plugin-gtk.
# They are compiled against Ubuntu 24.04 and fail with "undefined symbol"
# on older or differently-patched systems.  Deployd does not use remote
# volume monitoring, so removing them is safe.
find "$APPDIR/usr/lib/gio/modules" -name "libgvfs*.so" -delete 2>/dev/null || true

# 2b. Regenerate GDK pixbuf loaders.cache
# linuxdeploy-plugin-gtk writes a cache pointing at the system loader path.
# We regenerate it against the AppDir's own bundled loaders, then stamp
# @@APPDIR@@ in place of the build-time absolute path so AppRun can expand
# it to the real squashfs mount point at runtime.
echo "==> Regenerating GDK pixbuf loaders.cache"
LOADERS_DIR="$APPDIR/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders"
CACHE_FILE="$APPDIR/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"

if [ -d "$LOADERS_DIR" ]; then
    GDK_QUERY_LOADERS=$(find /usr/lib -name "gdk-pixbuf-query-loaders" -type f 2>/dev/null | head -1)
    if [ -z "$GDK_QUERY_LOADERS" ]; then
        echo "    WARNING: gdk-pixbuf-query-loaders not found; install libgdk-pixbuf2.0-bin in Dockerfile"
    else
        GDK_PIXBUF_MODULEDIR="$LOADERS_DIR" "$GDK_QUERY_LOADERS" > "$CACHE_FILE"
        if grep -q "svg" "$CACHE_FILE"; then
            echo "    SVG loader: OK"
        else
            echo "    WARNING: SVG loader missing — install librsvg2-common in Dockerfile"
        fi
        sed -i "s|$APPDIR|@@APPDIR@@|g" "$CACHE_FILE"
    fi
else
    echo "    WARNING: loaders directory not found, skipping"
fi

# 2c. Bundle umu-run (UMU Launcher)
# umu-run is a self-contained binary built with PyInstaller; it does not need
# linuxdeploy shared-library resolution. On first tool launch deployd points
# UMU at Deployd's own data directory so Proton GE is isolated from other apps.
echo "==> Bundling umu-run into AppDir"
if [ -f /opt/umu-run ]; then
    cp /opt/umu-run "$APPDIR/usr/bin/umu-run"
    chmod +x "$APPDIR/usr/bin/umu-run"
    echo "  umu-run bundled successfully"
else
    echo "  WARNING: /opt/umu-run not found; external tools will not be available"
fi

# 3. Install custom AppRun (handles NXM protocol registration)
echo "==> Installing custom AppRun"
cp "packaging/appimage/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"

# 4. AppStream metainfo
mkdir -p "$APPDIR/usr/share/metainfo"
cp "data/$DESKTOP_ID.metainfo.xml" "$APPDIR/usr/share/metainfo/"

# 5. Package (zstd compression)
echo "==> Packaging -> $OUTPUT"
rm -f "$OUTPUT"
VERSION="$VERSION" ARCH=x86_64 appimagetool \
    --comp zstd \
    "$APPDIR" \
    "$OUTPUT"
chmod a+rx "$OUTPUT"

echo ""
echo "Done: $OUTPUT"
echo "  Size: $(du -sh "$OUTPUT" | cut -f1)"
