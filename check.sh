#!/usr/bin/env bash
# Run approved Cargo commands inside the Deployd LXD build environment.
set -euo pipefail

# Ignore SIGPIPE so piping output through `tail` doesn't kill the script
# prematurely when the reader closes early (e.g. `./check.sh clippy 2>&1 | tail -40`).
trap '' PIPE

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
LXD_CONTAINER="deployd-appimage-build"
BUILD_UID="1000"
BUILD_GID="1000"
BUILD_HOME="/home/ubuntu"
BUILD_PATH="$BUILD_HOME/.cargo/bin:/opt/appimage-tools:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
CMD="${1:-check}"
FEATURES="loot,libarchive-fallback"
shift "$(( $# > 0 ? 1 : 0 ))"

case "$CMD" in
    build|check|test|clippy|doc|metadata|tree|audit|fmt|env) ;;
    lock-update)
        if [ "$#" -ne 2 ]; then
            echo "error: lock-update requires <package-spec> <version>" >&2
            exit 2
        fi
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
        echo "supported commands: build, check, test, nextest, clippy, fmt, audit, doc, metadata, tree, env, lock-update" >&2
        exit 2
        ;;
esac

for arg in "$@"; do
    case "$arg" in
        --all-features|--no-default-features|--features|--features=*|--manifest-path|--manifest-path=*|--target-dir|--target-dir=*|--config|--config=*)
            echo "error: '$arg' overrides project-controlled Cargo configuration" >&2
            exit 2
            ;;
    esac
done

if [ "${DEPLOYD_BUILD_CONTAINER:-0}" != "1" ]; then
    if ! command -v lxc &>/dev/null || ! lxc info &>/dev/null 2>&1; then
        echo "error: LXD is required for ./check.sh" >&2
        echo "hint: ensure 'lxc info' works and the '$LXD_CONTAINER' container exists" >&2
        exit 1
    fi

    lxc start "$LXD_CONTAINER" 2>/dev/null || true
    exec lxc exec --force-noninteractive "$LXD_CONTAINER" \
        --cwd /workspace \
        --user "$BUILD_UID" \
        --group "$BUILD_GID" \
        --env "HOME=$BUILD_HOME" \
        --env "PATH=$BUILD_PATH" \
        --env DEPLOYD_BUILD_CONTAINER=1 \
        --env "DEPLOYD_DEPENDENCY_MAINTENANCE=${DEPLOYD_DEPENDENCY_MAINTENANCE:-0}" \
        --env "DEPLOYD_EXPERIMENTAL=${DEPLOYD_EXPERIMENTAL:-0}" \
        --env APPIMAGE_EXTRACT_AND_RUN=1 \
        --env CARGO_TARGET_DIR=/build/target \
        -- bash /workspace/check.sh "$CMD" "$@"
fi

if [ "$(id -u)" != "$BUILD_UID" ] || [ "$(id -g)" != "$BUILD_GID" ]; then
    echo "error: ./check.sh must run as UID $BUILD_UID and GID $BUILD_GID inside LXD" >&2
    exit 1
fi

if [ "$REPO_ROOT" != "/workspace" ]; then
    echo "error: container workspace must be mounted at /workspace" >&2
    exit 1
fi

if [ ! -x "$BUILD_HOME/.cargo/bin/cargo" ]; then
    echo "error: the non-root Rust toolchain is not provisioned" >&2
    echo "hint: run 'bash packaging/appimage/build-appimage.sh --setup-only'" >&2
    exit 1
fi

cd "$REPO_ROOT"

case "$CMD" in
    env)
        if [ "$#" -ne 0 ]; then
            echo "error: env does not accept arguments" >&2
            exit 2
        fi
        printf 'uid=%s\n' "$(id -u)"
        printf 'gid=%s\n' "$(id -g)"
        printf 'home=%s\n' "$HOME"
        printf 'rust=%s\n' "$(rustc --version)"
        printf 'workspace=%s\n' "$REPO_ROOT"
        printf 'target=%s\n' "${CARGO_TARGET_DIR:-}"
        ;;
    fmt)
        exec cargo fmt "$@"
        ;;
    audit)
        # Cargo audit cannot distinguish inactive lockfile features, so keep the reviewed RSA
        # exception valid only while no package variant can compile that crate.
        RSA_TREE="$(cargo tree --locked --features "$FEATURES" --target all -i rsa 2>&1)" || {
            printf '%s\n' "$RSA_TREE" >&2
            exit 1
        }
        if printf '%s\n' "$RSA_TREE" | grep -q '^rsa v'; then
            echo "error: the ignored RSA advisory is reachable; remove the exception and remediate it" >&2
            exit 1
        fi
        exec cargo audit "$@"
        ;;
    nextest)
        NEXTEST_SUBCMD="${1:-run}"
        if [ "$#" -gt 0 ]; then
            shift
        fi
        exec cargo nextest "$NEXTEST_SUBCMD" --locked --features "$FEATURES" "$@"
        ;;
    lock-update)
        exec cargo update --package "$1" --precise "$2"
        ;;
    *)
        exec cargo "$CMD" --locked --features "$FEATURES" "$@"
        ;;
esac
