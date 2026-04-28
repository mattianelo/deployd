#!/bin/bash
# Provisions the deployd-appimage-build LXD container from scratch.
# Called once on first container creation by build-appimage.sh; subsequent
# builds reuse the already-provisioned container without re-running this.
#
# Mirrors the Dockerfile exactly so local (LXD) and CI (Docker) environments
# stay in sync. When the Dockerfile changes, update this script to match.

set -euo pipefail

export DEBIAN_FRONTEND=noninteractive

apt-get update -qq
apt-get install -y --no-install-recommends \
    build-essential curl ca-certificates git pkg-config \
    libgtk-4-dev libadwaita-1-dev \
    libglib2.0-bin libglib2.0-dev \
    libsqlite3-dev libssl-dev libarchive-dev \
    libunrar-dev \
    librsvg2-common \
    libgdk-pixbuf2.0-bin \
    webp-pixbuf-loader \
    patchelf file desktop-file-utils librsvg2-bin fuse \
    wget
rm -rf /var/lib/apt/lists/*

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain stable --profile minimal
/root/.cargo/bin/rustup component add rust-std clippy
/root/.cargo/bin/cargo install cargo-audit --locked

UMU_VERSION=1.4.0
wget -q \
    "https://github.com/Open-Wine-Components/umu-launcher/releases/download/${UMU_VERSION}/umu-launcher-${UMU_VERSION}-zipapp.tar" \
    -O /tmp/umu-zipapp.tar
tar -xf /tmp/umu-zipapp.tar -C /tmp
mv /tmp/umu/umu-run /opt/umu-run
chmod +x /opt/umu-run
rm -rf /tmp/umu-zipapp.tar /tmp/umu

mkdir -p /opt/appimage-tools /build
wget -q "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage" \
     -O /opt/appimage-tools/linuxdeploy
wget -q "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh" \
     -O /opt/appimage-tools/linuxdeploy-plugin-gtk.sh
wget -q "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" \
     -O /opt/appimage-tools/appimagetool
chmod +x /opt/appimage-tools/linuxdeploy \
         /opt/appimage-tools/appimagetool \
         /opt/appimage-tools/linuxdeploy-plugin-gtk.sh
