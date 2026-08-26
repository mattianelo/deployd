#!/bin/sh
set -eu

REPO_ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
TEST_DIR="$(mktemp -d)"
FIXTURE_ROOT="$TEST_DIR/fixture"
SENTINEL="$TEST_DIR/keep-me"

cleanup() {
    case "$TEST_DIR" in
        /tmp/*) rm -rf -- "$TEST_DIR" ;;
    esac
}
trap cleanup EXIT

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

mkdir -p \
    "$FIXTURE_ROOT/scripts" \
    "$FIXTURE_ROOT/pages" \
    "$FIXTURE_ROOT/data/screenshots" \
    "$FIXTURE_ROOT/data/icons/hicolor/scalable/apps"
cp "$REPO_ROOT/scripts/build-pages.sh" "$FIXTURE_ROOT/scripts/"
printf '%s\n' '<html></html>' >"$FIXTURE_ROOT/pages/index.html"
printf '%s\n' 'screenshot' >"$FIXTURE_ROOT/data/screenshots/example.png"
printf '%s\n' '<svg></svg>' >"$FIXTURE_ROOT/data/icons/hicolor/scalable/apps/deployd.svg"
printf '%s\n' 'preserve' >"$SENTINEL"

if DEPLOYD_PAGES_OUT="$TEST_DIR" sh "$FIXTURE_ROOT/scripts/build-pages.sh" \
    >"$TEST_DIR/stdout" 2>"$TEST_DIR/stderr"; then
    fail "accepted an output directory outside the checkout"
fi
[ -f "$SENTINEL" ] || fail "outside sentinel was removed"

sh "$FIXTURE_ROOT/scripts/build-pages.sh" >"$TEST_DIR/stdout"
[ -f "$FIXTURE_ROOT/out/index.html" ] || fail "missing generated index"
[ -f "$FIXTURE_ROOT/out/assets/screenshots/example.png" ] || fail "missing screenshot"
[ -f "$FIXTURE_ROOT/out/assets/icons/deployd.svg" ] || fail "missing application icon"

rm -rf -- "$FIXTURE_ROOT/out"
ln -s "$TEST_DIR" "$FIXTURE_ROOT/out"
if sh "$FIXTURE_ROOT/scripts/build-pages.sh" >"$TEST_DIR/stdout" 2>"$TEST_DIR/stderr"; then
    fail "accepted a symlinked output directory"
fi
[ -f "$SENTINEL" ] || fail "symlink target sentinel was removed"

echo "Pages build tests passed"
