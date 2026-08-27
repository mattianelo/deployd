#!/bin/sh
set -eu

REPO_ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)"
TEST_ROOT="$(mktemp -d)"
FAKE_BIN="$TEST_ROOT/bin"
CALLS="$TEST_ROOT/calls"

cleanup() {
    case "$TEST_ROOT" in
        /tmp/*) rm -rf -- "$TEST_ROOT" ;;
    esac
}
trap cleanup EXIT

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

mkdir -p "$FAKE_BIN"

cat >"$FAKE_BIN/lxc" <<'EOF'
#!/bin/sh
set -eu
case "${1:-} ${2:-}" in
    "info deployd-appimage-build") exit 0 ;;
    "config device") printf '%s\n' "$MCP_TEST_REPO_ROOT" ;;
    "exec deployd-appimage-build") printf '%s\n' "$*" >>"$MCP_TEST_CALLS" ;;
    *) exit 1 ;;
esac
EOF
chmod +x "$FAKE_BIN/lxc"

for wrapper in rust-analyzer fossil; do
    : >"$CALLS"
    MCP_TEST_REPO_ROOT="$REPO_ROOT" MCP_TEST_CALLS="$CALLS" \
        PATH="$FAKE_BIN:/usr/bin:/bin" \
        "$REPO_ROOT/scripts/mcp/$wrapper.sh"

    grep -q -- '--force-noninteractive' "$CALLS" \
        || fail "$wrapper omitted non-interactive LXD execution"
    grep -q -- '--user 1000 --group 1000' "$CALLS" \
        || fail "$wrapper omitted the non-root identity"
    grep -q -- '--env HOME=/home/ubuntu' "$CALLS" \
        || fail "$wrapper omitted the fixed HOME"
    grep -q -- 'unshare --user --map-root-user --mount --net' "$CALLS" \
        || fail "$wrapper omitted namespace isolation"

    if "$REPO_ROOT/scripts/mcp/$wrapper.sh" unexpected \
        >"$TEST_ROOT/stdout" 2>"$TEST_ROOT/stderr"; then
        fail "$wrapper accepted an argument"
    fi
done

echo "MCP wrapper tests passed"
