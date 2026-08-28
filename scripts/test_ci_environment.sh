#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

HOME=/home/ubuntu \
CI_PROJECT_DIR="$REPO_ROOT" \
CARGO_HOME="$REPO_ROOT/.ci-cache/cargo" \
CARGO_TARGET_DIR="$REPO_ROOT/.ci-cache/target" \
    "$REPO_ROOT/scripts/ci-environment.sh"

if HOME=/home/ubuntu \
    CI_PROJECT_DIR="$REPO_ROOT" \
    CARGO_HOME=/tmp/deployd-external-cargo \
    "$REPO_ROOT/scripts/ci-environment.sh" >/dev/null 2>&1
then
    fail "accepted an external Cargo cache path"
fi

if HOME=/home/ubuntu \
    CI_PROJECT_DIR=/tmp/deployd-other-checkout \
    "$REPO_ROOT/scripts/ci-environment.sh" >/dev/null 2>&1
then
    fail "accepted a mismatched CI checkout"
fi

echo "CI environment tests passed"
