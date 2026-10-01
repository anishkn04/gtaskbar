#!/usr/bin/env bash
#
# Build and install gtaskbar into the current user's home directory.
# No root required: everything lands under ~/.local, plus the XDG autostart
# entry so the app can run in the tray at login.
#
# Usage:
#   ./install.sh              build release and install
#   ./install.sh --no-build   install an already-built binary
#   ./install.sh --uninstall  remove everything this script installed

set -euo pipefail

APP_ID="dev.anishkn04.gtaskbar"
BIN_NAME="gtaskbar"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"

BIN_DIR="$PREFIX/bin"
DATA_DIR="$PREFIX/share/$BIN_NAME"
APP_DIR="$PREFIX/share/applications"
ICON_SCALE="$PREFIX/share/icons/hicolor/scalable/apps"
ICON_SYMBOLIC="$PREFIX/share/icons/hicolor/symbolic/apps"
AUTOSTART_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"

BUILD=1
UNINSTALL=0
for arg in "$@"; do
    case "$arg" in
        --no-build) BUILD=0 ;;
        --uninstall) UNINSTALL=0; UNINSTALL=1 ;;
        -h|--help)
            sed -n '3,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown option: $arg" >&2
            exit 2
            ;;
    esac
done

uninstall() {
    rm -f "$BIN_DIR/$BIN_NAME"
    rm -f "$APP_DIR/$BIN_NAME.desktop"
    rm -f "$AUTOSTART_DIR/$BIN_NAME.desktop"
    rm -f "$ICON_SCALE/$BIN_NAME.svg"
    # Remove every bundled symbolic icon, not just the app's own.
    rm -f "$ICON_SYMBOLIC/$BIN_NAME"*.svg
    rm -rf "$DATA_DIR"
    # Only prune the icon cache if update-desktop-cache and gtk-update-icon-cache
    # are actually installed; missing tools are not a reason to fail.
    command -v gtk-update-icon-cache >/dev/null 2>&1 \
        && gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true
    command -v update-desktop-database >/dev/null 2>&1 \
        && update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
    echo "gtaskbar removed."
}

if [[ $UNINSTALL -eq 1 ]]; then
    uninstall
    exit 0
fi

if [[ $BUILD -eq 1 ]]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "error: cargo not found in PATH." >&2
        echo "Install Rust from https://rustup.rs, or re-run with --no-build" >&2
        echo "after providing the binary yourself." >&2
        exit 1
    fi
    echo "==> Building release binary"
    cargo build --release --locked
fi

SOURCE_BIN="$SCRIPT_DIR/target/release/$BIN_NAME"
if [[ ! -x "$SOURCE_BIN" ]]; then
    echo "error: $SOURCE_BIN not found or not executable." >&2
    echo "Run without --no-build to build it." >&2
    exit 1
fi

echo "==> Installing to $PREFIX"
mkdir -p "$BIN_DIR" "$DATA_DIR" "$APP_DIR" "$ICON_SCALE" "$ICON_SYMBOLIC" "$AUTOSTART_DIR"

install -Dm755 "$SOURCE_BIN" "$BIN_DIR/$BIN_NAME"
install -Dm644 "$SCRIPT_DIR/data/icons/scalable/apps/$BIN_NAME.svg" "$ICON_SCALE/$BIN_NAME.svg"

# The symbolic variant of the app icon, for the tray and for notifications.
# Interface icons are not installed: they are official GNOME icons resolved from
# the system icon theme at runtime.
for icon in "$SCRIPT_DIR/data/icons/symbolic/apps/$BIN_NAME"*.svg; do
    [ -e "$icon" ] || continue
    install -Dm644 "$icon" "$ICON_SYMBOLIC/$(basename "$icon")"
done

install -Dm644 "$SCRIPT_DIR/data/$BIN_NAME.desktop" "$APP_DIR/$BIN_NAME.desktop"
install -Dm644 "$SCRIPT_DIR/data/$BIN_NAME-autostart.desktop" "$AUTOSTART_DIR/$BIN_NAME.desktop"

# Mark the app as untrusted in the autostart launcher so it can start without
# the "untrusted desktop launcher" prompt on GNOME.
if command -v gio >/dev/null 2>&1; then
    gio set "$AUTOSTART_DIR/$BIN_NAME.desktop" metadata::trusted true >/dev/null 2>&1 || true
fi

# Refresh caches so the new icon and desktop entry are picked up immediately.
command -v gtk-update-icon-cache >/dev/null 2>&1 \
    && gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true
command -v update-desktop-database >/dev/null 2>&1 \
    && update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true

echo
echo "Installed $BIN_NAME to $BIN_DIR/$BIN_NAME"
echo
echo "Next steps:"
echo "  1. Launch it:            $BIN_NAME"
echo "  2. Create an OAuth client at https://console.cloud.google.com:"
echo "     - enable the Google Tasks API"
echo "     - OAuth consent screen -> External, add yourself as a test user"
echo "     - Credentials -> OAuth client ID -> Desktop app"
echo "  3. Open Preferences in the app and paste the client ID and secret."
echo
