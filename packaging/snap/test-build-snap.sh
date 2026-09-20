#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TEST_ROOT="$(mktemp -d)"
PROJECT_ROOT="$TEST_ROOT/project"
FAKE_BIN="$TEST_ROOT/bin"
LXC_LOG="$TEST_ROOT/lxc.log"

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

mkdir -p "$PROJECT_ROOT/packaging/snap" "$FAKE_BIN"
cp "$REPO_ROOT/packaging/snap/build-snap.sh" "$PROJECT_ROOT/packaging/snap/build-snap.sh"
printf '[package]\nname = "deployd"\nversion = "3.0.0"\n' > "$PROJECT_ROOT/Cargo.toml"

printf '%s\n' \
    '#!/bin/bash' \
    'set -euo pipefail' \
    'printf "%s\n" "$*" >> "$LXC_LOG"' \
    'case "${1:-}" in' \
    '    info|start|exec|delete|launch|config) exit 0 ;;' \
    '    file)' \
    '        [ "${2:-}" = "pull" ] || exit 1' \
    '        mkdir -p "$(dirname "$4")"' \
    '        : > "$4"' \
    '        ;;' \
    '    *) exit 1 ;;' \
    'esac' > "$FAKE_BIN/lxc"
chmod +x "$FAKE_BIN/lxc"

PATH="$FAKE_BIN:$PATH" LXC_LOG="$LXC_LOG" \
    "$PROJECT_ROOT/packaging/snap/build-snap.sh" --development

grep -F -- 'snap/snapcraft-dev.yaml /build/deployd-snap/snap/snapcraft.yaml' \
    "$LXC_LOG" >/dev/null || fail "development recipe was not selected"
[ -f "$PROJECT_ROOT/out/snap/deployd-dev_3.0.0_amd64.snap" ] || \
    fail "development artifact name is incorrect"

: > "$LXC_LOG"
PATH="$FAKE_BIN:$PATH" LXC_LOG="$LXC_LOG" \
    "$PROJECT_ROOT/packaging/snap/build-snap.sh"

if grep -F -- 'snapcraft-dev.yaml' "$LXC_LOG" >/dev/null; then
    fail "production build selected the development recipe"
fi
[ -f "$PROJECT_ROOT/out/snap/deployd_3.0.0_amd64.snap" ] || \
    fail "production artifact name is incorrect"

if PATH="$FAKE_BIN:$PATH" LXC_LOG="$LXC_LOG" \
    "$PROJECT_ROOT/packaging/snap/build-snap.sh" --unknown \
    > "$TEST_ROOT/stdout" 2> "$TEST_ROOT/stderr"; then
    fail "unknown build option was accepted"
fi

echo "Snap build wrapper tests passed"
