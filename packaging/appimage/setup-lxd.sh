#!/bin/bash
# Provisions the deployd-appimage-build LXD container from scratch.
# Called once on first container creation by build-appimage.sh; subsequent
# builds reuse the already-provisioned container without re-running this.
#
# System provisioning runs as container root. Rust tooling is installed later
# by setup-user.sh under the non-root build identity.

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

UMU_VERSION=1.4.0
wget -q \
    "https://github.com/Open-Wine-Components/umu-launcher/releases/download/${UMU_VERSION}/umu-launcher-${UMU_VERSION}-zipapp.tar" \
    -O /tmp/umu-zipapp.tar
tar -xf /tmp/umu-zipapp.tar -C /tmp
mv /tmp/umu/umu-run /opt/umu-run
chmod +x /opt/umu-run
rm -rf /tmp/umu-zipapp.tar /tmp/umu

install -d -o ubuntu -g ubuntu /build
chown -R ubuntu:ubuntu /build
mkdir -p /opt/appimage-tools
wget -q "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage" \
     -O /opt/appimage-tools/linuxdeploy
wget -q "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh" \
     -O /opt/appimage-tools/linuxdeploy-plugin-gtk.sh
wget -q "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" \
     -O /opt/appimage-tools/appimagetool
chmod +x /opt/appimage-tools/linuxdeploy \
         /opt/appimage-tools/appimagetool \
         /opt/appimage-tools/linuxdeploy-plugin-gtk.sh
