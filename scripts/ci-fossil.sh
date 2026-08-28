#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
source "$REPO_ROOT/packaging/appimage/mcp-versions.sh"
"$REPO_ROOT/scripts/ci-environment.sh"

MODE="${1:-}"
OUTPUT_DIR="${2:-}"
BASE_REF="${3:-}"
BINARY="$HOME/.local/lib/deployd-mcp/fossil-mcp-$FOSSIL_MCP_VERSION"

if [ "$MODE" != "observe" ] && [ "$MODE" != "diff" ]; then
    echo "usage: ci-fossil.sh <observe|diff> <output-directory> [base-ref]" >&2
    exit 2
fi
if [ -z "$OUTPUT_DIR" ]; then
    echo "error: an output directory is required" >&2
    exit 2
fi
if [ ! -x "$BINARY" ]; then
    echo "error: pinned Fossil binary is unavailable: $BINARY" >&2
    exit 1
fi

RESOLVED_OUTPUT="$(realpath -m "$OUTPUT_DIR")"
case "$RESOLVED_OUTPUT" in
    "$REPO_ROOT"/*) ;;
    *)
        echo "error: Fossil output must stay inside the checkout" >&2
        exit 2
        ;;
esac
mkdir -p "$RESOLVED_OUTPUT"

export FOSSIL_NO_UPDATE_CHECK=1
if [ "$MODE" = "observe" ]; then
    [ -z "$BASE_REF" ] || {
        echo "error: observe mode does not accept a base ref" >&2
        exit 2
    }
    "$BINARY" scan \
        --config "$REPO_ROOT/fossil.toml" \
        --format json \
        --output "$RESOLVED_OUTPUT/fossil-scan.json" \
        "$REPO_ROOT"
    "$BINARY" scaffolding \
        --config "$REPO_ROOT/fossil.toml" \
        --format json \
        --output "$RESOLVED_OUTPUT/fossil-scaffolding.json" \
        "$REPO_ROOT"
    test -s "$RESOLVED_OUTPUT/fossil-scan.json"
    test -s "$RESOLVED_OUTPUT/fossil-scaffolding.json"
    wc -c "$RESOLVED_OUTPUT/fossil-scan.json" \
        "$RESOLVED_OUTPUT/fossil-scaffolding.json"
else
    case "$BASE_REF" in
        ''|-*|*[!A-Za-z0-9._/-]*|*..*)
            echo "error: invalid Fossil base ref '$BASE_REF'" >&2
            exit 2
            ;;
    esac
    git -C "$REPO_ROOT" rev-parse --verify --quiet "$BASE_REF^{commit}" >/dev/null || {
        echo "error: Fossil base ref is unavailable: $BASE_REF" >&2
        exit 1
    }
    "$BINARY" check \
        --diff "$BASE_REF" \
        --max-dead-code 0 \
        --max-clones 4294967295 \
        --max-scaffolding 0 \
        --min-confidence high \
        --fail-on-scaffolding \
        --config "$REPO_ROOT/fossil.toml" \
        --format json \
        --output "$RESOLVED_OUTPUT/fossil-diff.json" \
        "$REPO_ROOT"
    test -s "$RESOLVED_OUTPUT/fossil-diff.json"
fi

"$REPO_ROOT/scripts/ci-ownership.sh" "$RESOLVED_OUTPUT"
