#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
"$REPO_ROOT/scripts/ci-environment.sh"

GROUP="${1:-}"
[ "$#" -eq 1 ] || {
    echo "usage: ci-inventory-tests.sh <variant:name|engine:name>" >&2
    exit 2
}

FILTER="$(python3 "$REPO_ROOT/scripts/ci_test_inventory.py" filter "$GROUP")"
"$REPO_ROOT/scripts/rust-command.sh" run nextest run -E "$FILTER"
"$REPO_ROOT/scripts/ci-ownership.sh" \
    "${CARGO_HOME:?CARGO_HOME is required}" \
    "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is required}"
