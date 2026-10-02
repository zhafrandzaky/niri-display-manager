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
#
# Icons:
#   The scalable SVG is installed as-is. A 128x128 PNG is rendered at install
#   time with rsvg-convert, magick, or convert (first available). When no
#   renderer is available, the pre-rendered PNG committed under data/icons is
#   used; if that is missing too, the scalable icon is installed alone.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
SKIP_BUILD="${SKIP_BUILD:-0}"
MODE="install"

BIN_DIR="$PREFIX/bin"
APP_DIR="$PREFIX/share/applications"
ICONS_ROOT="$PREFIX/share/icons/hicolor"
SVG_DIR="$ICONS_ROOT/scalable/apps"
PNG_DIR="$ICONS_ROOT/128x128/apps"

ICON_NAME="niri-display-manager"
ICON_SVG_SRC="$SCRIPT_DIR/data/icons/hicolor/scalable/apps/$ICON_NAME.svg"
ICON_PNG_SRC="$SCRIPT_DIR/data/icons/hicolor/128x128/apps/$ICON_NAME.png"

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
    if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -d "$ICONS_ROOT" ]; then
        gtk-update-icon-cache -f -t "$ICONS_ROOT" >/dev/null 2>&1 || true
    fi
}

remove_file() {
    if [ -e "$1" ]; then
        rm -f -- "$1"
        printf 'removed  %s\n' "$1"
    fi
}

# Print the path of the PNG to install.
#
# A fresh raster is rendered into target/icon-staging so the repository tree
# stays untouched; the committed PNG is the fallback when no renderer exists.
prepare_png_icon() {
    local staging_dir="$SCRIPT_DIR/target/icon-staging"
    local staging_png="$staging_dir/$ICON_NAME.png"

    if [ -f "$ICON_SVG_SRC" ]; then
        mkdir -p "$staging_dir"
        if command -v rsvg-convert >/dev/null 2>&1; then
            if rsvg-convert -w 128 -h 128 -o "$staging_png" "$ICON_SVG_SRC" 2>/dev/null; then
                printf '%s\n' "$staging_png"
                return 0
            fi
        fi
        if command -v magick >/dev/null 2>&1; then
            if magick -background none "$ICON_SVG_SRC" -resize 128x128 "$staging_png" 2>/dev/null; then
                printf '%s\n' "$staging_png"
                return 0
            fi
        fi
        if command -v convert >/dev/null 2>&1; then
            if convert -background none "$ICON_SVG_SRC" -resize 128x128 "$staging_png" 2>/dev/null; then
                printf '%s\n' "$staging_png"
                return 0
            fi
        fi
    fi

    if [ -f "$ICON_PNG_SRC" ]; then
        printf 'warning: no SVG renderer available; using the pre-rendered PNG\n' >&2
        printf '%s\n' "$ICON_PNG_SRC"
        return 0
    fi

    printf 'warning: no 128x128 PNG icon could be produced\n' >&2
    return 1
}

if [ "$MODE" = "uninstall" ]; then
    remove_file "$APP_DIR/$ICON_NAME.desktop"
    remove_file "$SVG_DIR/$ICON_NAME.svg"
    remove_file "$PNG_DIR/$ICON_NAME.png"
    remove_file "$BIN_DIR/$ICON_NAME"
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

png_source="$(prepare_png_icon)" || png_source=""

install -d "$BIN_DIR" "$APP_DIR" "$SVG_DIR"
install -m 755 "$SCRIPT_DIR/target/release/niri-display-manager" "$BIN_DIR/niri-display-manager"
install -m 644 "$SCRIPT_DIR/data/niri-display-manager.desktop" "$APP_DIR/niri-display-manager.desktop"
install -m 644 "$ICON_SVG_SRC" "$SVG_DIR/$ICON_NAME.svg"

printf 'installed:\n'
printf '  %s\n' "$BIN_DIR/niri-display-manager"
printf '  %s\n' "$APP_DIR/niri-display-manager.desktop"
printf '  %s\n' "$SVG_DIR/$ICON_NAME.svg"

if [ -n "$png_source" ]; then
    install -d "$PNG_DIR"
    install -m 644 "$png_source" "$PNG_DIR/$ICON_NAME.png"
    printf '  %s\n' "$PNG_DIR/$ICON_NAME.png"
fi

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) printf 'note: %s is not on PATH; add it to launch the manager from a shell\n' "$BIN_DIR" ;;
esac

refresh_caches
printf 'done\n'
