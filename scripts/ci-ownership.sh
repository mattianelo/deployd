#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"

[ "$#" -gt 0 ] || {
    echo "usage: ci-ownership.sh <generated-path>..." >&2
    exit 2
}

for target in "$@"; do
    [ -e "$target" ] || {
        echo "error: generated path does not exist: $target" >&2
        exit 1
    }
    resolved="$(realpath "$target")"
    case "$resolved" in
        "$REPO_ROOT"/*) ;;
        *)
            echo "error: generated path is outside the checkout: $target" >&2
            exit 1
            ;;
    esac
    unexpected="$(find "$resolved" \( ! -uid 1000 -o ! -gid 1000 \) -print -quit)"
    if [ -n "$unexpected" ]; then
        echo "error: generated path is not owned by UID/GID 1000:1000: $unexpected" >&2
        exit 1
    fi
done
