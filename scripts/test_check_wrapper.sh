#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_DIR="$(mktemp -d)"
LXC_LOG="$TEST_DIR/lxc.log"

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

assert_rejected() {
    if "$REPO_ROOT/check.sh" "$@" >"$TEST_DIR/stdout" 2>"$TEST_DIR/stderr"; then
        fail "expected rejection for: $*"
    fi
}

printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'case "${1:-}" in' \
    '    info|start) exit 0 ;;' \
    '    exec) printf "%s\n" "$@" > "$LXC_LOG"; exit "${LXC_EXIT_CODE:-0}" ;;' \
    '    *) exit 1 ;;' \
    'esac' >"$TEST_DIR/lxc"
chmod +x "$TEST_DIR/lxc"

assert_rejected run
assert_rejected nextest archive
assert_rejected check --all-features
assert_rejected check --features extra
assert_rejected check --manifest-path ../other/Cargo.toml
assert_rejected check --target-dir target
assert_rejected check --config net.git-fetch-with-cli=true
assert_rejected env unexpected
assert_rejected freshness
assert_rejected lock-update anyhow@1.0.102 1.0.103

if DEPLOYD_DEPENDENCY_MAINTENANCE=1 "$REPO_ROOT/check.sh" \
    lock-update 'bad/package' 1.0.103 >"$TEST_DIR/stdout" 2>"$TEST_DIR/stderr"; then
    fail "accepted an invalid dependency package spec"
fi

PATH="$TEST_DIR:$PATH" LXC_LOG="$LXC_LOG" "$REPO_ROOT/check.sh" check --locked

grep -Fx -- '--user' "$LXC_LOG" >/dev/null || fail "missing LXD user option"
grep -Fx -- '1000' "$LXC_LOG" >/dev/null || fail "missing non-root UID or GID"
grep -Fx -- 'HOME=/home/ubuntu' "$LXC_LOG" >/dev/null || fail "missing non-root HOME"
grep -Fx -- 'DEPLOYD_BUILD_CONTAINER=1' "$LXC_LOG" >/dev/null || fail "missing container marker"
grep -Fx -- 'CARGO_TARGET_DIR=/build/target' "$LXC_LOG" >/dev/null || fail "missing container target directory"
grep -Fx -- '/workspace/check.sh' "$LXC_LOG" >/dev/null || fail "missing container check script"
grep -Fx -- 'check' "$LXC_LOG" >/dev/null || fail "missing Cargo command"
grep -Fx -- '--locked' "$LXC_LOG" >/dev/null || fail "missing forwarded safe argument"

PATH="$TEST_DIR:$PATH" LXC_LOG="$LXC_LOG" "$REPO_ROOT/check.sh" env
grep -Fx -- 'env' "$LXC_LOG" >/dev/null || fail "missing diagnostic command"

PATH="$TEST_DIR:$PATH" LXC_LOG="$LXC_LOG" DEPLOYD_CI_FRESHNESS=1 \
    "$REPO_ROOT/check.sh" freshness
grep -Fx -- 'DEPLOYD_CI_FRESHNESS=1' "$LXC_LOG" >/dev/null || \
    fail "missing scheduled-freshness marker"
grep -Fx -- 'freshness' "$LXC_LOG" >/dev/null || fail "missing freshness command"

PATH="$TEST_DIR:$PATH" LXC_LOG="$LXC_LOG" DEPLOYD_DEPENDENCY_MAINTENANCE=1 \
    "$REPO_ROOT/check.sh" lock-update anyhow@1.0.102 1.0.103
grep -Fx -- 'DEPLOYD_DEPENDENCY_MAINTENANCE=1' "$LXC_LOG" >/dev/null || \
    fail "missing dependency-maintenance marker"
grep -Fx -- 'lock-update' "$LXC_LOG" >/dev/null || fail "missing lock-update command"

set +e
PATH="$TEST_DIR:$PATH" LXC_LOG="$LXC_LOG" LXC_EXIT_CODE=17 \
    "$REPO_ROOT/check.sh" check >"$TEST_DIR/stdout" 2>"$TEST_DIR/stderr"
STATUS=$?
set -e
[ "$STATUS" -eq 17 ] || fail "LXD failure status was not propagated"

echo "check wrapper tests passed"
