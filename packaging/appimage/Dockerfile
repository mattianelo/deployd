FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential curl ca-certificates git pkg-config python3 \
    libgtk-4-dev libadwaita-1-dev \
    libglib2.0-bin libglib2.0-dev \
    libsqlite3-dev libssl-dev libarchive-dev \
    libunrar-dev \
    librsvg2-common \
    libgdk-pixbuf2.0-bin \
    webp-pixbuf-loader \
    patchelf file desktop-file-utils librsvg2-bin fuse \
    wget \
    && rm -rf /var/lib/apt/lists/*

# UMU Launcher — Python zipapp bundling all dependencies; needs python3 at runtime.
# Pinned to a specific release for reproducible builds.
ARG UMU_VERSION=1.4.0
RUN wget -q \
        "https://github.com/Open-Wine-Components/umu-launcher/releases/download/${UMU_VERSION}/umu-launcher-${UMU_VERSION}-zipapp.tar" \
        -O /tmp/umu-zipapp.tar \
    && tar -xf /tmp/umu-zipapp.tar -C /tmp \
    && mv /tmp/umu/umu-run /opt/umu-run \
    && chmod +x /opt/umu-run \
    && rm -rf /tmp/umu-zipapp.tar /tmp/umu

RUN install -d -o ubuntu -g ubuntu /build \
    && mkdir -p /opt/appimage-tools \
    && wget -q "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage" \
         -O /opt/appimage-tools/linuxdeploy \
    && wget -q "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh" \
         -O /opt/appimage-tools/linuxdeploy-plugin-gtk.sh \
    && wget -q "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" \
         -O /opt/appimage-tools/appimagetool \
    && chmod +x /opt/appimage-tools/linuxdeploy \
                /opt/appimage-tools/appimagetool \
                /opt/appimage-tools/linuxdeploy-plugin-gtk.sh

ENV APPIMAGE_EXTRACT_AND_RUN=1
ENV PATH="/opt/appimage-tools:$PATH"

USER ubuntu
ENV HOME="/home/ubuntu"
ENV PATH="/home/ubuntu/.cargo/bin:/opt/appimage-tools:$PATH"

COPY --chown=ubuntu:ubuntu rust-toolchain.toml /tmp/deployd-rust-toolchain.toml

RUN RUST_VERSION="$(sed -n 's/^channel = "\([^"]*\)"/\1/p' /tmp/deployd-rust-toolchain.toml)" \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain "$RUST_VERSION" --profile minimal \
    && rustup component add --toolchain "$RUST_VERSION" rustfmt clippy rust-analyzer rust-src \
    && cargo install cargo-audit --version 0.22.2 --locked \
    && cargo install cargo-nextest --version 0.9.143 --locked

COPY --chown=ubuntu:ubuntu \
    packaging/appimage/mcp-versions.sh \
    packaging/appimage/install-rust-analyzer-mcp.sh \
    packaging/appimage/install-fossil.sh \
    /tmp/deployd-mcp/

RUN bash /tmp/deployd-mcp/install-rust-analyzer-mcp.sh \
    && bash /tmp/deployd-mcp/install-fossil.sh \
    && rm -rf /build/mcp/sources /build/mcp/target

WORKDIR /workspace
