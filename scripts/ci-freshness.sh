#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
"$REPO_ROOT/scripts/ci-environment.sh"

OUTPUT_DIR="$REPO_ROOT/.ci-artifacts/freshness"
RESOLVED_OUTPUT="$(realpath -m "$OUTPUT_DIR")"
case "$RESOLVED_OUTPUT" in
    "$REPO_ROOT"/*) ;;
    *)
        echo "error: freshness output must stay inside the checkout" >&2
        exit 1
        ;;
esac
mkdir -p "$RESOLVED_OUTPUT"
REPORT="$OUTPUT_DIR/cargo-update-dry-run.txt"

set +e
DEPLOYD_CI_FRESHNESS=1 \
    "$REPO_ROOT/scripts/rust-command.sh" run freshness >"$REPORT" 2>&1
STATUS=$?
set -e
cat "$REPORT"
"$REPO_ROOT/scripts/ci-ownership.sh" "$OUTPUT_DIR"
"$REPO_ROOT/scripts/ci-ownership-smoke.sh"
exit "$STATUS"
