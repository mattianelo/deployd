#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"

if [ "$(id -u)" != "1000" ] || [ "$(id -g)" != "1000" ]; then
    echo "error: CI enforcement must run as UID/GID 1000:1000" >&2
    exit 1
fi
if [ "${HOME:-}" != "/home/ubuntu" ]; then
    echo "error: CI enforcement requires HOME=/home/ubuntu" >&2
    exit 1
fi
if [ -n "${CI_PROJECT_DIR:-}" ] \
    && [ "$(realpath "$CI_PROJECT_DIR")" != "$REPO_ROOT" ]
then
    echo "error: CI_PROJECT_DIR does not identify this checkout" >&2
    exit 1
fi

for variable in CARGO_HOME CARGO_TARGET_DIR; do
    value="${!variable:-}"
    [ -n "$value" ] || continue
    resolved="$(realpath -m "$value")"
    case "$resolved" in
        "$REPO_ROOT"/*) ;;
        *)
            echo "error: $variable must stay inside the checkout" >&2
            exit 1
            ;;
    esac
done
