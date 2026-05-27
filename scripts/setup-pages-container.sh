#!/usr/bin/env bash
# Attach Deployd to a dedicated LXD container and expose a static Pages preview.
set -euo pipefail

DEPLOYD_PAGES_CONTAINER="${DEPLOYD_PAGES_CONTAINER:-deployd-pages}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEPLOYD_SRC="${DEPLOYD_SRC:-$(cd "$SCRIPT_DIR/.." && pwd)}"
DEPLOYD_PAGES_HOST_PORT="${DEPLOYD_PAGES_HOST_PORT:-3003}"
DEPLOYD_PAGES_CONTAINER_PORT="${DEPLOYD_PAGES_CONTAINER_PORT:-3003}"
WORKSPACE=/workspace
DEPLOYD_TARGET="$WORKSPACE/deployd"

ensure_container() {
  if ! lxc info "$DEPLOYD_PAGES_CONTAINER" >/dev/null 2>&1; then
    echo "==> Launching Ubuntu 24.04 container: $DEPLOYD_PAGES_CONTAINER"
    lxc launch ubuntu:24.04 "$DEPLOYD_PAGES_CONTAINER"
    sleep 8
  fi

  if lxc info "$DEPLOYD_PAGES_CONTAINER" | grep -q "Status: STOPPED"; then
    echo "==> Starting container: $DEPLOYD_PAGES_CONTAINER"
    lxc start "$DEPLOYD_PAGES_CONTAINER"
    sleep 3
  fi

  echo "==> Applying container security settings"
  lxc config set "$DEPLOYD_PAGES_CONTAINER" \
    security.syscalls.intercept.mknod=true \
    security.syscalls.intercept.setxattr=true
}

refresh_device() {
  local device="$1"
  if lxc config device show "$DEPLOYD_PAGES_CONTAINER" | grep -q "^${device}:"; then
    lxc config device remove "$DEPLOYD_PAGES_CONTAINER" "$device"
  fi
}

ensure_mount() {
  local device=deployd-src
  local current_source current_path

  current_source="$(lxc config device get "$DEPLOYD_PAGES_CONTAINER" "$device" source 2>/dev/null || true)"
  current_path="$(lxc config device get "$DEPLOYD_PAGES_CONTAINER" "$device" path 2>/dev/null || true)"

  if [ "$current_source" = "$DEPLOYD_SRC" ] && [ "$current_path" = "$DEPLOYD_TARGET" ]; then
    echo "==> Deployd already mounted at $DEPLOYD_TARGET"
    return
  fi

  echo "==> Mounting Deployd at $DEPLOYD_TARGET"
  refresh_device "$device"
  lxc config device add "$DEPLOYD_PAGES_CONTAINER" "$device" disk \
    source="$DEPLOYD_SRC" \
    path="$DEPLOYD_TARGET" \
    shift=true
}

ensure_proxy() {
  local device=deployd-pages-web
  local listen="tcp:127.0.0.1:${DEPLOYD_PAGES_HOST_PORT}"
  local connect="tcp:127.0.0.1:${DEPLOYD_PAGES_CONTAINER_PORT}"
  local current_listen current_connect

  current_listen="$(lxc config device get "$DEPLOYD_PAGES_CONTAINER" "$device" listen 2>/dev/null || true)"
  current_connect="$(lxc config device get "$DEPLOYD_PAGES_CONTAINER" "$device" connect 2>/dev/null || true)"

  if [ "$current_listen" = "$listen" ] && [ "$current_connect" = "$connect" ]; then
    echo "==> Deployd Pages proxy already points localhost:$DEPLOYD_PAGES_HOST_PORT -> container:$DEPLOYD_PAGES_CONTAINER_PORT"
    return
  fi

  echo "==> Adding Deployd Pages proxy localhost:$DEPLOYD_PAGES_HOST_PORT -> container:$DEPLOYD_PAGES_CONTAINER_PORT"
  refresh_device "$device"
  lxc config device add "$DEPLOYD_PAGES_CONTAINER" "$device" proxy \
    listen="$listen" \
    connect="$connect"
}

ensure_python() {
  if lxc exec "$DEPLOYD_PAGES_CONTAINER" -- python3 --version >/dev/null 2>&1; then
    echo "==> Python 3 is already installed"
    return
  fi

  echo "==> Installing Python 3 in $DEPLOYD_PAGES_CONTAINER"
  lxc exec "$DEPLOYD_PAGES_CONTAINER" -- bash -c "apt-get update && apt-get install -y python3"
}

ensure_container
ensure_mount
ensure_proxy
ensure_python

echo ""
echo "Deployd Pages preview container ready: $DEPLOYD_PAGES_CONTAINER"
echo "Build the artifact:"
echo "  lxc exec $DEPLOYD_PAGES_CONTAINER --cwd $DEPLOYD_TARGET --user 1000 --group 1000 --env HOME=/home/ubuntu -- bash scripts/build-pages.sh"
echo "Serve it:"
echo "  lxc exec $DEPLOYD_PAGES_CONTAINER --cwd $DEPLOYD_TARGET/out --user 1000 --group 1000 --env HOME=/home/ubuntu -- python3 -m http.server $DEPLOYD_PAGES_CONTAINER_PORT --bind 0.0.0.0"
echo "Then open http://localhost:$DEPLOYD_PAGES_HOST_PORT"
