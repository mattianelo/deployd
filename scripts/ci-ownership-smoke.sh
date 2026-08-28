#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
JOB_ID="${CI_JOB_ID:-local}"

case "$JOB_ID" in
    *[!A-Za-z0-9_-]*|'')
        echo "error: invalid CI job identifier" >&2
        exit 2
        ;;
esac

PROBE_ROOT="$REPO_ROOT/.ci-artifacts/ownership/$JOB_ID"
RESOLVED_PROBE="$(realpath -m "$PROBE_ROOT")"
case "$RESOLVED_PROBE" in
    "$REPO_ROOT"/*) ;;
    *)
        echo "error: ownership probe must stay inside the checkout" >&2
        exit 1
        ;;
esac
if [ -e "$RESOLVED_PROBE" ]; then
    echo "error: ownership probe already exists" >&2
    exit 1
fi

mkdir -p "$RESOLVED_PROBE/generated"
printf 'uid=%s gid=%s\n' "$(id -u)" "$(id -g)" \
    >"$RESOLVED_PROBE/generated/owner.txt"
"$REPO_ROOT/scripts/ci-ownership.sh" "$RESOLVED_PROBE"
