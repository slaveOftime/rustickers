#!/usr/bin/env sh
set -eu

PREFIX="${HOME}/.local"
BIN_DIR="$PREFIX/bin"
APP_DIR="$PREFIX/share/applications"
ICON_DIR="$PREFIX/share/icons/hicolor/256x256/apps"

if [ ! -f "./rustickers" ]; then
  echo "Expected ./rustickers next to this script." >&2
  echo "Run this from the extracted release folder." >&2
  exit 1
fi

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR"

install -m 755 "./rustickers" "$BIN_DIR/rustickers"
install -m 644 "./rustickers.desktop" "$APP_DIR/rustickers.desktop"
install -m 644 "./rustickers.png" "$ICON_DIR/rustickers.png"

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -f "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true

echo "Installed: $BIN_DIR/rustickers"
echo "Desktop entry: $APP_DIR/rustickers.desktop"
echo "Icon: $ICON_DIR/rustickers.png"
