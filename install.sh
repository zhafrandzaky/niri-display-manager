#!/usr/bin/env bash
# Install or uninstall niri-display-manager for the current user.
#
# Usage:
#   ./install.sh                 build and install into $PREFIX (default ~/.local)
#   ./install.sh --skip-build    install the existing release binary
#   ./install.sh --uninstall     remove installed files
#
# Environment:
#   PREFIX       install prefix (default: $HOME/.local)
#   SKIP_BUILD   set to 1 to skip the release build

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
SKIP_BUILD="${SKIP_BUILD:-0}"
MODE="install"

BIN_DIR="$PREFIX/bin"
APP_DIR="$PREFIX/share/applications"
ICON_DIR="$PREFIX/share/icons/hicolor/scalable/apps"

usage() {
    printf 'usage: %s [--skip-build|--uninstall]\n' "$0"
}

for argument in "$@"; do
    case "$argument" in
        --uninstall) MODE="uninstall" ;;
        --skip-build) SKIP_BUILD=1 ;;
        -h|--help) usage; exit 0 ;;
        *)
            printf 'error: unknown option: %s\n' "$argument" >&2
            usage >&2
            exit 2
            ;;
    esac
done

refresh_caches() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -d "$PREFIX/share/icons/hicolor" ]; then
        gtk-update-icon-cache -q -t -f "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true
    fi
}

remove_file() {
    if [ -e "$1" ]; then
        rm -f -- "$1"
        printf 'removed  %s\n' "$1"
    fi
}

if [ "$MODE" = "uninstall" ]; then
    remove_file "$APP_DIR/niri-display-manager.desktop"
    remove_file "$ICON_DIR/niri-display-manager.svg"
    remove_file "$BIN_DIR/niri-display-manager"
    refresh_caches
    printf 'uninstall complete\n'
    exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
    printf 'error: cargo is required; install rustup first\n' >&2
    exit 1
fi
if ! command -v niri >/dev/null 2>&1; then
    printf 'warning: niri was not found in PATH; the manager needs a running niri session\n'
fi
if ! command -v wl-mirror >/dev/null 2>&1; then
    printf 'warning: wl-mirror was not found; mirroring will not start until it is installed\n'
fi

if [ "$SKIP_BUILD" != "1" ]; then
    printf 'building release binary...\n'
    (cd "$SCRIPT_DIR" && cargo build --release)
fi

if [ ! -x "$SCRIPT_DIR/target/release/niri-display-manager" ]; then
    printf 'error: release binary not found; run cargo build --release first\n' >&2
    exit 1
fi

install -d "$BIN_DIR" "$APP_DIR" "$ICON_DIR"
install -m 755 "$SCRIPT_DIR/target/release/niri-display-manager" "$BIN_DIR/niri-display-manager"
install -m 644 "$SCRIPT_DIR/data/niri-display-manager.desktop" "$APP_DIR/niri-display-manager.desktop"
install -m 644 "$SCRIPT_DIR/data/icons/hicolor/scalable/apps/niri-display-manager.svg" "$ICON_DIR/niri-display-manager.svg"

printf 'installed:\n'
printf '  %s\n' "$BIN_DIR/niri-display-manager"
printf '  %s\n' "$APP_DIR/niri-display-manager.desktop"
printf '  %s\n' "$ICON_DIR/niri-display-manager.svg"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) printf 'note: %s is not on PATH; add it to launch the manager from a shell\n' "$BIN_DIR" ;;
esac

refresh_caches
printf 'done\n'
