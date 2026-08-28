#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
BUILD_UID="1000"
BUILD_GID="1000"
BUILD_HOME="/home/ubuntu"
FEATURES="loot,libarchive-fallback"

usage() {
    echo "usage: rust-command.sh <validate|run> [command] [arguments...]" >&2
    exit 2
}

validate_command() {
    CMD="${1:-check}"
    shift "$(( $# > 0 ? 1 : 0 ))"
    CMD_ARGS=("$@")

    case "$CMD" in
        build|check|test|clippy|doc|metadata|tree|audit|fmt) ;;
        env)
            [ "$#" -eq 0 ] || {
                echo "error: env does not accept arguments" >&2
                exit 2
            }
            ;;
        freshness)
            if [ "${DEPLOYD_CI_FRESHNESS:-0}" != "1" ]; then
                echo "error: freshness is restricted to the scheduled CI report" >&2
                exit 2
            fi
            [ "$#" -eq 0 ] || {
                echo "error: freshness does not accept arguments" >&2
                exit 2
            }
            ;;
        lock-update)
            [ "$#" -eq 2 ] || {
                echo "error: lock-update requires <package-spec> <version>" >&2
                exit 2
            }
            case "$1" in
                *[!A-Za-z0-9_@.+-]*|'')
                    echo "error: invalid package spec '$1'" >&2
                    exit 2
                    ;;
            esac
            case "$2" in
                [0-9]*) ;;
                *)
                    echo "error: invalid package version '$2'" >&2
                    exit 2
                    ;;
            esac
            case "$2" in
                *[!A-Za-z0-9.+-]*)
                    echo "error: invalid package version '$2'" >&2
                    exit 2
                    ;;
            esac
            if [ "${DEPLOYD_DEPENDENCY_MAINTENANCE:-0}" != "1" ]; then
                echo "error: lock-update requires DEPLOYD_DEPENDENCY_MAINTENANCE=1" >&2
                exit 2
            fi
            ;;
        nextest)
            if [ "${1:-run}" != "run" ] && [ "${1:-run}" != "list" ]; then
                echo "error: unsupported nextest subcommand '${1:-}'" >&2
                exit 2
            fi
            ;;
        *)
            echo "error: unsupported check command '$CMD'" >&2
            echo "supported commands: build, check, test, nextest, clippy, fmt, audit, doc, metadata, tree, env, freshness, lock-update" >&2
            exit 2
            ;;
    esac

    for arg in "${CMD_ARGS[@]}"; do
        case "$arg" in
            --all-features|--no-default-features|--features|--features=*|--manifest-path|--manifest-path=*|--target-dir|--target-dir=*|--config|--config=*)
                echo "error: '$arg' overrides project-controlled Cargo configuration" >&2
                exit 2
                ;;
        esac
    done
}

run_command() {
    if [ "$(id -u)" != "$BUILD_UID" ] || [ "$(id -g)" != "$BUILD_GID" ]; then
        echo "error: Rust commands must run as UID $BUILD_UID and GID $BUILD_GID" >&2
        exit 1
    fi
    if [ "${HOME:-}" != "$BUILD_HOME" ]; then
        echo "error: Rust commands require HOME=$BUILD_HOME" >&2
        exit 1
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        echo "error: the pinned Cargo toolchain is unavailable" >&2
        exit 1
    fi

    cd "$REPO_ROOT"

    case "$CMD" in
        env)
            [ "${#CMD_ARGS[@]}" -eq 0 ] || {
                echo "error: env does not accept arguments" >&2
                exit 2
            }
            printf 'uid=%s\n' "$(id -u)"
            printf 'gid=%s\n' "$(id -g)"
            printf 'home=%s\n' "$HOME"
            printf 'rust=%s\n' "$(rustc --version)"
            printf 'workspace=%s\n' "$REPO_ROOT"
            printf 'target=%s\n' "${CARGO_TARGET_DIR:-}"
            ;;
        fmt)
            exec cargo fmt "${CMD_ARGS[@]}"
            ;;
        audit)
            RSA_TREE="$(cargo tree --locked --features "$FEATURES" --target all -i rsa 2>&1)" || {
                printf '%s\n' "$RSA_TREE" >&2
                exit 1
            }
            if printf '%s\n' "$RSA_TREE" | grep -q '^rsa v'; then
                echo "error: the ignored RSA advisory is reachable; remove the exception and remediate it" >&2
                exit 1
            fi
            exec cargo audit "${CMD_ARGS[@]}"
            ;;
        nextest)
            NEXTEST_SUBCMD="${CMD_ARGS[0]:-run}"
            if [ "${#CMD_ARGS[@]}" -gt 0 ]; then
                CMD_ARGS=("${CMD_ARGS[@]:1}")
            fi
            exec cargo nextest "$NEXTEST_SUBCMD" --locked --features "$FEATURES" "${CMD_ARGS[@]}"
            ;;
        freshness)
            exec cargo update --dry-run
            ;;
        lock-update)
            exec cargo update --package "${CMD_ARGS[0]}" --precise "${CMD_ARGS[1]}"
            ;;
        *)
            exec cargo "$CMD" --locked --features "$FEATURES" "${CMD_ARGS[@]}"
            ;;
    esac
}

[ "$#" -gt 0 ] || usage
MODE="$1"
shift
validate_command "$@"

case "$MODE" in
    validate) ;;
    run) run_command ;;
    *) usage ;;
esac
