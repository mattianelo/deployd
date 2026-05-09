#!/usr/bin/env bash
# Run cargo commands inside the deployd build environment (mirrors build-appimage.sh flow).
# Usage: ./check.sh [check|clippy|test|...] [extra cargo flags]
#
# Requires the deployd-appimage-build LXD container. Run build-appimage.sh first
# if it does not exist. Docker is intentionally not supported by this wrapper.
set -eu
# Ignore SIGPIPE so piping output through `tail` doesn't kill the script
# prematurely when the reader closes early (e.g. `./check.sh clippy 2>&1 | tail -40`).
trap '' PIPE

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
LXD_CONTAINER="deployd-appimage-build"
CMD="${1:-check}"
FEATURES="loot,libarchive-fallback"

# ── LXD path (host) ──────────────────────────────────────────────────────────
if [ "${DEPLOYD_NO_LXD:-0}" != "1" ] \
    && command -v lxc &>/dev/null \
    && lxc info &>/dev/null 2>&1
then
    lxc start "$LXD_CONTAINER" 2>/dev/null || true
    lxc exec --force-noninteractive "$LXD_CONTAINER" -- \
        env DEPLOYD_NO_LXD=1 DEPLOYD_NO_DOCKER=1 \
            DEPLOYD_EXPERIMENTAL="${DEPLOYD_EXPERIMENTAL:-0}" \
            APPIMAGE_EXTRACT_AND_RUN=1 \
            PATH="/root/.cargo/bin:/opt/appimage-tools:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
            CARGO_TARGET_DIR=/build/target \
        bash /workspace/check.sh "$@"
    exit $?
fi

# ── Direct path (inside LXD container) ───────────────────────────────────────
if [ "${DEPLOYD_NO_DOCKER:-0}" = "1" ]; then
    cd "$REPO_ROOT"
    if [ "$CMD" = "fmt" ]; then
        exec cargo fmt "${@:2}"
    fi
    if [ "$CMD" = "nextest" ]; then
        NEXTEST_SUBCMD="${2:-run}"
        exec cargo nextest "$NEXTEST_SUBCMD" --features "$FEATURES" "${@:3}"
    fi
    exec cargo "$CMD" --features "$FEATURES" "${@:2}"
fi

echo "error: LXD is required for ./check.sh; Docker fallback is intentionally disabled." >&2
echo "hint: ensure 'lxc info' works and the '$LXD_CONTAINER' container exists." >&2
exit 1
